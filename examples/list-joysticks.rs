use anyhow::Result;
use tracing::info;

use joystick_rs::{
    driver::{
        rawinput::{Config, DeviceType},
        Event, Manager,
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
    info!("devices constructed");

    let rx = mgr.as_event_receiver();

    while let Ok(evt) = rx.recv()? {
        match evt {
            Event::DeviceAttached(id, info) => {
                info!("device {} attached: {:?}", id, info);
            }

            Event::DeviceDeattached(id) => {
                info!("device {} deattached", id);
                break;
            }

            _ => {}
        }
    }

    _ = mgr.close();

    println!("done");

    Ok(())
}
