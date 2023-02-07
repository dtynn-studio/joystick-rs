use anyhow::Result;
use tracing::{info, warn};

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
    let mut state_count = 0;

    while let Ok(evt) = rx.recv()? {
        match evt {
            Event::DeviceAttached(id, info) => {
                info!("device {} attached: {:?}", id, info);
            }

            Event::DeviceDeattached(id) => {
                info!("device {} deattached", id);
                break;
            }

            Event::DeviceState {
                ident,
                is_sink,
                states,
            } => {
                if state_count % 100 == 0 {
                    info!(
                        ident,
                        is_sink,
                        ?states,
                        received = state_count,
                        "state event"
                    );
                }

                state_count += 1;
            }

            Event::Warning(e) | Event::Interuption(e) => {
                warn!("err received: {:?}", e);
                break;
            }
        }
    }

    _ = mgr.close();

    info!("done");

    Ok(())
}
