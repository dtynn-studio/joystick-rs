use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::ops::RangeInclusive;
use std::thread::{spawn, JoinHandle};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use tracing::{debug, trace, warn, warn_span};
use windows::{
    core::{Error as wError, HSTRING, PCWSTR},
    Win32::{
        Devices::HumanInterfaceDevice::{
            HidP_GetButtonCaps, HidP_GetCaps, HidP_GetValueCaps, HidP_Input, HIDP_BUTTON_CAPS,
            HIDP_CAPS, HIDP_VALUE_CAPS, HID_USAGE_GENERIC_GAMEPAD, HID_USAGE_GENERIC_HATSWITCH,
            HID_USAGE_GENERIC_JOYSTICK, HID_USAGE_GENERIC_RX, HID_USAGE_GENERIC_RY,
            HID_USAGE_GENERIC_RZ, HID_USAGE_GENERIC_SLIDER, HID_USAGE_GENERIC_X,
            HID_USAGE_GENERIC_Y, HID_USAGE_GENERIC_Z, HID_USAGE_PAGE_GENERIC,
        },
        Foundation::{
            GetLastError, ERROR_INSUFFICIENT_BUFFER, HANDLE, HWND, LPARAM, LRESULT, SUCCESS, WPARAM,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::{
                GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices,
                RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAW_INPUT_DEVICE_INFO_COMMAND, RIDEV_DEVNOTIFY,
                RIDEV_INPUTSINK, RIDI_DEVICEINFO, RIDI_DEVICENAME, RIDI_PREPARSEDDATA,
                RID_DEVICE_INFO, RID_DEVICE_INFO_HID, RIM_TYPEHID,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetMessageW, RegisterClassExW,
                CW_USEDEFAULT, GIDC_ARRIVAL, GIDC_REMOVAL, HWND_MESSAGE, MSG, WM_INPUT,
                WM_INPUT_DEVICE_CHANGE, WNDCLASSEXW, WNDCLASS_STYLES,
            },
        },
    },
};

use crate::driver::{AxisType, Manager};

const FAIL: u32 = -1i32 as u32;

const HID_AXIS_USAGES: [u16; 6] = [
    HID_USAGE_GENERIC_X,
    HID_USAGE_GENERIC_Y,
    HID_USAGE_GENERIC_Z,
    HID_USAGE_GENERIC_RX,
    HID_USAGE_GENERIC_RY,
    HID_USAGE_GENERIC_RZ,
];

type DevIdent = isize;
pub type Value = i32;
pub type ValueRange = RangeInclusive<Value>;
type Event = crate::driver::Event<DevIdent, ValueRange>;
type DeviceInfo = crate::driver::DeviceInfo<ValueRange>;
type DeviceSpecs = crate::driver::DeviceSpecs<ValueRange>;

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

#[derive(Debug, Clone, Copy)]
pub enum DeviceType {
    Joystick,
    Gamepad,
    Both,
}

impl Default for DeviceType {
    fn default() -> Self {
        Self::Both
    }
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub dev_type: DeviceType,
    pub event_buf_size: Option<usize>,
}

impl Config {
    pub fn start(self) -> Result<Mgr> {
        unsafe {
            let hwnd = setup_message_window()?;
            let (tx, rx) = match self.event_buf_size {
                Some(size) => bounded(size),
                None => unbounded(),
            };

            let join = spawn(move || {
                let tx = tx;
                let _span = warn_span!("event loop").entered();
                debug!("start");
                if let Err(e) = start_event_loop(hwnd, self, &tx) {
                    _ = tx.send(Err(e));
                };
                debug!("stop");
            });

            let notifier = Mgr {
                rx,
                hwnd,
                join: Some(join),
            };

            Ok(notifier)
        }
    }
}

#[derive(Debug)]
pub enum DeviceObjectIndex {
    Button(usize),
    Axis(usize),
    Slider(usize),
    Hat(usize),
    Unknown(u16, u16),
}

type DevObjectsMapping = HashMap<u16, DeviceObjectIndex>;

#[derive(Debug, Default)]
struct DevInfo {
    name: HSTRING,
    buttons: usize,
    axises: Vec<AxisType>,
    sliders: usize,
    hats: usize,
    mapping: DevObjectsMapping,
}

#[derive(Debug)]
pub struct Mgr {
    rx: Receiver<Result<Event>>,
    hwnd: HWND,
    join: Option<JoinHandle<()>>,
}

impl Drop for Mgr {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd) };
        if let Some(join) = self.join.take() {
            _ = join.join();
        }
    }
}

impl Manager for Mgr {
    type DeviceIdent = DevIdent;
    type Value = Value;
    type ValueRange = ValueRange;

    fn as_event_receiver(&self) -> &Receiver<Result<Event>> {
        &self.rx
    }
}

unsafe fn setup_message_window() -> Result<HWND> {
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

unsafe fn register_devices(hwnd: HWND, typ: DeviceType) -> Result<()> {
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

    RegisterRawInputDevices(
        match typ {
            DeviceType::Joystick => &devices_opts[..1],
            DeviceType::Gamepad => &devices_opts[1..],
            DeviceType::Both => &devices_opts[..],
        },
        size_of::<RAWINPUTDEVICE>() as u32,
    )
    .ok()
    .context("RegisterRawInputDevices")?;

    Ok(())
}

fn proc_input_message(
    deivces: &mut HashMap<isize, DevInfo>,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<Event> {
    unimplemented!()
}

unsafe fn proc_input_change_message(
    deivces: &mut HashMap<isize, DevInfo>,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<Event> {
    match wparam.0 as u32 {
        GIDC_ARRIVAL => {}
        GIDC_REMOVAL => return Ok(Event::DeviceDeattached(lparam.0)),
        other => return Err(anyhow!("unexpected wparam {}", other)),
    };

    let (info, specs) = get_device(HANDLE(lparam.0)).context("get device")?;
    let dev_info = DeviceInfo {
        name: info.name.to_string_lossy(),
        specs,
    };

    deivces.insert(lparam.0, info);

    Ok(Event::DeviceAttached(lparam.0, dev_info))
}

unsafe fn get_device(dev_hdl: HANDLE) -> Result<(DevInfo, DeviceSpecs)> {
    let _span = warn_span!("device", hdl = ?dev_hdl).entered();
    let mut name_buf = [0u16; 1024];
    let name_buf_size = name_buf.len();
    let name_buf_used = sys_get_device_info(
        dev_hdl,
        RIDI_DEVICENAME,
        name_buf.as_mut_ptr(),
        Some(name_buf_size),
    )
    .context("get device name")?;

    let hname = HSTRING::from_wide(&name_buf[..name_buf_used.min(name_buf_size)])
        .context("construct device name string")?;

    let mut dev_info = RID_DEVICE_INFO {
        cbSize: size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };

    sys_get_device_info(dev_hdl, RIDI_DEVICEINFO, &mut dev_info, None)?;
    if !is_hid_joystick(&dev_info) {
        return Err(anyhow!(
            "unexpected device type 0x{:x}(0x{:x}-0x{:x})",
            dev_info.dwType.0,
            dev_info.Anonymous.hid.usUsagePage,
            dev_info.Anonymous.hid.usUsage
        ));
    }

    // get pre parsed data
    let pre_data_size = sys_get_device_info_size(dev_hdl, RIDI_PREPARSEDDATA)
        .context("get pre parsed data size")?;

    let mut pre_data_buf = allocate_buffer::<u8>(pre_data_size);
    sys_get_device_info(
        dev_hdl,
        RIDI_PREPARSEDDATA,
        pre_data_buf.as_mut_ptr(),
        Some(pre_data_size),
    )
    .context("get pre parsed data")?;

    let pre_data_buf_ptr = pre_data_buf.as_ptr() as isize;

    let mut hidp_caps = HIDP_CAPS::default();
    HidP_GetCaps(pre_data_buf_ptr, &mut hidp_caps).context("get device caps")?;

    trace!("caps: {:?}", hidp_caps);

    let mut info = DevInfo {
        name: hname,
        ..Default::default()
    };

    let mut axis_specs = Vec::new();
    let mut slider_specs = Vec::new();

    if hidp_caps.NumberInputButtonCaps > 0 {
        let mut buttons_num = hidp_caps.NumberInputButtonCaps;
        let mut buttons = allocate_buffer::<HIDP_BUTTON_CAPS>(buttons_num as usize);

        HidP_GetButtonCaps(
            HidP_Input,
            buttons.as_mut_ptr(),
            &mut buttons_num,
            pre_data_buf_ptr,
        )
        .context("HidP_GetButtonCaps")?;

        for cap in buttons {
            if cap.IsRange.as_bool() {
                for di in cap.Anonymous.Range.DataIndexMin..=cap.Anonymous.Range.DataIndexMax {
                    if let Some(prev) = info
                        .mapping
                        .insert(di, DeviceObjectIndex::Button(info.buttons))
                    {
                        warn!(
                            di,
                            "found duplicate data index for button, prev: {:?}", prev
                        );
                    };

                    info.buttons += 1;
                }
            } else {
                if let Some(prev) = info.mapping.insert(
                    cap.Anonymous.NotRange.DataIndex,
                    DeviceObjectIndex::Button(info.buttons),
                ) {
                    warn!(
                        di = cap.Anonymous.NotRange.DataIndex,
                        "found duplicate data index for button, prev: {:?}", prev
                    );
                }

                info.buttons += 1;
            }
        }
    };

    if hidp_caps.NumberInputValueCaps > 0 {
        let mut values_num = hidp_caps.NumberInputValueCaps;
        let mut values = allocate_buffer::<HIDP_VALUE_CAPS>(values_num as usize);

        HidP_GetValueCaps(
            HidP_Input,
            values.as_mut_ptr(),
            &mut values_num,
            pre_data_buf_ptr,
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
                    cap.Anonymous.NotRange.Usage,
                    cap.Anonymous.NotRange.DataIndex,
                )
            };

            let item = match (cap.UsagePage, usage) {
                (HID_USAGE_PAGE_GENERIC, HID_USAGE_GENERIC_SLIDER) => {
                    let idx = info.sliders;
                    info.sliders += 1;
                    slider_specs.push(cap.LogicalMin..=cap.LogicalMax);
                    DeviceObjectIndex::Slider(idx)
                }

                (HID_USAGE_PAGE_GENERIC, HID_USAGE_GENERIC_HATSWITCH) => {
                    let idx = info.hats;
                    info.hats += 1;
                    DeviceObjectIndex::Hat(idx)
                }

                (HID_USAGE_PAGE_GENERIC, usage) if HID_AXIS_USAGES.contains(&usage) => {
                    let atyp = match usage {
                        HID_USAGE_GENERIC_X => AxisType::X,
                        HID_USAGE_GENERIC_Y => AxisType::Y,
                        HID_USAGE_GENERIC_Z => AxisType::Z,
                        HID_USAGE_GENERIC_RX => AxisType::RX,
                        HID_USAGE_GENERIC_RY => AxisType::RY,
                        HID_USAGE_GENERIC_RZ => AxisType::RZ,
                        _ => unreachable!("unexpected usage id {} for axis", usage),
                    };

                    let idx = info.axises.len();
                    info.axises.push(atyp);
                    axis_specs.push((atyp, cap.LogicalMin..=cap.LogicalMax));
                    DeviceObjectIndex::Axis(idx)
                }

                (upage, uid) => DeviceObjectIndex::Unknown(upage, uid),
            };

            if let Some(prev) = info.mapping.insert(di, item) {
                warn!(
                    di,
                    "found duplicate data index for value(0x{:04x}-0x{:04x}), prev: {:?}",
                    cap.UsagePage,
                    usage,
                    prev
                );
            };
        }
    };

    let specs = DeviceSpecs {
        button_count: info.buttons,
        axis: axis_specs,
        sliders: slider_specs,
        hats_count: info.hats,
    };

    Ok((info, specs))
}

unsafe fn start_event_loop(hwnd: HWND, cfg: Config, tx: &Sender<Result<Event>>) -> Result<()> {
    register_devices(hwnd, cfg.dev_type)?;

    let mut devices = HashMap::new();
    loop {
        let mut msg = MSG::default();
        match GetMessageW(&mut msg, hwnd, WM_INPUT_DEVICE_CHANGE, WM_INPUT).0 {
            0 => return Ok(()),
            -1 => return Err(get_last_err()).context("GetMessageW"),
            code @ i32::MIN..=-2 => {
                return Err(anyhow!(
                    "unexpected negative ret code {} from GetMessageW",
                    code
                ))
            }
            _ => {}
        };

        let _span =
            warn_span!("message", msg.message, ?msg.hwnd, ?msg.wParam, ?msg.lParam).entered();

        trace!("received");

        let res = match msg.message {
            WM_INPUT => {
                proc_input_message(&mut devices, msg.wParam, msg.lParam).context("proc input msg")
            }
            WM_INPUT_DEVICE_CHANGE => {
                proc_input_change_message(&mut devices, msg.wParam, msg.lParam)
                    .context("proc input change msg")
            }
            _other => {
                warn!("unexpected msg type");
                continue;
            }
        };

        trace!("processed");

        match res {
            Ok(evt) => tx.send(Ok(evt)).context("event chan broken")?,
            Err(e) => {
                warn!("failed: {:?}", e);
            }
        }
    }
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
fn allocate_buffer<T: Default + Clone>(cap: usize) -> Vec<T> {
    let buf = vec![T::default(); cap];
    buf
}

#[inline]
unsafe fn is_hid_joystick(info: &RID_DEVICE_INFO) -> bool {
    info.dwType == RIM_TYPEHID
        && info.Anonymous.hid.usUsagePage == HID_USAGE_PAGE_GENERIC
        && (info.Anonymous.hid.usUsage == HID_USAGE_GENERIC_JOYSTICK
            || info.Anonymous.hid.usUsage == HID_USAGE_GENERIC_GAMEPAD)
}

// impl Manager {
//     pub fn new(include_xbox: bool) -> Result<Self> {
//         let classname = w!("joystick-rs rawinput");
//         unsafe {
//             let hinstance = GetModuleHandleW(None)?;

//             let wclass = WNDCLASSEXW {
//                 cbSize: size_of::<WNDCLASSEXW>() as u32,
//                 style: WNDCLASS_STYLES::default(),
//                 lpfnWndProc: Some(window_proc_sys),
//                 cbClsExtra: 0,
//                 cbWndExtra: 0,
//                 hInstance: hinstance,
//                 hIcon: None.into(),
//                 hCursor: None.into(),
//                 hbrBackground: None.into(),
//                 lpszMenuName: PCWSTR::null(),
//                 lpszClassName: classname,
//                 hIconSm: None.into(),
//             };

//             if RegisterClassExW(&wclass) == 0 {
//                 return Err(get_last_err("get zero from RegisterClassExW").into());
//             }

//             let hwnd = CreateWindowExW(
//                 Default::default(),
//                 classname,
//                 classname,
//                 Default::default(),
//                 CW_USEDEFAULT,
//                 CW_USEDEFAULT,
//                 CW_USEDEFAULT,
//                 CW_USEDEFAULT,
//                 HWND_MESSAGE,
//                 None,
//                 hinstance,
//                 None,
//             );

//             if hwnd.0 == 0 {
//                 return Err(get_last_err("get null from CreateWindowExW").into());
//             }

//             println!("window {:?} constructed", hwnd);

//             let mut mgr = Self { hwnd };
//             mgr.register_devices(include_xbox)?;

//             Ok(mgr)
//         }
//     }

//     unsafe fn register_devices(&mut self, include_xbox: bool) -> Result<()> {
//         // https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/hid-architecture#hid-clients-supported-in-windows
//         // TODO: RIDEV_DEVNOTIFY
//         let devices = [
//             RAWINPUTDEVICE {
//                 usUsagePage: HID_USAGE_PAGE_GENERIC,
//                 usUsage: HID_USAGE_GENERIC_JOYSTICK,
//                 dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
//                 hwndTarget: self.hwnd,
//             },
//             RAWINPUTDEVICE {
//                 usUsagePage: HID_USAGE_PAGE_GENERIC,
//                 usUsage: HID_USAGE_GENERIC_GAMEPAD,
//                 dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
//                 hwndTarget: self.hwnd,
//             },
//         ];

//         RegisterRawInputDevices(
//             if include_xbox {
//                 &devices[..]
//             } else {
//                 &devices[..1]
//             },
//             size_of::<RAWINPUTDEVICE>() as u32,
//         )
//         .ok()?;

//         Ok(())
//     }

//     pub fn list_devices(&mut self) -> Result<Vec<Joystick>> {
//         let mut dev_list: [RAWINPUTDEVICELIST; RAW_DEV_LIST_NUM] = Default::default();
//         const DEV_NUM_MAX: u32 = RAW_DEV_LIST_NUM as u32;
//         let mut dev_num = dev_list.len() as u32;

//         unsafe {
//             match GetRawInputDeviceList(
//                 Some(dev_list.as_mut_ptr()),
//                 &mut dev_num,
//                 size_of::<RAWINPUTDEVICELIST>() as u32,
//             ) {
//                 count @ 0..=DEV_NUM_MAX => {
//                     dev_num = count;
//                 }

//                 code if code == ERROR_INSUFFICIENT_BUFFER.0 => {
//                     return Err(wError::new(
//                         ERROR_INSUFFICIENT_BUFFER.to_hresult(),
//                         format!(
//                             "insufficient buffer for {} devices from GetRawInputDeviceList",
//                             dev_num
//                         )
//                         .into(),
//                     )
//                     .into())
//                 }

//                 FAIL => return Err(get_last_err("GetRawInputDeviceList").into()),

//                 other => {
//                     return Err(anyhow!(
//                         "unexpected return value {} from GetRawInputDeviceList",
//                         other
//                     ))
//                 }
//             };

//             let joysticks = dev_list
//                 .into_iter()
//                 .take(dev_num as usize)
//                 .filter_map(|dev| {
//                     Joystick::from(dev)
//                         .map_err(|e| println!("init joystick for {:?}: {:?}", dev.hDevice, e))
//                         .ok()
//                         .unwrap_or(None)
//                 })
//                 .collect();

//             Ok(joysticks)
//         }
//     }

//     pub fn on_message(&mut self) -> Result<()> {
//         unsafe {
//             let mut msg = MSG::default();
//             if GetMessageW(&mut msg, self.hwnd, 0, 0).as_bool() {
//                 println!(
//                     "get msg: {}: {:?}, {:?}",
//                     msg.message, msg.wParam, msg.lParam
//                 );
//             };
//         }

//         Ok(())
//     }
// }

// #[derive(Debug)]
// pub struct Joystick {
//     dev: RAWINPUTDEVICELIST,
//     name: HSTRING,
//     info: RID_DEVICE_INFO_HID,
// }

// impl Joystick {
//     unsafe fn from(dev: RAWINPUTDEVICELIST) -> Result<Option<Self>> {
//         if dev.dwType != RIM_TYPEHID {
//             return Ok(None);
//         }

//         let mut name_buf = [0u16; 1024];
//         let name_buf_size = name_buf.len();
//         let name_buf_used = get_device_info(
//             dev.hDevice,
//             RIDI_DEVICENAME,
//             name_buf.as_mut_ptr(),
//             Some(name_buf_size),
//         )?;

//         let name = HSTRING::from_wide(&name_buf[..name_buf_used])?;

//         let mut dev_info = RID_DEVICE_INFO {
//             cbSize: size_of::<RID_DEVICE_INFO>() as u32,
//             ..Default::default()
//         };

//         get_device_info(dev.hDevice, RIDI_DEVICEINFO, &mut dev_info, None)?;
//         if !is_hid_joystick(&dev_info.Anonymous.hid) {
//             return Ok(None);
//         }

//         // get pre parsed data
//         let pre_data_size = get_device_info_size(dev.hDevice, RIDI_PREPARSEDDATA)?;
//         let mut pre_data_buf = allocate_buffer::<u8>(pre_data_size);
//         get_device_info(
//             dev.hDevice,
//             RIDI_PREPARSEDDATA,
//             pre_data_buf.as_mut_ptr(),
//             Some(pre_data_size),
//         )?;

//         let pre_data_buf_ptr = pre_data_buf.as_ptr() as isize;

//         let mut hidp_caps = HIDP_CAPS::default();
//         HidP_GetCaps(pre_data_buf_ptr, &mut hidp_caps)?;

//         println!("Caps: {:?}", hidp_caps);

//         let buttons = if hidp_caps.NumberInputButtonCaps > 0 {
//             let mut button_num = hidp_caps.NumberInputButtonCaps;
//             let mut buttons = allocate_buffer::<HIDP_BUTTON_CAPS>(button_num as usize);

//             HidP_GetButtonCaps(
//                 HidP_Input,
//                 buttons.as_mut_ptr(),
//                 &mut button_num,
//                 pre_data_buf_ptr,
//             )?;

//             println!(
//                 "Buttons [{}, {}]",
//                 buttons[0].Anonymous.Range.UsageMin, buttons[0].Anonymous.Range.UsageMax
//             );

//             buttons
//         } else {
//             Vec::new()
//         };

//         let values = if hidp_caps.NumberInputValueCaps > 0 {
//             let mut value_num = hidp_caps.NumberInputValueCaps;
//             let mut values = allocate_buffer::<HIDP_VALUE_CAPS>(value_num as usize);

//             HidP_GetValueCaps(
//                 HidP_Input,
//                 values.as_mut_ptr(),
//                 &mut value_num,
//                 pre_data_buf_ptr,
//             )?;

//             // see https://www.usb.org/document-library/hid-usage-tables-14
//             for val in values.iter() {
//                 println!("Report Count: {}", val.ReportCount);
//                 match (val.UsagePage, val.Anonymous.Range.UsageMin) {
//                     (HID_USAGE_PAGE_GENERIC, 0x30) => println!(
//                         "Value: XAxis {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x31) => println!(
//                         "Value: YAxis {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x32) => println!(
//                         "Value: ZAxis {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x33) => println!(
//                         "Value: RXAxis {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x34) => println!(
//                         "Value: RYAxis {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x35) => println!(
//                         "Value: RZAxis {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x36) => println!(
//                         "Value: Slider {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (HID_USAGE_PAGE_GENERIC, 0x39) => println!(
//                         "Value: Hat {:?} {:?} [{}, {}] [{}, {}]",
//                         val.IsAbsolute,
//                         val.HasNull,
//                         val.LogicalMin,
//                         val.LogicalMax,
//                         val.PhysicalMin,
//                         val.PhysicalMax
//                     ),
//                     (usage_page, usage_id) if usage_page >= 0xff00 => {
//                         println!("Vendor Defined {} {}", usage_page, usage_id,)
//                     }
//                     other => println!("Unknown Value {:?}", other),
//                 }
//             }

//             values
//         } else {
//             Vec::new()
//         };

//         Ok(Some(Self {
//             dev,
//             name,
//             info: dev_info.Anonymous.hid,
//         }))
//     }
// }
