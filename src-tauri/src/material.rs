//! Native desktop acrylic for Windows 11 22H2 and later.
//! Every Win32 call is checked; unsupported systems retain the ordinary UI.
use serde::Serialize;
use std::ffi::c_void;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialResult {
    pub active: bool,
    pub supported: bool,
}

#[repr(C)]
struct OsVersion {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(version: *mut OsVersion) -> i32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmIsCompositionEnabled(enabled: *mut i32) -> i32;
    fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const c_void, size: u32) -> i32;
}

fn supported_version(major: u32, build: u32) -> bool {
    major >= 10 && build >= 22621
}

fn supported() -> bool {
    let mut version: OsVersion = unsafe { std::mem::zeroed() };
    version.size = std::mem::size_of::<OsVersion>() as u32;
    let mut composition = 0;
    unsafe {
        RtlGetVersion(&mut version) >= 0
            && supported_version(version.major, version.build)
            && DwmIsCompositionEnabled(&mut composition) >= 0
            && composition != 0
    }
}

fn set_attribute(hwnd: isize, attribute: u32, value: i32) -> Result<(), String> {
    let result = unsafe {
        DwmSetWindowAttribute(hwnd, attribute, &value as *const i32 as *const c_void, 4)
    };
    if result < 0 {
        Err(format!("Windows 窗口效果调用失败（0x{:08X}）", result as u32))
    } else {
        Ok(())
    }
}

pub fn apply(hwnd: isize, enabled: bool, dark: bool) -> Result<MaterialResult, String> {
    if !supported() {
        return Ok(MaterialResult { active: false, supported: false });
    }
    // DWMWA_SYSTEMBACKDROP_TYPE: NONE=1, TRANSIENTWINDOW (desktop acrylic)=3.
    if !enabled {
        set_attribute(hwnd, 38, 1)?;
        let _ = set_attribute(hwnd, 33, 1);
        return Ok(MaterialResult { active: false, supported: true });
    }
    // Match the app theme rather than assuming the Windows theme is identical.
    set_attribute(hwnd, 20, i32::from(dark))?;
    if let Err(error) = set_attribute(hwnd, 38, 3) {
        let _ = set_attribute(hwnd, 38, 1);
        return Err(error);
    }
    // Corner preference is cosmetic and may be ignored for some window styles.
    let _ = set_attribute(hwnd, 33, 2);
    Ok(MaterialResult { active: true, supported: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_acrylic_requires_windows_11_22h2() {
        assert!(!supported_version(10, 19045));
        assert!(!supported_version(10, 22000));
        assert!(!supported_version(10, 22620));
        assert!(supported_version(10, 22621));
        assert!(supported_version(10, 26100));
    }

    #[test]
    fn invalid_window_never_reports_active_acrylic() {
        assert!(!matches!(apply(0, true, false), Ok(MaterialResult { active: true, .. })));
    }

    #[test]
    fn native_window_can_enable_and_disable_acrylic_or_report_unsupported() {
        #[link(name = "user32")]
        extern "system" {
            fn CreateWindowExW(ex_style: u32, class: *const u16, title: *const u16,
                style: u32, x: i32, y: i32, width: i32, height: i32,
                parent: isize, menu: isize, instance: isize, param: *const c_void) -> isize;
            fn DestroyWindow(hwnd: isize) -> i32;
        }
        struct TestWindow(isize);
        impl Drop for TestWindow {
            fn drop(&mut self) { unsafe { DestroyWindow(self.0); } }
        }
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "Acrylic CI probe\0".encode_utf16().collect();
        let window = TestWindow(unsafe {
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0x80000000,
                0, 0, 100, 100, 0, 0, 0, std::ptr::null())
        });
        assert_ne!(window.0, 0, "Native test window creation failed");
        let enabled = apply(window.0, true, false).expect("Enable native acrylic");
        assert_eq!(enabled.active, enabled.supported);
        let dark = apply(window.0, true, true).expect("Switch native tint to dark");
        assert_eq!(dark.active, enabled.active);
        let disabled = apply(window.0, false, false).expect("Disable native acrylic");
        assert!(!disabled.active);
    }
}
