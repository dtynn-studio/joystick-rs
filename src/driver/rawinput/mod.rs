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
            HidP_GetButtonCaps, HidP_GetCaps, HidP_GetData, HidP_GetValueCaps, HidP_Input,
            HIDP_BUTTON_CAPS, HIDP_CAPS, HIDP_DATA, HIDP_VALUE_CAPS, HID_USAGE_GENERIC_GAMEPAD,
            HID_USAGE_GENERIC_HATSWITCH, HID_USAGE_GENERIC_JOYSTICK, HID_USAGE_GENERIC_RX,
            HID_USAGE_GENERIC_RY, HID_USAGE_GENERIC_RZ, HID_USAGE_GENERIC_SLIDER,
            HID_USAGE_GENERIC_X, HID_USAGE_GENERIC_Y, HID_USAGE_GENERIC_Z, HID_USAGE_PAGE_GENERIC,
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

use crate::driver::{AxisType, ButtonState, HatState, Manager};

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
type Event = crate::driver::Event<DevIdent, Value, ValueRange>;
type DeviceInfo = crate::driver::DeviceInfo<ValueRange>;
type DeviceSpecs = crate::driver::DeviceSpecs<ValueRange>;
type DeviceObjectStates = crate::driver::DeviceObjectStates<Value>;

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
        let (hwnd_tx, hwnd_rx) = bounded(1);
        let (tx, rx) = match self.event_buf_size {
            Some(size) => bounded(size),
            None => unbounded(),
        };

        unsafe {
            let join = spawn(move || {
                let hwnd = match setup_message_window() {
                    Ok(h) => {
                        _ = hwnd_tx.send(Ok(h));
                        h
                    }

                    err @ Err(_) => {
                        _ = hwnd_tx.send(err);
                        return;
                    }
                };

                let tx = tx;
                let _span = warn_span!("event loop", ?hwnd).entered();
                debug!("start");
                if let Err(e) = start_event_loop(hwnd, self, &tx) {
                    _ = tx.send(Err(e));
                };
                debug!("stop");
            });

            let hwnd = hwnd_rx
                .recv()
                .context("get hwnd from spawned thread")?
                .context("construct hwnd")?;

            let notifier = Mgr {
                rx,
                ctx: Some((hwnd, join)),
            };

            Ok(notifier)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DeviceObjectIndex {
    Button(usize),
    Axis(usize),
    Slider(usize),
    Hat(usize),
    Unknown(u16, u16),
}

impl DeviceObjectIndex {
    #[inline]
    fn is_unknown(&self) -> bool {
        matches!(self, DeviceObjectIndex::Unknown(_, _))
    }
}

type DevObjectsMapping = HashMap<u16, DeviceObjectIndex>;

#[derive(Debug)]
struct DevInfo {
    name: HSTRING,
    pre_parsed_data: Vec<u8>,
    // max_data_count: u32,
    // hidp_caps: HIDP_CAPS,
    buttons: usize,
    axises: Vec<AxisType>,
    sliders: usize,
    hats: usize,
    mapping: DevObjectsMapping,
}

impl DevInfo {
    fn replace_mapping(&mut self, data_idx: u16, obj_idx: DeviceObjectIndex) {
        let new_is_unknown = obj_idx.is_unknown();
        if let Some(prev) = self.mapping.insert(data_idx, obj_idx) {
            if new_is_unknown && prev.is_unknown() {
                trace!(di=data_idx, ?prev, next = ?obj_idx, "found duplicate data index");
            } else {
                warn!(di=data_idx, ?prev, next = ?obj_idx, "found duplicate data index");
            }
        }
    }
}

#[derive(Debug)]
pub struct Mgr {
    rx: Receiver<Result<Event>>,
    ctx: Option<(HWND, JoinHandle<()>)>,
}

impl Mgr {
    fn shutdown(&mut self) -> Result<()> {
        if let Some((hwnd, join)) = self.ctx.take() {
            unsafe {
                if !PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).as_bool() {
                    warn!("failed to destory window: {:?}", get_last_err());
                };
            }

            _ = join.join();
        }

        Ok(())
    }
}

impl Drop for Mgr {
    fn drop(&mut self) {
        _ = self.shutdown();
    }
}

impl Manager for Mgr {
    type DeviceIdent = DevIdent;
    type Value = Value;
    type ValueRange = ValueRange;

    fn as_event_receiver(&self) -> &Receiver<Result<Event>> {
        &self.rx
    }

    fn close(mut self) -> Result<()> {
        self.shutdown()
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

    trace!(devs=?typ,"registered");

    Ok(())
}

unsafe fn proc_input_message(
    devices: &HashMap<isize, DevInfo>,
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<Event> {
    let is_sink = match wparam.0 as u32 {
        RIM_INPUT => false,

        RIM_INPUTSINK => true,

        _other => return Err(anyhow!("unexpected wparam {:?} for input message", wparam)),
    };

    let res = fetch_input_data(devices, lparam);

    if !is_sink {
        // TODO: handle the result?
        DefWindowProcW(hwnd, WM_INPUT, wparam, lparam);
    }

    res.map(|(hdl, states)| Event::DeviceState {
        ident: hdl,
        is_sink,
        states,
    })
}

unsafe fn get_raw_input_data(lparam: LPARAM) -> Result<RAWINPUT> {
    let mut raw_data_size = 0u32;
    if GetRawInputData(
        HRAWINPUT(lparam.0),
        RID_INPUT,
        None,
        &mut raw_data_size,
        size_of::<RAWINPUTHEADER>() as u32,
    ) != 0
    {
        return Err(get_last_err()).context("get raw data size by calling GetRawInputData");
    };

    let mut raw_data = RAWINPUT::default();
    if GetRawInputData(
        HRAWINPUT(lparam.0),
        RID_INPUT,
        Some(&mut raw_data as *mut RAWINPUT as *mut c_void),
        &mut raw_data_size,
        size_of::<RAWINPUTHEADER>() as u32,
    ) == FAIL
    {
        return Err(get_last_err()).context("GetRawInputData");
    };

    Ok(raw_data)
}

unsafe fn fetch_input_data(
    devices: &HashMap<isize, DevInfo>,
    lparam: LPARAM,
) -> Result<(isize, DeviceObjectStates)> {
    let mut raw_data = get_raw_input_data(lparam)?;

    if raw_data.header.dwType != RIM_TYPEHID.0 {
        return Err(anyhow!(
            "unexpected dwType {} in RAWINPUT.header",
            raw_data.header.dwType
        ));
    }

    let hdev = raw_data.header.hDevice.0;
    let dev_info = devices
        .get(&hdev)
        .ok_or_else(|| anyhow!("device info for {} not found", lparam.0))?;

    let mut data_list = allocate_buffer::<HIDP_DATA>(raw_data.data.hid.dwCount as usize);
    let mut data_len = data_list.len() as u32;

    let b_raw_data_size = (raw_data.data.hid.dwCount * raw_data.data.hid.dwSizeHid) as usize;
    let b_raw =
        std::slice::from_raw_parts_mut(raw_data.data.hid.bRawData.as_mut_ptr(), b_raw_data_size);

    HidP_GetData(
        HidP_Input,
        data_list.as_mut_ptr(),
        &mut data_len,
        dev_info.pre_parsed_data.as_ptr() as isize,
        b_raw,
    )
    .context("HidP_GetData")?;

    let mut states = DeviceObjectStates {
        buttons: allocate_buffer(dev_info.buttons),
        axis: allocate_buffer(dev_info.axises.len()),
        sliders: allocate_buffer(dev_info.sliders),
        hats: allocate_buffer(dev_info.hats),
    };

    for data in data_list.into_iter().take(data_len as usize) {
        let obj_idx = match dev_info.mapping.get(&data.DataIndex) {
            Some(i) => i,
            None => {
                trace!("object index not found for {}", data.DataIndex);
                continue;
            }
        };

        let _data_span = warn_span!("value data", data_idx = data.DataIndex, ?obj_idx).entered();

        match obj_idx {
            DeviceObjectIndex::Button(idx) => {
                if let Some(p) = states.buttons.get_mut(*idx) {
                    *p = if data.Anonymous.On.as_bool() {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Releaed
                    }
                } else {
                    warn!("button object not found");
                }
            }

            DeviceObjectIndex::Axis(idx) => {
                if let Some(p) = states.axis.get_mut(*idx) {
                    p.replace(data.Anonymous.RawValue as i32);
                } else {
                    warn!("axis object not found");
                }
            }

            DeviceObjectIndex::Slider(idx) => {
                if let Some(p) = states.sliders.get_mut(*idx) {
                    p.replace(data.Anonymous.RawValue as i32);
                } else {
                    warn!("slider object not found");
                }
            }

            DeviceObjectIndex::Hat(idx) => {
                if let Some(p) = states.hats.get_mut(*idx) {
                    let st = match data.Anonymous.RawValue {
                        0 => HatState::Up,
                        1 => HatState::UpRight,
                        2 => HatState::Right,
                        3 => HatState::DownRight,
                        4 => HatState::Down,
                        5 => HatState::DownLeft,
                        6 => HatState::Left,
                        7 => HatState::UpLeft,
                        _other => HatState::Null,
                    };
                    *p = st;
                } else {
                    warn!("hat object not found");
                }
            }

            DeviceObjectIndex::Unknown(_, _) => {
                trace!("ignore value data");
            }
        }
    }

    Ok((hdev, states))
}

unsafe fn proc_input_change_message(
    deivces: &mut HashMap<isize, DevInfo>,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<Event> {
    let _span = warn_span!("input change", hdl = lparam.0).entered();
    match wparam.0 as u32 {
        GIDC_ARRIVAL => {}
        GIDC_REMOVAL => {
            return {
                if deivces.remove(&lparam.0).is_none() {
                    warn!("no device found on removal");
                }

                Ok(Event::DeviceDeattached(lparam.0))
            }
        }
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

    // let max_data_count = HidP_MaxDataListLength(HidP_Input, pre_data_buf_ptr);
    // if max_data_count == 0 {
    //     return Err(anyhow!("failed to get max data count of HidP_Input"));
    // }

    let mut hidp_caps = HIDP_CAPS::default();
    HidP_GetCaps(pre_data_buf_ptr, &mut hidp_caps).context("get device caps")?;

    debug!("caps: {:?}", hidp_caps);

    let mut info = DevInfo {
        name: hname,
        pre_parsed_data: pre_data_buf,
        // max_data_count,
        // hidp_caps,
        buttons: 0,
        axises: Default::default(),
        sliders: 0,
        hats: 0,
        mapping: Default::default(),
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
                trace!(
                    di_range=?cap.Anonymous.Range.DataIndexMin..=cap.Anonymous.Range.DataIndexMax,
                    usage_range=?cap.Anonymous.Range.UsageMin..=cap.Anonymous.Range.UsageMax,
                    "ranged button cap",
                );

                for di in cap.Anonymous.Range.DataIndexMin..=cap.Anonymous.Range.DataIndexMax {
                    info.replace_mapping(di, DeviceObjectIndex::Button(info.buttons));
                    info.buttons += 1;
                }
            } else {
                trace!(
                    di = cap.Anonymous.NotRange.DataIndex,
                    usage = cap.Anonymous.NotRange.Usage,
                    "individual button cap",
                );

                info.replace_mapping(
                    cap.Anonymous.NotRange.DataIndex,
                    DeviceObjectIndex::Button(info.buttons),
                );
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
                    cap.Anonymous.NotRange.DataIndex,
                    cap.Anonymous.NotRange.Usage,
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
                    if !(cap.LogicalMin == 0 && cap.LogicalMax == 7) {
                        warn!(
                            min = cap.LogicalMin,
                            max = cap.LogicalMax,
                            "unexpected value range for hat"
                        );
                    }

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

                    let (mut vmin, mut vmax) = (cap.LogicalMin, cap.LogicalMax);
                    if vmin == 0 && vmax == -1 {
                        vmin = 0;
                        vmax = u16::MAX as i32;
                    }

                    let idx = info.axises.len();
                    info.axises.push(atyp);
                    axis_specs.push((atyp, vmin..=vmax));
                    DeviceObjectIndex::Axis(idx)
                }

                (upage, uid) => DeviceObjectIndex::Unknown(upage, uid),
            };

            trace!(di, cap.UsagePage, usage, ?item, "value cap");
            info.replace_mapping(di, item);
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
        trace!("waiting for message");
        let mut msg = MSG::default();
        match GetMessageW(&mut msg, hwnd, 0, WM_INPUT).0 {
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
            WM_CLOSE => {
                if !DestroyWindow(hwnd).as_bool() {
                    warn!("destory window: {:?}", get_last_err());
                }
                return Ok(());
            }

            WM_INPUT => {
                proc_input_message(&devices, hwnd, msg.wParam, msg.lParam).context("proc input msg")
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

        let evt = res.unwrap_or_else(Event::Warning);
        tx.send(Ok(evt)).context("event chan broken")?;
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
