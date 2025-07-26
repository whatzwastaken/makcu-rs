//! A library for interacting with a serial-controlled device that supports mouse button simulation and movement.
use bytes::BytesMut;
use crossbeam_channel::{bounded, Sender, Receiver};
use memchr::{memchr, memchr2};
use parking_lot::{Mutex, RwLock};
use once_cell::sync::Lazy;
use rustc_hash::FxHashMap;
use serialport::SerialPort; 
use std::time::Instant;

use std::{
    io::{Write},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

const CRLF: &str = "\r\n";
const DEBUG_IO: bool = false;
use std::collections::VecDeque;
static SENT_LOG: Lazy<Mutex<VecDeque<Vec<u8>>>> =
    Lazy::new(|| Mutex::new(VecDeque::with_capacity(32)));
const LOG_MAX: usize = 32;
// ============================= Errors ======================================

#[derive(Debug, thiserror::Error)]
/// Represents the different types of errors that may occur while using Makcu.
pub enum MakcuError {
    #[error("Connection error: {0}")]
    /// A connection error occurred.
    Connection(String),
    #[error("Command error: {0}")]
    /// A command failed or returned an error.
    Command(String),
    #[error("Timeout for command {0}")]
    /// A command timed out after a specified duration.
    Timeout(u32),
}

pub type MakcuResult<T> = Result<T, MakcuError>;

// ========================= Helper structures ===============================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseButtonStates {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub side1: bool,
    pub side2: bool,
}
impl MouseButtonStates {
    #[inline]
    pub fn from_mask(mask: u8) -> Self {
        Self {
            left: mask & 0x01 != 0,
            right: mask & 0x02 != 0,
            middle: mask & 0x04 != 0,
            side1: mask & 0x08 != 0,
            side2: mask & 0x10 != 0,
        }
    }
    #[allow(dead_code)]
    #[inline]
    pub fn to_mask(self) -> u8 {
        (self.left as u8)
            | ((self.right as u8) << 1)
            | ((self.middle as u8) << 2)
            | ((self.side1 as u8) << 3)
            | ((self.side2 as u8) << 4)
    }
}



struct PendingCommand {
     tx: Sender<String>,
     tag: &'static str,
     started: Instant,
}

// ========================= Performance profiler (optional) =================

#[cfg(feature = "profile")]
mod profiler {
    use super::*;
    use once_cell::sync::Lazy;
    use std::time::Instant;

    static PROFILER: Lazy<Mutex<FxHashMap<&'static str, (u64, f64)>>> =
        Lazy::new(|| Mutex::new(FxHashMap::default()));

    pub struct Perf;
    impl Perf {
        pub fn measure<F: FnOnce()>(name: &'static str, f: F) {
            let t0 = Instant::now();
            f();
            let dt = t0.elapsed().as_secs_f64() * 1e6;
            let mut map = PROFILER.lock();
            let e = map.entry(name).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += dt;
        }
        pub fn stats() -> FxHashMap<&'static str, FxHashMap<&'static str, f64>> {
            PROFILER
                .lock()
                .iter()
                .map(|(k, (c, t))| {
                    let mut inner = FxHashMap::default();
                    inner.insert("count", *c as f64);
                    inner.insert("total_us", *t);
                    inner.insert("avg_us", if *c == 0 { 0.0 } else { *t / *c as f64 });
                    (*k, inner)
                })
                .collect()
        }
        pub fn prof(tag: &'static str, started: Instant) {
            let dt = started.elapsed().as_secs_f64() * 1e6;
            let mut map = PROFILER.lock();
            let e = map.entry(tag).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += dt;
        }
    }
}
#[cfg(feature = "profile")]
use profiler::Perf as PerformanceProfiler;
#[cfg(not(feature = "profile"))]
struct PerformanceProfiler;
#[cfg(not(feature = "profile"))]
impl PerformanceProfiler {
     #[inline(always)]
    fn measure<F: FnOnce()>(_name: &'static str, f: F) {
         f()
     }
     fn stats() -> FxHashMap<&'static str, FxHashMap<&'static str, f64>> {
         FxHashMap::default()
     }
     pub fn prof(_tag: &'static str, _started: Instant) {}
}

// ================================ SerialPort ===============================

#[derive(Debug)]
struct Packet {
    data: Vec<u8>,
    tag: &'static str,
    started: Instant,
    tracked_id: Option<u32>,
}

#[derive(Clone)]
struct TxHandle {
    queue: Sender<Packet>,
}
impl TxHandle {
    #[inline]
    fn send(&self, packet: Packet) {
        let _ = self.queue.try_send(packet);
    }
}

/// Reader/Writer wrapper
pub struct SerialPortWrap {
    port_name: String,
    baudrate: u32,
    timeout: Duration,

    ser: Option<Box<dyn SerialPort>>,
    stop: Arc<AtomicBool>,
    reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    tx: Option<TxHandle>,

    pending: Arc<Mutex<FxHashMap<u32, PendingCommand>>>,
    button_cb: Arc<RwLock<Option<Box<dyn Fn(MouseButtonStates) + Send + Sync>>>>,
    cmd_id: AtomicU32,
}

impl SerialPortWrap {
    pub fn new(port: impl Into<String>, baud: u32, timeout: Duration) -> Self {
        Self {
            port_name: port.into(),
            baudrate: baud,
            timeout,
            ser: None,
            stop: Arc::new(AtomicBool::new(false)),
            reader: None,
            writer: None,
            tx: None,
            pending: Arc::new(Mutex::new(FxHashMap::default())),
            button_cb: Arc::new(RwLock::new(None)),
            cmd_id: AtomicU32::new(0),
        }
    }

    pub fn open(&mut self) -> MakcuResult<()> {
        if self.is_open() {
            return Ok(());
        }
        let ser = serialport::new(&self.port_name, self.baudrate)
            .timeout(self.timeout)
            .open()
            .map_err(|e| MakcuError::Connection(e.to_string()))?;
        self.ser = Some(ser.into());
        self.stop.store(false, Ordering::Relaxed);

        // ---------- writer ----------
        let (tx, rx): (Sender<Packet>, Receiver<Packet>) = bounded(1024);
        let mut wport = self.ser.as_mut().unwrap().try_clone().unwrap();
        let stop_w = self.stop.clone();
        self.writer = Some(thread::spawn(move || {
            let mut buf = Vec::<u8>::with_capacity(4096);
            while !stop_w.load(Ordering::Relaxed) {
                    match rx.recv() {
                    Ok(first) => {
                        let current_tag = first.tag;
                        let started     = first.started;
                        buf.clear();
                        buf.extend_from_slice(&first.data);
                        while let Ok(pk) = rx.try_recv() {
                            buf.extend_from_slice(&pk.data);
                        }
                        if let Err(e) = wport.write_all(&buf) {
                            eprintln!("writer error: {e}");
                            break;
                        }
                        PerformanceProfiler::prof(current_tag, started);
                    }
                    Err(_) => break,
                }
            }
        }));
        self.tx = Some(TxHandle { queue: tx });

        // ---------- reader ----------
        let mut rport = self.ser.as_mut().unwrap().try_clone().unwrap();
        let stop_r = self.stop.clone();
        let pending_r = self.pending.clone();
        let button_cb_r = self.button_cb.clone();
        self.reader = Some(thread::spawn(move || {
            
            reader_loop(&mut *rport, stop_r, pending_r, button_cb_r);
        }));

        Ok(())
    }

    pub fn close(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        
        if let Some(ser) = &mut self.ser {
            let _ = ser.set_timeout(Duration::from_millis(20));
        }
        if let Some(h) = &self.tx {
             let _ = h.queue.try_send(Packet {
                 data: Vec::new(),
                 tag: "",
                 started: Instant::now(),
                 tracked_id: None,
             });
         }

        if let Some(h) = self.reader.take() { let _ = h.join(); }
        if let Some(h) = self.writer.take() { let _ = h.join(); }

        self.ser = None;

        
        for (_, pc) in self.pending.lock().drain() {
            let _ = pc.tx.send("Port closed".into());
        }
    }


    #[inline] pub fn is_open(&self) -> bool { self.ser.is_some() }

    #[inline] fn next_id(&self) -> u32 {
        self.cmd_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    fn enqueue_ff(&self, packet: Packet) -> MakcuResult<()> {
         self.tx
             .as_ref()
             .ok_or_else(|| MakcuError::Connection("Port not open".into()))
             .map(|h| h.send(packet))
    }

    pub fn send_ff(&self, payload: &str) -> MakcuResult<()> {
        if DEBUG_IO { println!(">> {payload}"); }
        let mut v = Vec::with_capacity(payload.len() + 2);
        v.extend_from_slice(payload.as_bytes());
        v.extend_from_slice(CRLF.as_bytes());
        {
            let mut log = SENT_LOG.lock();
            if log.len() == LOG_MAX { log.pop_front(); }
            log.push_back(payload.as_bytes().to_vec());
            
        }
        self.enqueue_ff(Packet{
             data: v,
             tag: "move",           
             started: Instant::now(),
             tracked_id: None,
        })
    }

    pub fn send_tracked(&self, payload: &str, timeout_s: f32) -> MakcuResult<String> {
        let started = Instant::now();
        let tag = "serial";
        {                           
            let mut log = SENT_LOG.lock();
            if log.len()==LOG_MAX { log.pop_front(); }
            log.push_back(payload.as_bytes().to_vec());
        }
        let cid = self.next_id();
        let (tx, rx) = bounded::<String>(1);
        self.pending.lock().insert(cid, PendingCommand { tx, tag, started });

        let mut v = Vec::with_capacity(payload.len() + 16);
        v.extend_from_slice(payload.as_bytes());
        v.extend_from_slice(format!("#{cid}{CRLF}").as_bytes());
        self.enqueue_ff(Packet { data: v, tag, started, tracked_id: Some(cid) })?;

        match rx.recv_timeout(Duration::from_secs_f32(timeout_s)) {
            Ok(resp) => Ok(resp),
            Err(_) => {
                self.pending.lock().remove(&cid);
                Err(MakcuError::Timeout(cid))
            }
        }
    }

    pub fn set_button_callback<F>(&self, cb: Option<F>)
    where
        F: Fn(MouseButtonStates) + Send + Sync + 'static,
    {
        *self.button_cb.write() = cb.map(|f| Box::new(f) as _);
    }
}

// --------------------------- Reader loop -----------------------------------

fn reader_loop(
    ser: &mut dyn SerialPort,
    stop: Arc<AtomicBool>,
    pending: Arc<Mutex<FxHashMap<u32, PendingCommand>>>,
    button_cb: Arc<RwLock<Option<Box<dyn Fn(MouseButtonStates) + Send + Sync>>>>,
) {
    const BUF_TMP: usize = 512;
    let mut buf = BytesMut::with_capacity(2048);
    let mut tmp = [0u8; BUF_TMP];

    while !stop.load(Ordering::Relaxed) {
        match ser.read(&mut tmp) {
            Ok(0) => continue,
            Ok(n) => {
                
                if n == 1 && tmp[0] < 32 && tmp[0] != b'\r' && tmp[0] != b'\n' {
                    if let Some(cb) = &*button_cb.read() {
                        cb(MouseButtonStates::from_mask(tmp[0]));
                    }
                    continue;
                }
                buf.extend_from_slice(&tmp[..n]);
                while let Some(pos) = memchr(b'\n', &buf) {
                    let mut line = buf.split_to(pos + 1);
                    if line.ends_with(b"\n") { line.truncate(line.len() - 1); }
                    if line.ends_with(b"\r") { line.truncate(line.len() - 1); }
                    if !line.is_empty() {
                        handle_line_bytes(&line, &pending);
                    }
                }
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[inline]
fn handle_line_bytes(line: &[u8], pending: &Mutex<FxHashMap<u32, PendingCommand>>) {
    if DEBUG_IO {
        if let Ok(s) = std::str::from_utf8(line) { println!("<< {s}"); }
    }

    if let Some(hash) = memchr(b'#', line) {
        if let Some(colon) = memchr2(b':', b'\r', &line[hash + 1..]) {
            if let Some(cid) = std::str::from_utf8(&line[hash + 1..hash + 1 + colon])
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
            {
                if let Some(pc) = pending.lock().remove(&cid) {
                    let dt = pc.started.elapsed();
                    PerformanceProfiler::prof(pc.tag, pc.started); 
                    let payload =
                        String::from_utf8_lossy(&line[hash + 1 + colon + 1..]).into_owned();
                    let _ = pc.tx.send(payload);
                }
                return;
            }
        }
    }

   
    if pending.lock().is_empty() {
        return;                      
    }

    
    let payload = if line.starts_with(b">>>") {
        let mut p = &line[3..];
        if p.first() == Some(&b' ') { p = &p[1..]; }
        p
    } else {
        line
    };

    if SENT_LOG.lock().iter().any(|cmd| cmd.as_slice()==payload) { return; }

    let mut pend = pending.lock();
    if let Some((&cid, _)) = pend.iter().next() {
        if let Some(pc) = pend.remove(&cid) {
            let _ = pc.tx.send(String::from_utf8_lossy(payload).into_owned());
        }
    }
}

pub struct Batch<'a> {
    dev: &'a Device,
    buf: String,
}
impl<'a> Batch<'a> {
    #[inline(always)]
    fn mark(op: &'static str) {
        PerformanceProfiler::measure(op, || {})
    }

    #[inline(always)]
   fn btn_name(btn: MouseButton) -> &'static str {
       match btn {
           MouseButton::Left   => "left",
           MouseButton::Right  => "right",
           MouseButton::Middle => "middle",
           MouseButton::Side1  => "ms1",
           MouseButton::Side2  => "ms2",
       }
   }

    pub fn move_rel(mut self, dx:i32, dy:i32) -> Self {
        Self::mark("move");
        use std::fmt::Write; let _ = write!(self.buf,"km.move({dx},{dy}){CRLF}");
        self
    }
    pub fn click(mut self, btn:MouseButton) -> Self {
        Self::mark("click");
        use std::fmt::Write;
       let _ = write!(self.buf, "km.{}(){CRLF}", Self::btn_name(btn));
       self
    }
    pub fn press(mut self, btn: MouseButton) -> Self {
       Self::mark("press");
       use std::fmt::Write;
       let _ = write!(self.buf, "km.{}(1){CRLF}", Self::btn_name(btn));
       self
   }

    pub fn release(mut self, btn: MouseButton) -> Self {
       Self::mark("release");
       use std::fmt::Write;
       let _ = write!(self.buf, "km.{}(0){CRLF}", Self::btn_name(btn));
        self.buf.push_str(CRLF);
        self
    }
    pub fn wheel(mut self, d:i32)->Self {
        Self::mark("wheel");
        use std::fmt::Write;  let _ = write!(self.buf, "km.wheel({d}){CRLF}");
        self
    }
    pub fn run(self) -> MakcuResult<()> {           
        Ok(PerformanceProfiler::measure("batch", || {
           
            let _ = self.dev.send_ff(&self.buf);
        }))
    }
}


#[cfg(not(feature = "async"))]
pub struct DeviceAsync; 

#[cfg(not(feature = "async"))]
impl DeviceAsync {
    pub async fn new(_: &str, _: u32) -> std::io::Result<Self> {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "async not enabled"))
    }
}

#[cfg(feature = "async")]
pub struct DeviceAsync {
    inner: tokio::sync::Mutex<tokio_serial::SerialStream>,
}

#[cfg(feature = "async")]
impl DeviceAsync {
    /* ---------- конструктор ---------- */
    pub async fn new(port: &str, baud: u32) -> std::io::Result<Self> {
        use tokio_serial::SerialPortBuilderExt;
        let stream = tokio_serial::new(port, baud).open_native_async()?;
        Ok(Self { inner: tokio::sync::Mutex::new(stream) })
    }

    /* ---------- низкоуровневый raw‑write ---------- */
    #[inline(always)]
    async fn send_raw(&self, txt: &str) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut guard = self.inner.lock().await;
        AsyncWriteExt::write_all(&mut *guard, txt.as_bytes()).await
    }

    /* --- общие вспомогательные штуки --- */
    #[inline(always)]
    fn btn_name(btn: MouseButton) -> &'static str {
        match btn {
            MouseButton::Left  => "left",
            MouseButton::Right => "right",
            MouseButton::Middle=> "middle",
            MouseButton::Side1 => "ms1",
            MouseButton::Side2 => "ms2",
        }
    }

    /* ---------- отдельные операции ---------- */
    pub async fn move_rel(&self, dx: i32, dy: i32) -> std::io::Result<()> {
        PerformanceProfiler::measure("move_async", || {});
        self.send_raw(&format!("km.move({dx},{dy}){CRLF}")).await
    }

    pub async fn wheel(&self, delta: i32) -> std::io::Result<()> {
        PerformanceProfiler::measure("wheel_async", || {});
        self.send_raw(&format!("km.wheel({delta}){CRLF}")).await
    }

    pub async fn press(&self, btn: MouseButton) -> std::io::Result<()> {
        PerformanceProfiler::measure("press_async", || {});
        self.send_raw(&format!("km.{}(1){CRLF}", Self::btn_name(btn))).await
    }

    pub async fn release(&self, btn: MouseButton) -> std::io::Result<()> {
        PerformanceProfiler::measure("release_async", || {});
        self.send_raw(&format!("km.{}(0){CRLF}", Self::btn_name(btn))).await
    }

    pub async fn click(&self, btn: MouseButton) -> std::io::Result<()> {
        PerformanceProfiler::measure("click_async", || {});
        self.press(btn).await?;
        self.release(btn).await
    }

    /* ---------- пакетная отправка ---------- */
    pub fn batch(&self) -> AsyncBatch<'_> {
        AsyncBatch { dev: self, buf: String::new() }
    }
}

/* ---------------- builder для async batch ---------------- */

#[cfg(feature = "async")]
pub struct AsyncBatch<'a> {
    dev: &'a DeviceAsync,
    buf: String,
}

#[cfg(feature = "async")]
impl<'a> AsyncBatch<'a> {
    #[inline(always)] fn mark(op: &'static str){ PerformanceProfiler::measure(op, ||{}) }
    #[inline(always)] fn btn(btn: MouseButton)->&'static str{ DeviceAsync::btn_name(btn) }

    pub fn move_rel(mut self, dx:i32, dy:i32)->Self{
        Self::mark("move_async");
        use core::fmt::Write; let _=write!(self.buf,"km.move({dx},{dy}){CRLF}");
        self
    }
    pub fn wheel  (mut self, d:i32)->Self{
        Self::mark("wheel_async");
        use core::fmt::Write; let _=write!(self.buf,"km.wheel({d}){CRLF}");
        self
    }
    pub fn press  (mut self, b:MouseButton)->Self{
        Self::mark("press_async");
        self.buf.push_str(&format!("km.{}(1){CRLF}", Self::btn(b))); self
    }
    pub fn release(mut self, b:MouseButton)->Self{
        Self::mark("release_async");
        self.buf.push_str(&format!("km.{}(0){CRLF}", Self::btn(b))); self
    }
    pub fn click  (mut self, b:MouseButton)->Self{
        Self::mark("click_async");
        self.buf.push_str(&format!("km.{}(){CRLF}",   Self::btn(b))); self
    }

    pub async fn run(self) -> std::io::Result<()> {
        PerformanceProfiler::measure("batch_async", ||{});
        self.dev.send_raw(&self.buf).await
    }
}


// ================================ Device ===================================

#[derive(Clone)]
pub struct Device {
    sp: Arc<Mutex<SerialPortWrap>>,
    connected: Arc<AtomicBool>,
    lock_valid: Arc<AtomicBool>,
    btn_cache: Arc<Mutex<MouseButtonStates>>,
}

#[derive(Clone, Copy)]
pub enum MouseButton { Left, Right, Middle, Side1, Side2 }

impl Device {
    pub fn batch(&self)->Batch { Batch { dev:self, buf:String::new() } }
    pub fn new(port: impl Into<String>, baud: u32, timeout: Duration) -> Self {
        Self {
            
            sp: Arc::new(Mutex::new(SerialPortWrap::new(port, baud, timeout))),
            connected: Arc::new(AtomicBool::new(false)),
            lock_valid: Arc::new(AtomicBool::new(false)),
            btn_cache: Arc::new(Mutex::new(MouseButtonStates::default())),
        }
    }

    pub fn connect(&self) -> MakcuResult<()> {
        PerformanceProfiler::measure("connect", || {
            self.sp.lock().open().unwrap();
        });
        self.connected.store(true, Ordering::Release);

        let cache = self.btn_cache.clone();
        self.sp.lock().set_button_callback(Some(move |st| *cache.lock() = st));
        self.send_ff("km.buttons(1)")
    }

    pub fn disconnect(&self) {
        PerformanceProfiler::measure("disconnect", || self.sp.lock().close());
        self.connected.store(false, Ordering::Release);
        self.lock_valid.store(false, Ordering::Release);
    }

    /* ---- построение команд для кнопок ---- */
    fn btn_cmd(&self, btn: MouseButton, down: bool) -> MakcuResult<()> {
        PerformanceProfiler::measure("click", || {
            let suffix = if down { "(1)" } else { "(0)" };
            let cmd = match btn {
                MouseButton::Left   => "km.left",
                MouseButton::Right  => "km.right",
                MouseButton::Middle => "km.middle",
                MouseButton::Side1  => "km.ms1",
                MouseButton::Side2  => "km.ms2",
            };
            let _ = self.send_ff(&format!("{cmd}{suffix}"));
            });
        Ok(())
    }
    

    /* ---- lock‑helpers ---- */
    fn send_lock(&self, short: &str, lock: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.lock_{short}({})", if lock {1} else {0}))
    }


    pub fn set_button_callback<F>(&self, cb: Option<F>)
    where
        F: Fn(MouseButtonStates) + Send + Sync + 'static,
    {
        let cache = self.btn_cache.clone();
        let sp = self.sp.lock();
        if let Some(user_cb) = cb {
            sp.set_button_callback(Some(move |st| {
                *cache.lock() = st;     
                user_cb(st);            
            }));
        } else {
            sp.set_button_callback(None::<fn(MouseButtonStates)>);
        }
    }

    #[inline] fn ensure(&self) -> MakcuResult<()> {
        if self.connected.load(Ordering::Acquire) && self.sp.lock().is_open() {
            Ok(())
        } else {
            Err(MakcuError::Connection("Not connected".into()))
        }
    }

    #[inline] fn send_ff(&self, cmd: &str) -> MakcuResult<()> {
        self.ensure()?;
        self.sp.lock().send_ff(cmd)
    }
    #[inline] fn send_tr(&self, cmd: &str, t: f32) -> MakcuResult<String> {
        self.ensure()?;
        self.sp.lock().send_tracked(cmd, t)
    }

    


    // ====== API ======
    pub fn move_rel(&self, dx: i32, dy: i32) -> MakcuResult<()> {
        PerformanceProfiler::measure("move", || {
            let _ = self.send_ff(&format!("km.move({dx},{dy})"));
        });
        Ok(())
    }
    #[inline] fn btn(&self, name: &str, down: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.{name}({})", if down { 1 } else { 0 }))
    }

    pub fn press_left(&self) -> MakcuResult<()> { self.btn("left", true) }
    pub fn release_left(&self) -> MakcuResult<()> { self.btn("left", false) }
    pub fn press_right(&self) -> MakcuResult<()> { self.btn("right", true) }
    pub fn release_right(&self) -> MakcuResult<()> { self.btn("right", false) }
    pub fn press_middle(&self) -> MakcuResult<()> { self.btn("middle", true) }
    pub fn release_middle(&self) -> MakcuResult<()> { self.btn("middle", false) }

    pub fn click_left(&self) -> MakcuResult<()> { self.press_left()?; self.release_left() }
    pub fn click_right(&self) -> MakcuResult<()> { self.press_right()?; self.release_right() }

    pub fn press(&self, btn: MouseButton)  -> MakcuResult<()> { self.btn_cmd(btn, true)  }
    pub fn release(&self, btn: MouseButton)-> MakcuResult<()> { self.btn_cmd(btn, false) }
    pub fn click(&self, btn: MouseButton)  -> MakcuResult<()> { self.press(btn)?; self.release(btn) }

    /* --------- колёсико -------- */
    pub fn wheel(&self, delta: i32) -> MakcuResult<()> {
        PerformanceProfiler::measure("wheel", ||
            { self.send_ff(&format!("km.wheel({delta})")).ok(); }
        );
        Ok(())
    }

    /* --------- блокировки ------ */
    pub fn lock_mouse_x(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mx", lock) }
    pub fn lock_mouse_y(&self, lock: bool) -> MakcuResult<()> { self.send_lock("my", lock) }
    pub fn lock_left     (&self, lock: bool) -> MakcuResult<()> { self.send_lock("ml", lock) }
    pub fn lock_right    (&self, lock: bool) -> MakcuResult<()> { self.send_lock("mr", lock) }
    pub fn lock_middle   (&self, lock: bool) -> MakcuResult<()> { self.send_lock("mm", lock) }
    pub fn lock_side1    (&self, lock: bool) -> MakcuResult<()> { self.send_lock("ms1", lock) }
    pub fn lock_side2    (&self, lock: bool) -> MakcuResult<()> { self.send_lock("ms2", lock) }

    pub fn set_serial(&self, v: &str) -> MakcuResult<String> {
        let arg = if v.is_empty() { "0".into() }
                  else { format!("'{}'", v.replace('\'', "\\'")) };
        self.send_tr(&format!("km.serial({arg})"), 1.0)
    }

    pub fn profiler_stats() -> FxHashMap<&'static str, FxHashMap<&'static str, f64>> {
        PerformanceProfiler::stats()
    }

}
// ============================ MockSerial (optional) ========================
#[cfg(feature = "mockserial")]
pub mod mockserial {
    use super::*;
    use std::io::{Read, Write};

    pub struct MockSerial {
        timeout: Duration,
        in_q: Mutex<std::collections::VecDeque<Vec<u8>>>,
        out_q: Mutex<std::collections::VecDeque<Vec<u8>>>,
        last_btn: Mutex<Instant>,
    }
    impl MockSerial {
        pub fn new(timeout: Duration) -> Self {
            Self {
                timeout,
                in_q: Mutex::default(),
                out_q: Mutex::default(),
                last_btn: Mutex::new(Instant::now()),
            }
        }
    }
    impl Read for MockSerial {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let now = Instant::now();
            if now.duration_since(*self.last_btn.lock()) > Duration::from_millis(500) {
                *self.last_btn.lock() = now;
                self.in_q.lock().push_back(vec![0x01]);
            }
            if let Some(chunk) = self.in_q.lock().pop_front() {
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                return Ok(n);
            }
            thread::sleep(self.timeout);
            Ok(0)
        }
    }
    impl Write for MockSerial {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.out_q.lock().push_back(buf.to_vec());
            let txt = String::from_utf8_lossy(buf);
            if let Some(idx) = txt.rfind('#') {
                let sid = &txt[idx + 1..].trim_end();
                let resp = format!(">>> OK#{sid}:OK{CRLF}").into_bytes();
                self.in_q.lock().push_back(resp);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
}
