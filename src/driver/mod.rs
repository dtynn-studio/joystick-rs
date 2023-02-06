use std::ops::RangeBounds;

use anyhow::Result;
use crossbeam_channel::Receiver;

pub mod rawinput;

pub trait Manager {
    type DeviceIdent;
    type Value;
    type ValueRange: RangeBounds<Self::Value>;

    fn as_event_receiver(&self) -> &Receiver<Result<Event<Self::DeviceIdent, Self::ValueRange>>>;
}

#[derive(Debug)]
pub enum Event<DI, VR> {
    DeviceAttached(DI, DeviceInfo<VR>),
    DeviceDeattached(DI),
}

#[derive(Debug)]
pub struct DeviceInfo<VR> {
    pub name: String,
    pub specs: DeviceSpecs<VR>,
}

#[derive(Debug, Default)]
pub struct DeviceSpecs<VR> {
    pub button_count: usize,
    pub axis: Vec<(AxisType, VR)>,
    pub sliders: Vec<VR>,
    pub hats_count: usize,
}

#[derive(Debug)]
pub struct DeviceObjectStates<V> {
    pub buttons: Vec<ButtonState>,
    pub axis: Vec<V>,
    pub sliders: Vec<V>,
    pub hats: Vec<HatState>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Clone, Copy)]
pub enum AxisType {
    X,
    Y,
    Z,
    RX,
    RY,
    RZ,
}

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
