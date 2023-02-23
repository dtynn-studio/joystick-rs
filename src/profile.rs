use crate::{Axis, AxisDef, AxisIdent, Button, Joystick};

#[derive(Debug, Default, Clone, Copy)]
pub struct PS4Compact;

impl Joystick<14> for PS4Compact {
    const DPAD: bool = true;

    const BUTTONS: [Button; 14] = [
        Button::West,
        Button::South,
        Button::East,
        Button::North,
        Button::LShoulder,
        Button::RShoulder,
        Button::LTrigger,
        Button::RTrigger,
        Button::Select,
        Button::Start,
        Button::LThumb,
        Button::RThumb,
        Button::Mode,
        Button::Other("TrackPad"),
    ];

    const AXIS: [Option<AxisDef>; AxisIdent::Limit as usize] = [
        Some(AxisDef {
            typ: Axis::LThumbX,
            centered: true,
        }),
        Some(AxisDef {
            typ: Axis::LThumbY,
            centered: true,
        }),
        Some(AxisDef {
            typ: Axis::RThumbX,
            centered: true,
        }),
        Some(AxisDef {
            typ: Axis::LTrigger,
            centered: false,
        }),
        Some(AxisDef {
            typ: Axis::RTrigger,
            centered: false,
        }),
        Some(AxisDef {
            typ: Axis::RThumbY,
            centered: true,
        }),
    ];
}
