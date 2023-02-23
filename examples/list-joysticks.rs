use anyhow::{Context, Result};
use tracing::{info, warn};

use joystick_rs::{
    driver::{rawinput::RawInput, Driver, Event},
    logging::init_from_env,
    profile::PS4Compact,
    ObjectDiff,
};

pub fn main() -> Result<()> {
    init_from_env().context("init logging")?;

    let hdl = RawInput::background().context("init rawinput in background")?;

    let rx = hdl.as_event_receiver();
    // let mut state_count = 0;

    loop {
        let evt = rx.recv()?;
        match evt {
            Event::Attached(id, info) => {
                info!("device {} attached: {:?}", id, info);
            }

            Event::Deattached(id) => {
                info!("device {} deattached", id);
                break;
            }

            Event::StateDiff { id, is_sink, diff } => {
                let obj_diffs = diff.diffs(&PS4Compact);
                // state_count += obj_diffs.len();

                for odiff in obj_diffs {
                    match odiff {
                        ObjectDiff::Button(bid, bst) => {
                            info!(dev = id, is_sink, "button state ({:?}, {:?})", bid, bst);
                        }

                        ObjectDiff::Axis(aid, ast) => {
                            if ast == 0 || ast == 255 {
                                info!(dev = id, is_sink, "axis edge state ({:?}, {:?})", aid, ast);
                            }
                        }

                        ObjectDiff::DPad(dst) => {
                            info!(dev = id, is_sink, "dpad state {:?}", dst);
                        }

                        _ => {}
                    }
                }

                // if state_count >= 1000 {
                //     info!("a lot of state diffs");
                //     break;
                // }
            }

            Event::Warn(e) => {
                warn!("err received: {:?}", e);
                break;
            }

            Event::Interruption(res) => {
                warn!("interrupted: {:?}", res);
                break;
            }
        }
    }

    hdl.close();

    info!("done");

    Ok(())
}
