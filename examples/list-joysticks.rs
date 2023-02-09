use anyhow::Result;
use tracing::{info, warn};

use joystick_rs::{
    driver::{
        rawinput::{Config, DeviceType},
        Event, HatState, Manager,
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

    let mut btn_states = vec![];
    let mut hat_state = HatState::default();

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
                if states.buttons != btn_states {
                    info!(prev = ?btn_states, next = ?states.buttons, "button states changed");
                    btn_states = states.buttons;
                }

                if let Some(st) = states.hats.first() {
                    if st != &hat_state {
                        info!(prev = ?hat_state, next = ?st, "hat state changed");
                        hat_state = *st;
                    }
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
