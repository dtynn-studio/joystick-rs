use joystick_rs::{driver::rawinput::Manager, logging::init_from_env, Result};

pub fn main() -> Result<()> {
    init_from_env()?;

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
