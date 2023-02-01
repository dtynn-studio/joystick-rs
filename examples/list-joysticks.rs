use joystick_rs::{driver::rawinput::Manager, Result};

pub fn main() -> Result<()> {
    let mut mgr = Manager::new(false)?;
    let devices = mgr.list_devices()?;

    for dev in devices {
        println!("device: {:?}", dev);
    }

    println!("done");

    Ok(())
}
