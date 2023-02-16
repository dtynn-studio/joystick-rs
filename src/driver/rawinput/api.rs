use std::mem::size_of;
use std::time::SystemTime;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use tracing::trace;

use windows::{
    core::{Error as wError, HSTRING, PCWSTR},
    Win32::{
        Devices::HumanInterfaceDevice::{
            HidP_GetButtonCaps, HidP_GetCaps, HidP_GetData, HidP_GetValueCaps, HidP_Input,
            HidP_MaxDataListLength, HIDP_BUTTON_CAPS, HIDP_CAPS, HIDP_DATA, HIDP_VALUE_CAPS,
            HID_USAGE_GENERIC_GAMEPAD, HID_USAGE_GENERIC_HATSWITCH, HID_USAGE_GENERIC_JOYSTICK,
            HID_USAGE_GENERIC_RX, HID_USAGE_GENERIC_RY, HID_USAGE_GENERIC_RZ,
            HID_USAGE_GENERIC_SLIDER, HID_USAGE_GENERIC_X, HID_USAGE_GENERIC_Y,
            HID_USAGE_GENERIC_Z, HID_USAGE_PAGE_GENERIC,
        },
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, SUCCESS, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::{
                GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, HRAWINPUT,
                RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RAW_INPUT_DEVICE_INFO_COMMAND,
                RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDI_DEVICEINFO, RIDI_DEVICENAME,
                RIDI_PREPARSEDDATA, RID_DEVICE_INFO, RID_INPUT, RIM_TYPEHID,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetMessageW, PostMessageW,
                RegisterClassExW, CW_USEDEFAULT, GIDC_ARRIVAL, GIDC_REMOVAL, HWND_MESSAGE, MSG,
                RIM_INPUT, RIM_INPUTSINK, WM_CLOSE, WM_INPUT, WM_INPUT_DEVICE_CHANGE, WNDCLASSEXW,
                WNDCLASS_STYLES,
            },
        },
    },
};

use crate::driver::Event;

const FAIL: u32 = -1i32 as u32;

const HID_AXIS_USAGES: [u16; 6] = [
    HID_USAGE_GENERIC_X,
    HID_USAGE_GENERIC_Y,
    HID_USAGE_GENERIC_Z,
    HID_USAGE_GENERIC_RX,
    HID_USAGE_GENERIC_RY,
    HID_USAGE_GENERIC_RZ,
];

#[inline]
unsafe fn get_last_err() -> wError {
    wError::from_win32()
}

unsafe extern "system" fn window_proc_sys(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    trace!(?hwnd, msg, ?wparam, ?lparam, "recv window message");

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

pub(super) unsafe fn setup_message_window() -> Result<HWND> {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .context("generate unix timestamp")?;

    let hinstance = GetModuleHandleW(None).context("GetModuleHandleW")?;

    let classname_raw = HSTRING::from(format!("joystick-rs-rawinput-{}", ts));
    let classname = PCWSTR::from_raw(classname_raw.as_ptr());

    let wclass = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES::default(),
        lpfnWndProc: Some(window_proc_sys),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: None.into(),
        hCursor: None.into(),
        hbrBackground: None.into(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR::from_raw(classname.as_ptr()),
        hIconSm: None.into(),
    };

    if RegisterClassExW(&wclass) == 0 {
        return Err(get_last_err()).context("RegisterClassExW");
    }

    let hwnd = CreateWindowExW(
        Default::default(),
        classname,
        classname,
        Default::default(),
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        HWND_MESSAGE,
        None,
        hinstance,
        None,
    );

    if hwnd.0 == 0 {
        return Err(get_last_err()).context("CreateWindowExW");
    }

    Ok(hwnd)
}

pub(super) unsafe fn close_message_window(hwnd: HWND) -> Result<()> {
    if !PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).as_bool() {
        return Err(get_last_err()).context("send WM_CLOSE message to window handle");
    };

    Ok(())
}

pub(super) unsafe fn start_message_loop(
    hwnd: HWND,
    event_tx: &Sender<Result<Event<HSTRING, u32>>>,
) -> Result<()> {
    unimplemented!();
}
