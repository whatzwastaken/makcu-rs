//! Demonstrates basic usage of the `makcu-rs` crate.

use makcu_rs::Device;
use std::time::Duration;


fn main() -> anyhow::Result<()> {

    let dev = Device::new("COM3", 115_200, Duration::from_millis(10));

    dev.connect()?;


    dev.set_button_callback(Some(|st| {
        println!("Buttons: {:?}", st);
    }));

 
    dev.batch()
            .move_rel(100,50)
            .click(makcu_rs::MouseButton::Left)
            .wheel(-120)
            .press(makcu_rs::MouseButton::Right)
            .release(makcu_rs::MouseButton::Right)
            .run()?;

    


  
    println!("Profiler stats: {:?}", Device::profiler_stats());

    // dev.disconnect();
    Ok(())
}




// #[tokio::main]
// async fn main() -> anyhow::Result<()> {
//     use makcu_rs::{DeviceAsync, MouseButton};

//     let dev = DeviceAsync::new("COM3", 115_200).await?;

    
//     dev.move_rel(100, 0).await?;
//     dev.click(MouseButton::Left).await?;

    
//     dev.batch()
//         .press(MouseButton::Left)
//         .move_rel(50, 0)
//         .release(MouseButton::Left)
//         .wheel(-120)
//         .run()
//         .await?;

//     println!("{:#?}", makcu_rs::Device::profiler_stats());
//     Ok(())
// }
