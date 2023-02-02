use joystick_rs::{driver::rawinput::Manager, Result};

pub fn main() -> Result<()> {
    let mut mgr = Manager::new(true)?;
    println!("devices constructed");
    let devices = mgr.list_devices()?;

    for dev in devices {
        println!("device: {:?}", dev);
    }

    for _ in 0..5 {
        println!("waiting for incoming msg:");
        mgr.on_message()?;
    }
    println!("done");

    Ok(())
}
