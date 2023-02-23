pub mod driver;
pub mod logging;
pub mod profile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DPadState {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Start,
    Select,
    Mode,
    LThumb,
    RThumb,
    LShoulder,
    RShoulder,
    LTrigger,
    RTrigger,
    North,
    South,
    East,
    West,
    Other(&'static str),
}

pub type ButtonIdent = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Pressed,
    Released,
}

impl From<bool> for ButtonState {
    fn from(v: bool) -> Self {
        if v {
            Self::Pressed
        } else {
            Self::Released
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    LThumbX,
    LThumbY,
    RThumbX,
    RThumbY,
    LTrigger,
    RTrigger,
    Other(&'static str),
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisIdent {
    X = 0,
    Y = 1,
    Z = 2,
    RX = 3,
    RY = 4,
    RZ = 5,
    Limit = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisDef {
    pub typ: Axis,
    pub centered: bool,
    // TODO: more definitions
}

impl From<usize> for AxisIdent {
    fn from(v: usize) -> Self {
        match v {
            0 => Self::X,
            1 => Self::Y,
            2 => Self::Z,
            3 => Self::RX,
            4 => Self::RY,
            5 => Self::RZ,
            _ => unreachable!("invalid axis ident {}", v),
        }
    }
}

pub type AxisState = i32;

pub type SliderState = i32;

#[derive(Debug)]
pub enum ObjectDiff {
    DPad(DPadState),
    Button(Button, ButtonState),
    Axis(Axis, AxisState),
    Slider(SliderState),
}

pub trait Joystick<const BTN_NUM: usize> {
    const DPAD: bool;
    const BUTTONS: [Button; BTN_NUM];
    const AXIS: [Option<AxisDef>; AxisIdent::Limit as usize];
}
