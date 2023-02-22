use std::{
    collections::HashMap,
    ffi::c_void,
    mem::{replace, size_of},
    slice::from_raw_parts_mut,
    time::SystemTime,
};

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use tracing::{debug, trace, warn, warn_span};

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
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                PostMessageW, RegisterClassExW, CW_USEDEFAULT, GIDC_ARRIVAL, GIDC_REMOVAL,
                HWND_MESSAGE, MSG, RIM_INPUT, RIM_INPUTSINK, WM_CLOSE, WM_INPUT,
                WM_INPUT_DEVICE_CHANGE, WNDCLASSEXW, WNDCLASS_STYLES,
            },
        },
    },
};

use super::ButtonBits;
use crate::{
    driver::{Bits, DeviceInfo, StateDiff},
    AxisIdent, ButtonIdent, DPadState,
};

type Event = crate::driver::Event<isize, u32>;

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

#[derive(Default)]
struct DeviceCap {
    dpad: Option<HIDP_VALUE_CAPS>,
    button_caps: Option<Vec<HIDP_BUTTON_CAPS>>,
    buttons_num: usize,
    axis_caps: [Option<HIDP_VALUE_CAPS>; AxisIdent::Limit as usize],
    slider: Option<HIDP_VALUE_CAPS>,
    mapping: HashMap<u16, DeviceObjectIndex>,
}

#[derive(Debug)]
enum DeviceObjectIndex {
    DPad,
    Button(ButtonIdent),
    Axis(AxisIdent),
    Slider,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DeviceObjectStates {
    dpad: Option<DPadState>,
    buttons: ButtonBits,
    axis: [Option<i32>; AxisIdent::Limit as usize],
    slider: Option<i32>,
}

struct DeviceStatus {
    _name: HSTRING,
    max_data_count: u32,
    pre_parsed_data: Vec<u8>,
    cap: DeviceCap,
    obj_states: DeviceObjectStates,
}

pub(super) unsafe fn start_message_loop(hwnd: HWND, event_tx: &Sender<Event>) -> Result<()> {
    register_events(hwnd).context("register events")?;
    debug!("register rawinput events");

    loop {
        trace!("waiting for message");
        let mut msg = MSG::default();

        match GetMessageW(&mut msg, hwnd, 0, WM_INPUT).0 {
            0 => return Ok(()),
            -1 => return Err(get_last_err()).context("GetMessageW"),
            _ => {}
        };

        let _span =
            warn_span!("message", code = msg.message, hwnd = ?msg.hwnd, wparam = ?msg.wParam, lparam = ?msg.lParam).entered();

        let mut devices = HashMap::new();

        let event_res = match msg.message {
            WM_CLOSE => {
                if !DestroyWindow(hwnd).as_bool() {
                    warn!("destory window: {:?}", get_last_err());
                }
                return Ok(());
            }

            WM_INPUT => process_input_message(&mut devices, msg.wParam, msg.lParam)
                .context("process input event"),

            WM_INPUT_DEVICE_CHANGE => {
                process_input_change_message(&mut devices, msg.wParam, msg.lParam)
                    .context("process input change event")
            }

            _ => Ok(None),
        };

        DispatchMessageW(&msg);

        if let Some(evt) = event_res.unwrap_or_else(|e| Some(Event::Warn(e))) {
            event_tx.send(evt).context("event chan broken")?;
        }
    }
}

unsafe fn register_events(hwnd: HWND) -> Result<()> {
    let devices_opts = [
        RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: HID_USAGE_GENERIC_JOYSTICK,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: HID_USAGE_GENERIC_GAMEPAD,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        },
    ];

    RegisterRawInputDevices(devices_opts.as_slice(), size_of::<RAWINPUTDEVICE>() as u32)
        .ok()
        .context("RegisterRawInputDevices")?;

    Ok(())
}

unsafe fn process_input_change_message(
    deivces: &mut HashMap<isize, DeviceStatus>,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<Option<Event>> {
    let _span = warn_span!("input change").entered();
    match wparam.0 as u32 {
        GIDC_ARRIVAL => {}
        GIDC_REMOVAL => {
            return {
                if deivces.remove(&lparam.0).is_none() {
                    warn!("no device found on removal");
                }

                Ok(Some(Event::Deattached(lparam.0)))
            }
        }

        _other => {
            warn!("unexpected wparam");
            return Ok(None);
        }
    };

    let (pub_info, profile) = match get_device(HANDLE(lparam.0)).context("get device info")? {
        Some(i) => i,
        None => return Ok(None),
    };

    deivces.insert(lparam.0, profile);

    Ok(Some(Event::Attached(lparam.0, pub_info)))
}

unsafe fn get_device(hdl: HANDLE) -> Result<Option<(DeviceInfo, DeviceStatus)>> {
    // device name
    let mut name_buf = [0u16; 1024];
    let name_buf_size = name_buf.len();
    let name_buf_used = sys_get_device_info(
        hdl,
        RIDI_DEVICENAME,
        name_buf.as_mut_ptr(),
        Some(name_buf_size),
    )
    .context("get device name")?;

    let hname = HSTRING::from_wide(&name_buf[..name_buf_used.min(name_buf_size)])
        .context("construct device name string")?;

    // device info
    let mut dev_info = RID_DEVICE_INFO {
        cbSize: size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };

    sys_get_device_info(hdl, RIDI_DEVICEINFO, &mut dev_info, None).context("get device info")?;
    if !is_hid_joystick(&dev_info) {
        warn!(
            dw = dev_info.dwType.0,
            page = dev_info.Anonymous.hid.usUsagePage,
            usage = dev_info.Anonymous.hid.usUsage,
            "device info of unsupported type"
        );
        return Ok(None);
    }

    // get pre parsed data
    let pre_parsed_data_size =
        sys_get_device_info_size(hdl, RIDI_PREPARSEDDATA).context("get pre parsed data size")?;

    let mut pre_parsed_data = allocate_buffer::<u8>(pre_parsed_data_size);
    sys_get_device_info(
        hdl,
        RIDI_PREPARSEDDATA,
        pre_parsed_data.as_mut_ptr(),
        Some(pre_parsed_data_size),
    )
    .context("get pre parsed data")?;

    let pre_parsed_data_ptr = pre_parsed_data.as_ptr() as isize;

    let max_data_count = HidP_MaxDataListLength(HidP_Input, pre_parsed_data_ptr);
    if max_data_count == 0 {
        return Err(anyhow!("failed to get max data count of HidP_Input"));
    }

    // get device caps
    let cap = match get_device_cap(pre_parsed_data_ptr).context("get device cap")? {
        Some(c) => c,
        None => return Ok(None),
    };

    let mut info = DeviceInfo {
        name: hname.to_string_lossy(),
        buttons_num: cap.buttons_num,
        dpad: cap.dpad.is_some(),
        axis: Default::default(),
        slider: None,
    };

    for (idx, value) in cap.axis_caps.iter().enumerate() {
        if let Some(val_caps) = value {
            let (mut vmin, mut vmax) = (val_caps.LogicalMin, val_caps.LogicalMax);
            if vmin == 0 && vmax == -1 {
                vmin = 0;
                vmax = u16::MAX as i32;
            }

            info.axis[idx].replace((vmin, vmax));
        }
    }

    if let Some(val_caps) = cap.slider.as_ref() {
        info.slider
            .replace((val_caps.LogicalMin, val_caps.LogicalMax));
    }

    let status = DeviceStatus {
        _name: hname,
        max_data_count,
        pre_parsed_data,
        cap,
        obj_states: Default::default(),
    };

    Ok(Some((info, status)))
}

#[inline]
unsafe fn is_hid_joystick(info: &RID_DEVICE_INFO) -> bool {
    info.dwType == RIM_TYPEHID
        && info.Anonymous.hid.usUsagePage == HID_USAGE_PAGE_GENERIC
        && (info.Anonymous.hid.usUsage == HID_USAGE_GENERIC_JOYSTICK
            || info.Anonymous.hid.usUsage == HID_USAGE_GENERIC_GAMEPAD)
}

#[inline]
unsafe fn sys_get_device_info<T>(
    hdl: HANDLE,
    cmd: RAW_INPUT_DEVICE_INFO_COMMAND,
    buf: *mut T,
    space: Option<usize>,
) -> Result<usize> {
    let buf_cap = space.unwrap_or(size_of::<T>()) as u32;
    let mut buf_size = buf_cap;
    match GetRawInputDeviceInfoW(hdl, cmd, Some(buf as *mut c_void), &mut buf_size) {
        FAIL => Err(anyhow!(
            "insufficient buf for {:?}, {} required",
            cmd,
            buf_size
        )),

        num if num <= buf_cap => Ok(num as usize),

        other => {
            Err(get_last_err()).with_context(|| format!("unexpected ret {} for {:?}", other, cmd))
        }
    }
}

#[inline]
unsafe fn sys_get_device_info_size(
    hdl: HANDLE,
    cmd: RAW_INPUT_DEVICE_INFO_COMMAND,
) -> Result<usize> {
    let mut size = 0u32;
    match GetRawInputDeviceInfoW(hdl, cmd, None, &mut size) {
        SUCCESS => Ok(size as usize),
        other => {
            Err(get_last_err()).with_context(|| format!("unexpected ret {} for {:?}", other, cmd))
        }
    }
}

#[inline]
unsafe fn get_device_cap(pre_parsed_data_ptr: isize) -> Result<Option<DeviceCap>> {
    let max_data_count = HidP_MaxDataListLength(HidP_Input, pre_parsed_data_ptr);
    if max_data_count == 0 {
        return Err(anyhow!("HidP_MaxDataListLength got zero"));
    }

    let mut hidp_caps = HIDP_CAPS::default();
    HidP_GetCaps(pre_parsed_data_ptr, &mut hidp_caps).context("HidP_GetCaps")?;

    debug!("hidp caps: {:?}", hidp_caps);

    if hidp_caps.NumberInputButtonCaps == 0 && hidp_caps.NumberInputValueCaps == 0 {
        warn!("no buttons & values available");
        return Ok(None);
    }

    let mut dev_cap = DeviceCap::default();

    // construct button caps and mappings
    if hidp_caps.NumberInputButtonCaps > 0 {
        let mut button_caps_num = hidp_caps.NumberInputButtonCaps;
        let mut button_caps = allocate_buffer::<HIDP_BUTTON_CAPS>(button_caps_num as usize);

        HidP_GetButtonCaps(
            HidP_Input,
            button_caps.as_mut_ptr(),
            &mut button_caps_num,
            pre_parsed_data_ptr,
        )
        .context("HidP_GetButtonCaps")?;

        for button_cap in button_caps.iter().take(button_caps_num as usize) {
            if button_cap.IsRange.as_bool() {
                for data_idx in button_cap.Anonymous.Range.DataIndexMin
                    ..=button_cap.Anonymous.Range.DataIndexMax
                {
                    let btn_idx = dev_cap.mapping.len();
                    dev_cap
                        .mapping
                        .insert(data_idx, DeviceObjectIndex::Button(btn_idx));
                }
            } else {
                let btn_idx = dev_cap.mapping.len();
                dev_cap.mapping.insert(
                    button_cap.Anonymous.NotRange.DataIndex,
                    DeviceObjectIndex::Button(btn_idx),
                );
            }
        }

        let buttons_num = dev_cap.mapping.len();

        if buttons_num > ButtonBits::CAP {
            warn!(
                cap = ButtonBits::CAP,
                num = buttons_num,
                "input button caps: maximum bits cap exceeded",
            );
            return Ok(None);
        }

        dev_cap.button_caps.replace(button_caps);
        dev_cap.buttons_num = buttons_num;
    }

    // construct value caps & mappings
    if hidp_caps.NumberInputValueCaps > 0 {
        let mut values_num = hidp_caps.NumberInputValueCaps;
        let mut values = allocate_buffer::<HIDP_VALUE_CAPS>(values_num as usize);

        HidP_GetValueCaps(
            HidP_Input,
            values.as_mut_ptr(),
            &mut values_num,
            pre_parsed_data_ptr,
        )
        .context("HidP_GetValueCaps")?;

        for cap in values {
            let (di, usage) = if cap.IsRange.as_bool() {
                (
                    cap.Anonymous.Range.DataIndexMin,
                    cap.Anonymous.Range.UsageMin,
                )
            } else {
                (
                    cap.Anonymous.NotRange.DataIndex,
                    cap.Anonymous.NotRange.Usage,
                )
            };

            let object = match (cap.UsagePage, usage) {
                (HID_USAGE_PAGE_GENERIC, HID_USAGE_GENERIC_SLIDER) => {
                    Some((&mut dev_cap.slider, DeviceObjectIndex::Slider))
                }

                (HID_USAGE_PAGE_GENERIC, HID_USAGE_GENERIC_HATSWITCH) => {
                    if !(cap.LogicalMin == 0 && cap.LogicalMax == 7) {
                        warn!(
                            min = cap.LogicalMin,
                            max = cap.LogicalMax,
                            "unexpected value range for hat"
                        );
                        None
                    } else {
                        Some((&mut dev_cap.dpad, DeviceObjectIndex::DPad))
                    }
                }

                (HID_USAGE_PAGE_GENERIC, usage) if HID_AXIS_USAGES.contains(&usage) => {
                    let idx = match usage {
                        HID_USAGE_GENERIC_X => AxisIdent::X,
                        HID_USAGE_GENERIC_Y => AxisIdent::Y,
                        HID_USAGE_GENERIC_Z => AxisIdent::Z,
                        HID_USAGE_GENERIC_RX => AxisIdent::RX,
                        HID_USAGE_GENERIC_RY => AxisIdent::RY,
                        HID_USAGE_GENERIC_RZ => AxisIdent::RZ,
                        _ => unreachable!("unexpected usage id {} for axis", usage),
                    };

                    dev_cap
                        .axis_caps
                        .get_mut(idx as usize)
                        .map(|slot| (slot, DeviceObjectIndex::Axis(idx)))
                }

                (_upage, _uid) => None,
            };

            let _span = warn_span!("value caps", page = cap.UsagePage, usage, di);
            match object {
                Some((slot, dev_id)) => {
                    if let Some(prev) = slot.replace(cap) {
                        let prev_di = if prev.IsRange.as_bool() {
                            prev.Anonymous.Range.DataIndexMin
                        } else {
                            prev.Anonymous.NotRange.DataIndex
                        };

                        warn!(prev_di, "duplicate typed object");
                    }

                    if let Some(prev_dev_id) = dev_cap.mapping.insert(di, dev_id) {
                        warn!(?prev_dev_id, "duplicate data index");
                    }
                }

                None => {
                    trace!("no slot")
                }
            }
        }
    }

    Ok(Some(dev_cap))
}

unsafe fn process_input_message(
    devices: &mut HashMap<isize, DeviceStatus>,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<Option<Event>> {
    let is_sink = match wparam.0 as u32 {
        RIM_INPUT => false,

        RIM_INPUTSINK => true,

        _other => {
            warn!("unexpected wparam code");
            return Ok(None);
        }
    };

    get_input_event(devices, is_sink, lparam)
}

#[inline]
unsafe fn get_raw_input_data(hdl: isize) -> Result<Vec<u8>> {
    let mut raw_data_size = 0u32;
    if GetRawInputData(
        HRAWINPUT(hdl),
        RID_INPUT,
        None,
        &mut raw_data_size,
        size_of::<RAWINPUTHEADER>() as u32,
    ) != 0
    {
        return Err(get_last_err()).context("get raw data size by calling GetRawInputData");
    };

    let mut raw_data = allocate_buffer::<u8>(raw_data_size as usize);
    if GetRawInputData(
        HRAWINPUT(hdl),
        RID_INPUT,
        Some(raw_data.as_mut_ptr() as *mut c_void),
        &mut raw_data_size,
        size_of::<RAWINPUTHEADER>() as u32,
    ) == FAIL
    {
        return Err(get_last_err()).context("GetRawInputData");
    };

    Ok(raw_data)
}

#[inline]
unsafe fn sys_hidp_get_data(
    status: &DeviceStatus,
    report_raw: &mut [u8],
    data_buf: &mut [HIDP_DATA],
) -> Result<u32> {
    let mut data_len = data_buf.len() as u32;
    HidP_GetData(
        HidP_Input,
        data_buf.as_mut_ptr(),
        &mut data_len,
        status.pre_parsed_data.as_ptr() as isize,
        report_raw,
    )?;

    Ok(data_len)
}

#[inline]
unsafe fn get_input_event(
    devices: &mut HashMap<isize, DeviceStatus>,
    is_sink: bool,
    hdl: LPARAM,
) -> Result<Option<Event>> {
    let mut raw_data_bytes = get_raw_input_data(hdl.0)?;
    let raw_data_ptr = raw_data_bytes.as_mut_ptr() as *mut RAWINPUT;
    let raw_data = &mut *raw_data_ptr;

    if raw_data.header.dwType != RIM_TYPEHID.0 {
        return Ok(None);
    }

    let hdev = raw_data.header.hDevice.0;
    let dev_status = devices
        .get_mut(&hdev)
        .ok_or_else(|| anyhow!("device info for {} not found", hdl.0))?;

    let mut new_states = DeviceObjectStates::default();

    let report_size = (raw_data.data.hid.dwCount * raw_data.data.hid.dwSizeHid) as usize;
    let reports = from_raw_parts_mut(raw_data.data.hid.bRawData.as_mut_ptr(), report_size);

    let mut data_buf = allocate_buffer::<HIDP_DATA>(dev_status.max_data_count as usize);

    for (chunk_idx, chunk) in reports
        .chunks_mut(raw_data.data.hid.dwSizeHid as usize)
        .enumerate()
    {
        let data_count =
            sys_hidp_get_data(dev_status, chunk, &mut data_buf).with_context(|| {
                format!(
                    "HidP_GetData for report chunk {}/{}",
                    chunk_idx, raw_data.data.hid.dwCount
                )
            })?;

        for data in data_buf.iter().take(data_count as usize) {
            let obj_idx = match dev_status.cap.mapping.get(&data.DataIndex) {
                Some(i) => i,
                None => {
                    trace!("object index not found for {}", data.DataIndex);
                    continue;
                }
            };

            let _data_value_span =
                warn_span!("data value", data_idx = data.DataIndex, ?obj_idx).entered();

            match obj_idx {
                DeviceObjectIndex::DPad => {
                    let st = match data.Anonymous.RawValue {
                        0 => DPadState::Up,
                        1 => DPadState::UpRight,
                        2 => DPadState::Right,
                        3 => DPadState::DownRight,
                        4 => DPadState::Down,
                        5 => DPadState::DownLeft,
                        6 => DPadState::Left,
                        7 => DPadState::UpLeft,
                        _other => DPadState::Null,
                    };

                    new_states.dpad.replace(st);
                }

                DeviceObjectIndex::Button(idx) => {
                    if data.Anonymous.On.as_bool() {
                        new_states.buttons.set(*idx);
                    }
                }

                DeviceObjectIndex::Axis(idx) => {
                    if let Some(slot) = new_states.axis.get_mut(*idx as usize) {
                        slot.replace(data.Anonymous.RawValue as i32);
                    }
                }

                DeviceObjectIndex::Slider => {
                    new_states.slider.replace(data.Anonymous.RawValue as i32);
                }
            }
        }
    }

    let prev_state = replace(&mut dev_status.obj_states, new_states);
    let btns_diff = dev_status.obj_states.buttons ^ prev_state.buttons;

    let evt = Event::StateDiff {
        id: hdev,
        is_sink,
        diff: StateDiff {
            dpad: dev_status.obj_states.dpad,
            buttons: (btns_diff, dev_status.obj_states.buttons),
            axis: dev_status.obj_states.axis,
            slider: dev_status.obj_states.slider,
        },
    };

    Ok(Some(evt))
}

#[inline]
fn allocate_buffer<T: Default + Clone>(cap: usize) -> Vec<T> {
    let buf = vec![T::default(); cap];
    buf
}
