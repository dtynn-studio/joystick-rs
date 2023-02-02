use std::ffi::c_void;
use std::mem::size_of;

use windows::{
    core::{Error as wError, HSTRING, PCWSTR},
    w,
    Win32::{
        Devices::HumanInterfaceDevice::{
            HidP_GetButtonCaps, HidP_GetCaps, HidP_GetValueCaps, HidP_Input, HIDP_BUTTON_CAPS,
            HIDP_CAPS, HIDP_VALUE_CAPS, HID_USAGE_GENERIC_GAMEPAD, HID_USAGE_GENERIC_JOYSTICK,
            HID_USAGE_GENERIC_MOUSE, HID_USAGE_PAGE_GENERIC,
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
                CW_USEDEFAULT, HWND_MESSAGE, MSG, WM_INPUT, WM_INPUT_DEVICE_CHANGE, WNDCLASSEXW,
                WNDCLASS_STYLES,
            },
        },
    },
};

use crate::Result;

const NULL: *const c_void = 0 as *const c_void;

const RAW_DEV_LIST_NUM: usize = 32;
const FAIL: u32 = -1i32 as u32;

#[inline]
unsafe fn get_last_err<S: AsRef<str>>(reason: S) -> wError {
    let hres = GetLastError().to_hresult();
    wError::new(
        hres,
        format!("{}: {}", reason.as_ref(), hres.message()).into(),
    )
}

unsafe extern "system" fn window_proc_sys(
    param0: HWND,
    param1: u32,
    param2: WPARAM,
    param3: LPARAM,
) -> LRESULT {
    DefWindowProcW(param0, param1, param2, param3)
}

pub struct Manager {
    hwnd: HWND,
}

impl Drop for Manager {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

impl Manager {
    pub fn new(include_xbox: bool) -> Result<Self> {
        let classname = w!("joystick-rs rawinput");
        unsafe {
            let hinstance = GetModuleHandleW(None)?;

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
                lpszClassName: classname,
                hIconSm: None.into(),
            };

            if RegisterClassExW(&wclass) == 0 {
                return Err(get_last_err("get zero from RegisterClassExW").into());
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
                return Err(get_last_err("get null from CreateWindowExW").into());
            }

            println!("window {:?} constructed", hwnd);

            let mut mgr = Self { hwnd };
            mgr.register_devices(include_xbox)?;

            Ok(mgr)
        }
    }

    unsafe fn register_devices(&mut self, include_xbox: bool) -> Result<()> {
        // https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/hid-architecture#hid-clients-supported-in-windows
        // TODO: RIDEV_DEVNOTIFY
        let devices = [
            RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_JOYSTICK,
                dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                hwndTarget: self.hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_GAMEPAD,
                dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                hwndTarget: self.hwnd,
            },
        ];

        RegisterRawInputDevices(
            if include_xbox {
                &devices[..]
            } else {
                &devices[..1]
            },
            size_of::<RAWINPUTDEVICE>() as u32,
        )
        .ok()?;

        Ok(())
    }

    pub fn list_devices(&mut self) -> Result<Vec<Joystick>> {
        let mut dev_list: [RAWINPUTDEVICELIST; RAW_DEV_LIST_NUM] = Default::default();
        const DEV_NUM_MAX: u32 = RAW_DEV_LIST_NUM as u32;
        let mut dev_num = dev_list.len() as u32;

        unsafe {
            match GetRawInputDeviceList(
                Some(dev_list.as_mut_ptr()),
                &mut dev_num,
                size_of::<RAWINPUTDEVICELIST>() as u32,
            ) {
                count @ 0..=DEV_NUM_MAX => {
                    dev_num = count;
                }

                code if code == ERROR_INSUFFICIENT_BUFFER.0 => {
                    return Err(wError::new(
                        ERROR_INSUFFICIENT_BUFFER.to_hresult(),
                        format!(
                            "insufficient buffer for {} devices from GetRawInputDeviceList",
                            dev_num
                        )
                        .into(),
                    )
                    .into())
                }

                FAIL => return Err(get_last_err("GetRawInputDeviceList").into()),

                other => {
                    return Err(format!(
                        "unexpected return value {} from GetRawInputDeviceList",
                        other
                    )
                    .into())
                }
            };

            let joysticks = dev_list
                .into_iter()
                .take(dev_num as usize)
                .filter_map(|dev| {
                    Joystick::from(dev)
                        .map_err(|e| println!("init joystick for {:?}: {:?}", dev.hDevice, e))
                        .ok()
                        .unwrap_or(None)
                })
                .collect();

            Ok(joysticks)
        }
    }

    pub fn on_message(&mut self) -> Result<()> {
        unsafe {
            let mut msg = MSG::default();
            if GetMessageW(&mut msg, self.hwnd, 0, 0).as_bool() {
                println!(
                    "get msg: {}: {:?}, {:?}",
                    msg.message, msg.wParam, msg.lParam
                );
            };
        }

        Ok(())
    }
}

unsafe fn get_device_info<T>(
    hdl: HANDLE,
    cmd: RAW_INPUT_DEVICE_INFO_COMMAND,
    buf: *mut T,
    space: Option<usize>,
) -> Result<usize> {
    let buf_cap = space.unwrap_or(size_of::<T>()) as u32;
    let mut buf_size = buf_cap;
    match GetRawInputDeviceInfoW(hdl, cmd, Some(buf as *mut c_void), &mut buf_size) {
        FAIL => Err(format!("insufficient buf for {:?}, {} required", cmd, buf_size).into()),

        num if num <= buf_cap => Ok(num as usize),

        other => Err(get_last_err(format!("unexpected ret {} for {:?}", other, cmd)).into()),
    }
}

unsafe fn get_device_info_size(hdl: HANDLE, cmd: RAW_INPUT_DEVICE_INFO_COMMAND) -> Result<usize> {
    let mut size = 0u32;
    match GetRawInputDeviceInfoW(hdl, cmd, None, &mut size) {
        SUCCESS => Ok(size as usize),
        other => Err(get_last_err(format!("unexpected ret {} for {:?}", other, cmd)).into()),
    }
}

#[inline]
fn allocate_buffer<T: Default + Clone>(cap: usize) -> Vec<T> {
    let buf = vec![T::default(); cap];
    buf
}

#[inline]
fn is_hid_joystick(info: &RID_DEVICE_INFO_HID) -> bool {
    info.usUsagePage == HID_USAGE_PAGE_GENERIC
        && (info.usUsage == HID_USAGE_GENERIC_JOYSTICK || info.usUsage == HID_USAGE_GENERIC_GAMEPAD)
}

#[derive(Debug)]
pub struct Joystick {
    dev: RAWINPUTDEVICELIST,
    name: HSTRING,
    info: RID_DEVICE_INFO_HID,
}

impl Joystick {
    unsafe fn from(dev: RAWINPUTDEVICELIST) -> Result<Option<Self>> {
        if dev.dwType != RIM_TYPEHID {
            return Ok(None);
        }

        let mut name_buf = [0u16; 1024];
        let name_buf_size = name_buf.len();
        let name_buf_used = get_device_info(
            dev.hDevice,
            RIDI_DEVICENAME,
            name_buf.as_mut_ptr(),
            Some(name_buf_size),
        )?;

        let name = HSTRING::from_wide(&name_buf[..name_buf_used])?;

        let mut dev_info = RID_DEVICE_INFO {
            cbSize: size_of::<RID_DEVICE_INFO>() as u32,
            ..Default::default()
        };

        get_device_info(dev.hDevice, RIDI_DEVICEINFO, &mut dev_info, None)?;
        if !is_hid_joystick(&dev_info.Anonymous.hid) {
            return Ok(None);
        }

        // get pre parsed data
        let pre_data_size = get_device_info_size(dev.hDevice, RIDI_PREPARSEDDATA)?;
        let mut pre_data_buf = allocate_buffer::<u8>(pre_data_size);
        get_device_info(
            dev.hDevice,
            RIDI_PREPARSEDDATA,
            pre_data_buf.as_mut_ptr(),
            Some(pre_data_size),
        )?;

        let pre_data_buf_ptr = pre_data_buf.as_ptr() as isize;

        let mut hidp_caps = HIDP_CAPS::default();
        HidP_GetCaps(pre_data_buf_ptr, &mut hidp_caps)?;

        println!("Caps: {:?}", hidp_caps);

        let buttons = if hidp_caps.NumberInputButtonCaps > 0 {
            let mut button_num = hidp_caps.NumberInputButtonCaps;
            let mut buttons = allocate_buffer::<HIDP_BUTTON_CAPS>(button_num as usize);

            HidP_GetButtonCaps(
                HidP_Input,
                buttons.as_mut_ptr(),
                &mut button_num,
                pre_data_buf_ptr,
            )?;

            println!(
                "Buttons [{}, {}]",
                buttons[0].Anonymous.Range.UsageMin, buttons[0].Anonymous.Range.UsageMax
            );

            buttons
        } else {
            Vec::new()
        };

        let values = if hidp_caps.NumberInputValueCaps > 0 {
            let mut value_num = hidp_caps.NumberInputValueCaps;
            let mut values = allocate_buffer::<HIDP_VALUE_CAPS>(value_num as usize);

            HidP_GetValueCaps(
                HidP_Input,
                values.as_mut_ptr(),
                &mut value_num,
                pre_data_buf_ptr,
            )?;

            // see https://www.usb.org/document-library/hid-usage-tables-14
            for val in values.iter() {
                println!("Report Count: {}", val.ReportCount);
                match (val.UsagePage, val.Anonymous.Range.UsageMin) {
                    (HID_USAGE_PAGE_GENERIC, 0x30) => println!(
                        "Value: XAxis {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x31) => println!(
                        "Value: YAxis {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x32) => println!(
                        "Value: ZAxis {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x33) => println!(
                        "Value: RXAxis {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x34) => println!(
                        "Value: RYAxis {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x35) => println!(
                        "Value: RZAxis {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x36) => println!(
                        "Value: Slider {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (HID_USAGE_PAGE_GENERIC, 0x39) => println!(
                        "Value: Hat {:?} {:?} [{}, {}] [{}, {}]",
                        val.IsAbsolute,
                        val.HasNull,
                        val.LogicalMin,
                        val.LogicalMax,
                        val.PhysicalMin,
                        val.PhysicalMax
                    ),
                    (usage_page, usage_id) if usage_page >= 0xff00 => {
                        println!("Vendor Defined {} {}", usage_page, usage_id,)
                    }
                    other => println!("Unknown Value {:?}", other),
                }
            }

            values
        } else {
            Vec::new()
        };

        Ok(Some(Self {
            dev,
            name,
            info: dev_info.Anonymous.hid,
        }))
    }
}
