use std::fmt::Debug;

use anyhow::{Error, Result};
use crossbeam_channel::Receiver;

use crate::{AxisIdent, AxisState, DPadState, Joystick, ObjectDiff, SliderState};

mod bits;
pub mod rawinput;

pub use bits::*;

pub struct StateDiff<B: Bits> {
    dpad: Option<DPadState>,
    buttons: (B, B),
    axis: [Option<AxisState>; AxisIdent::Limit as usize],
    slider: Option<SliderState>,
}

impl<B: Bits> StateDiff<B> {
    pub fn diffs<const BN: usize, J: Joystick<BN>>(&self) -> Vec<ObjectDiff> {
        let axis_count = self.axis.iter().filter(|x| x.is_some()).count();
        let obj_count = self.buttons.0.count_ones() as usize
            + axis_count
            + self.dpad.as_ref().into_iter().count()
            + self.slider.as_ref().into_iter().count();

        let mut obj_diffs = Vec::with_capacity(obj_count);

        if let Some(st) = self.dpad.as_ref().cloned() {
            obj_diffs.push(ObjectDiff::DPad(st));
        }

        for (idx, ident) in J::BUTTONS.iter().enumerate() {
            if let Some(true) = self.buttons.0.bit(idx) {
                obj_diffs.push(ObjectDiff::Button(
                    *ident,
                    self.buttons.1.bit(idx).unwrap_or(false).into(),
                ));
            }
        }

        for (idx, ax) in J::AXIS
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.map(|prof| (i, prof)))
        {
            if let Some(st) = self.axis.get(idx).cloned().and_then(|x| x) {
                obj_diffs.push(ObjectDiff::Axis(ax.0, st));
            }
        }

        if let Some(st) = self.slider.as_ref().cloned() {
            obj_diffs.push(ObjectDiff::Slider(st));
        }

        obj_diffs
    }
}

pub struct DeviceInfo {
    pub name: String,
    pub buttons_num: usize,
    pub dpad: bool,
    pub axis: [Option<(i32, i32)>; AxisIdent::Limit as usize],
    pub slider: Option<(i32, i32)>,
}

pub enum Event<DI: Debug + PartialEq, B: Bits> {
    Attached(DI, DeviceInfo),
    Deattached(DI),
    StateDiff {
        id: DI,
        is_sink: bool,
        diff: StateDiff<B>,
    },
    Warn(Error),
    Interruption(Result<()>),
}

pub trait Driver {
    type DeviceIdent: Debug + PartialEq;
    type ButtonBits: Bits;

    fn as_event_receiver(&self) -> &Receiver<Event<Self::DeviceIdent, Self::ButtonBits>>;

    fn close(self);
}
