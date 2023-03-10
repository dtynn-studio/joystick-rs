use std::thread::{spawn, JoinHandle};

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, unbounded, Receiver};
use tracing::{debug, warn, warn_span};
use windows::Win32::Foundation::HWND;

use crate::driver::{Driver, Event};

mod api;

type ButtonBits = u32;

pub struct RawInput {
    ctx: Option<(HWND, JoinHandle<()>)>,
    event_rx: Receiver<Event<isize, u32>>,
}

impl RawInput {
    /// init a RawInput instance with a background hwnd to receive joystick events
    pub fn background() -> Result<Self> {
        let (event_tx, event_rx) = unbounded();
        let (hwnd_tx, hwnd_rx) = bounded(0);
        let join = spawn(move || {
            let hwnd = match unsafe { api::setup_message_window() } {
                Ok(h) => {
                    _ = hwnd_tx.send(Ok(h));
                    h
                }
                err @ Err(_) => {
                    _ = hwnd_tx.send(err);
                    return;
                }
            };

            let _span = warn_span!("message loop", ?hwnd).entered();
            debug!("start");
            let res = unsafe { api::start_message_loop(hwnd, &event_tx) };
            if let Err(e) = res.as_ref() {
                warn!("fail: {:?}", e);
            }

            _ = event_tx.send(Event::Interruption(res));
            debug!("stop");
        });

        let hwnd = hwnd_rx
            .recv()
            .context("hwnd chan broken")?
            .context("setup message window")?;

        Ok(Self {
            ctx: Some((hwnd, join)),
            event_rx,
        })
    }

    fn cleanup(&mut self) {
        if let Some((hwnd, join)) = self.ctx.take() {
            if let Err(e) = unsafe { api::close_message_window(hwnd) } {
                warn!("close message window: {:?}", e);
            }
            debug!("cleaned up");

            _ = join.join();
            debug!("thread joined");
        }
    }
}

impl Drop for RawInput {
    fn drop(&mut self) {
        self.cleanup();
    }
}

impl Driver for RawInput {
    type DeviceIdent = isize;
    type ButtonBits = ButtonBits;

    fn as_event_receiver(&self) -> &Receiver<Event<Self::DeviceIdent, Self::ButtonBits>> {
        &self.event_rx
    }

    fn close(mut self) {
        self.cleanup();
    }
}
