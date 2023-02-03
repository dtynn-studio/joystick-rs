use anyhow::Result;
use crossbeam_channel::Receiver;

pub mod rawinput;

pub trait Manager {
    type DevIdent;
    fn as_event_receiver(&self) -> &Receiver<Result<Event<Self::DevIdent>>>;
}

#[derive(Debug)]
pub enum Event<DI> {
    DeviceAttached(DI, DeviceInfo),
    DeviceDeattached(DI),
}

#[derive(Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub objects: DeviceObjects,
}

#[derive(Debug)]
pub struct DeviceObjects {}

#[repr(u8)]
#[derive(Debug)]
pub enum ButtonState {
    Pressed = 1,
    Releaed = 0,
}

#[derive(Debug)]
pub enum HatState {
    Null,
    Up,
    Down,
    Left,
    Right,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

pub type AxisState = f64;

pub type SliderState = f64;
