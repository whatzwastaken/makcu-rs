# makcu-rs

Made by t.me/whatzwasthere

> **Fast and modular communication interface over serial ports. Built with performance and async-first in mind.**

[![Crates.io](https://img.shields.io/crates/v/makcu-rs)](https://crates.io/crates/makcu-rs)
[![Docs.rs](https://docs.rs/makcu-rs/badge.svg)](https://docs.rs/makcu-rs)
[![License](https://img.shields.io/crates/l/makcu-rs)](./LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/whatzwastaken/makcu-rs/ci.yml)](https://github.com/whatzwastaken/makcu-rs/actions)

---
# FASTEST EVER MAKCU LIB
## 🚦 Performance Comparison

| Operation         | makcu-py-lib v2.0 | makcu-cpp | makcu-rs (avg_us) | Evaluation        |
|------------------|-------------------|---------------------|----------------------|--------------------|
| Mouse Move       | 2000 µs           | 70 µs               | 0.0 µs               | ✅ Best            |
| Click            | 1000–2000 µs      | 160 µs              | 0.1 µs               | ✅ Best            |
| Press / Release  | ~1000 µs          | 55–100 µs           | 0.1 µs / 0.0 µs      | ✅ Best            |
| Wheel Scroll     | 1000–2000 µs      | 48 µs               | 0.0 µs               | ✅ Best            |
| Batch (9 ops)    | 3000 µs           | <100 µs             | 0.5 µs               | ✅ Best            |
| Async (5 ops)    | 2000 µs           | 200 µs              | ❌ No data           | ❓ Needs testing   |
| Connect          | ~800 µs           | —                   | 860.3 µs             | ⚠️ Acceptable      |



## Features

- ✅ Fast and lightweight sync/async communication via `serialport` and `tokio-serial`
- 🧠 Built-in **command tracking** with optional response timeout
- 🔧 Mouse movement, button presses, scroll wheel, and lock state support
- 🧪 Optional mock serial interface for testing (`mockserial` feature)
- 📈 Optional built-in performance profiler (`profile` feature)
- 🧵 Thread-safe, lock-free write queue (via `crossbeam-channel`)
- 🔋 Minimal dependencies & portable

---

## 📦 Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
makcu-rs = "0.1"
```

Or with optional features:

```toml
makcu-rs = { version = "0.1", features = ["async", "mockserial", "profile"] }
```

---

## 🚀 Quick Start

### Sync (threaded) example

```rust
use makcu_rs::{Device, MouseButton};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let dev = Device::new("/dev/ttyUSB0", 115200, Duration::from_millis(100));
    dev.connect()?;

    dev.move_rel(100, 0)?;
    dev.click(MouseButton::Left)?;
    dev.wheel(-2)?;

    dev.disconnect();
    Ok(())
}
```

### Async version

```rust
#[tokio::main]
async fn main() -> std::io::Result<()> {
    use makcu_rs::{DeviceAsync, MouseButton};

    let dev = DeviceAsync::new("/dev/ttyUSB0", 115200).await?;

    dev.move_rel(50, 0).await?;
    dev.click(MouseButton::Right).await?;
    dev.wheel(1).await?;

    Ok(())
}
```

---

## 🧰 Features

| Feature       | Description                               |
|---------------|-------------------------------------------|
| `async`       | Enables `tokio`-based async interface     |
| `profile`     | Built-in performance statistics collection |
| `mockserial`  | Fake serial port for unit testing         |

Enable with:

```bash
cargo build --features "profile mockserial"
```

---

## 🔎 Tracked Commands

Send a command and wait for a response with a timeout:

```rust
let response = dev.set_serial("ABC123")?;
println!("Response: {response}");
```

---

## 🔄 Batch Commands

```rust
dev.batch()
    .move_rel(100, 20)
    .click(MouseButton::Left)
    .wheel(-1)
    .run()?;
```

---

## 📊 Profiler (Optional)

If the `profile` feature is enabled, collect per-operation timing:

```rust
let stats = makcu_rs::Device::profiler_stats();
dbg!(stats);
```

---

## 🧪 Testing with Mock Serial

To simulate serial behavior during development:

```toml
[features]
mockserial = []
```

---

## 📚 Documentation

- [API Docs on docs.rs](https://docs.rs/makcu-rs)
- [Repository on GitHub](https://github.com/whatzwastaken/makcu-rs)

---

## 🪪 License

Licensed under either of:

- MIT License
- Apache License 2.0

---

# 🇷🇺 makcu-rs

> **Быстрый и модульный интерфейс работы с COM-портами. С поддержкой трекинга, асинхронности и пакетных команд.**

---

## Основные возможности

- ✨ Поддержка как синхронного, так и асинхронного (`tokio`) режима
- ⏱ Отправка команд с ожиданием ответа (с таймаутом)
- 🔘 Управление движением мыши, кнопками, прокруткой
- 📦 Пакетная отправка команд (batch API)
- 🧪 Тестируемый `mockserial` режим
- 🔍 Встроенный профайлер (опционально)

---

## Быстрый пример (синхронный)

```rust
let dev = Device::new("COM3", 115200, Duration::from_millis(100));
dev.connect()?;
dev.click(MouseButton::Left)?;
dev.disconnect();
```

## Быстрый пример (асинхронный)

```rust
let dev = DeviceAsync::new("COM3", 115200).await?;
dev.press(MouseButton::Middle).await?;
```

---

## Лицензия

Проект распространяется под MIT или Apache 2.0 лицензией на ваш выбор.

---

## 🤝 Поддержка

Создавайте [issue на GitHub](https://github.com/whatzwastaken/makcu-rs/issues) или пишите автору: [@whatzwasthere (TG/GitHub)](https://github.com/whatzwastaken)

---

## 📦 Cargo Features Summary

| Feature       | Описание                                |
|---------------|------------------------------------------|
| `async`       | Включает `tokio`-совместимый API         |
| `profile`     | Включает встроенный профайлер операций   |
| `mockserial`  | Фейковый COM-порт (для тестирования)     |

---

💡 *makcu-rs* помогает удобно управлять внешними устройствами (мыши, трекпады, манипуляторы) через COM-порт, минимизируя задержки и повышая надёжность взаимодействия.
