use anyhow::Result;

use joystick_rs::{
    driver::{
        rawinput::{Config, DeviceType},
        Manager,
    },
    logging::init_from_env,
};

pub fn main() -> Result<()> {
    init_from_env()?;

    let cfg = Config {
        dev_type: DeviceType::Both,
        ..Default::default()
    };

    let mgr = cfg.start()?;
    println!("devices constructed");

    let rx = mgr.as_event_receiver();

    for _ in 0..5 {
        println!("waiting for incoming msg:");
        let evt = rx.recv()??;
        println!("get event: {:?}", evt);
    }
    println!("done");

    Ok(())
}
