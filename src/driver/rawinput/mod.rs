use std::ffi::c_void;
use std::mem::size_of;

use windows::{
    core::{Error as wError, HSTRING, PCWSTR},
    w,
    Win32::{
        Foundation::{
            GetLastError, ERROR_INSUFFICIENT_BUFFER, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::{
                GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices,
                RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAW_INPUT_DEVICE_INFO_COMMAND, RIDEV_INPUTSINK,
                RIDI_DEVICEINFO, RIDI_DEVICENAME, RIDI_PREPARSEDDATA, RIM_TYPEHID,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, CW_USEDEFAULT,
                HWND_MESSAGE, WNDCLASSEXW, WNDCLASS_STYLES,
            },
        },
    },
};

use crate::Result;

const NULL: *const c_void = 0 as *const c_void;
const RAW_DEV_LIST_NUM: usize = 16;
const FAIL: u32 = -1i32 as u32;

#[inline]
unsafe fn get_last_err<S: AsRef<str>>(reason: S) -> wError {
    let hres = GetLastError().to_hresult();
    wError::new(hres, reason.as_ref().into())
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
                usUsagePage: 0x0001,
                usUsage: 0x0004,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: self.hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x0001,
                usUsage: 0x0005,
                dwFlags: RIDEV_INPUTSINK,
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
        }

        let joysticks = dev_list
            .into_iter()
            .take(dev_num as usize)
            .filter_map(|dev| {
                Joystick::from(dev)
                    .map_err(|e| println!("init joystick for {:?}: {:?}", dev.hDevice, e))
                    .ok()
            })
            .collect();

        Ok(joysticks)
    }
}

unsafe fn get_device_info<T>(
    hdl: HANDLE,
    cmd: RAW_INPUT_DEVICE_INFO_COMMAND,
    buf: &mut [T],
) -> Result<usize> {
    let buf_cap = buf.len() as u32;
    let mut buf_size = buf.len() as u32;
    match GetRawInputDeviceInfoW(
        hdl,
        cmd,
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut buf_size,
    ) {
        FAIL => Err(format!("insufficient buf for {:?}, {} required", cmd, buf_size).into()),

        num if num <= buf_cap => Ok(num as usize),

        other => Err(get_last_err(format!("unexpected ret {} for {:?}", other, cmd)).into()),
    }
}

pub struct Joystick {
    name: HSTRING,
}

impl Joystick {
    fn from(dev: RAWINPUTDEVICELIST) -> Result<Self> {
        if dev.dwType != RIM_TYPEHID {
            return Err(format!("unexpected raw input device type {:?}", dev.dwType).into());
        }

        let mut name_buf = [0u16; 1024];
        let name_buf_used =
            unsafe { get_device_info(dev.hDevice, RIDI_DEVICENAME, name_buf.as_mut_slice())? };

        let name = HSTRING::from_wide(&name_buf[..name_buf_used])?;

        Ok(Self { name })
    }
}
