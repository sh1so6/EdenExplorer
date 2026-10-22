//! Dark native popup menus that follow Eden's theme.
//!
//! Win32 popup menus (the native shell context menu shown with
//! `TrackPopupMenuEx`) are drawn light unless the process opts into dark
//! mode through undocumented `uxtheme.dll` exports (by ordinal only):
//!
//! - `#135 SetPreferredAppMode(mode)` (Windows 10 1903 / build 18362+;
//!   on 1809 the same ordinal is `AllowDarkModeForApp(BOOL)`, older builds
//!   have something else there, so it is only called on 18362+),
//! - `#104 RefreshImmersiveColorPolicyState()`,
//! - `#136 FlushMenuThemes()` (drops the cached menu theme so the next menu
//!   uses the new mode).
//!
//! The mode is process-wide, so it covers every popup menu of the process
//! (shell context menu, Eden's submenu, the window's system menu), without any
//! per-window call. It can be switched at runtime; the next menu opened uses
//! the new mode. Any missing export or an unsupported build makes this a no-op.
//!
//! Nothing is loaded or called until the theme is dark for the first time.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

use crate::gui::theme::ThemePalette;

/// `PreferredAppMode` values of `SetPreferredAppMode`.
const MODE_DEFAULT: i32 = 0;
const MODE_FORCE_DARK: i32 = 2;

/// First build with `SetPreferredAppMode` at ordinal 135 (Windows 10 1903).
const MIN_BUILD: u32 = 18362;

type SetPreferredAppModeFn = unsafe extern "system" fn(i32) -> i32;
type VoidFn = unsafe extern "system" fn();

struct UxTheme {
    set_preferred_app_mode: SetPreferredAppModeFn,
    refresh_immersive_color_policy_state: Option<VoidFn>,
    flush_menu_themes: VoidFn,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[repr(C)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd_version: [u16; 128],
}

#[repr(C)]
struct HighContrastW {
    size: u32,
    flags: u32,
    default_scheme: *mut u16,
}

const HCF_HIGHCONTRASTON: u32 = 1;

/// Last mode passed to `SetPreferredAppMode` (-1: none yet).
static APPLIED_MODE: AtomicI32 = AtomicI32::new(-1);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows build number (`RtlGetVersion` is not affected by manifests).
fn windows_build() -> Option<u32> {
    type RtlGetVersionFn = unsafe extern "system" fn(*mut OsVersionInfoW) -> i32;
    unsafe {
        let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let proc = GetProcAddress(ntdll, c"RtlGetVersion".as_ptr().cast());
        if proc.is_null() {
            return None;
        }
        let rtl_get_version: RtlGetVersionFn = std::mem::transmute(proc);
        let mut info: OsVersionInfoW = std::mem::zeroed();
        info.size = std::mem::size_of::<OsVersionInfoW>() as u32;
        (rtl_get_version(&mut info) >= 0 && info.major == 10).then_some(info.build)
    }
}

fn uxtheme() -> Option<&'static UxTheme> {
    static UXTHEME: OnceLock<Option<UxTheme>> = OnceLock::new();
    UXTHEME
        .get_or_init(|| {
            if windows_build()? < MIN_BUILD {
                return None;
            }
            unsafe {
                let module = LoadLibraryW(wide("uxtheme.dll").as_ptr());
                if module.is_null() {
                    return None;
                }
                // MAKEINTRESOURCEA(ordinal)
                let ordinal = |n: usize| {
                    let proc = GetProcAddress(module, n as *const u8);
                    (!proc.is_null()).then_some(proc)
                };
                Some(UxTheme {
                    set_preferred_app_mode: std::mem::transmute::<*mut c_void, SetPreferredAppModeFn>(
                        ordinal(135)?,
                    ),
                    refresh_immersive_color_policy_state: ordinal(104)
                        .map(|proc| std::mem::transmute::<*mut c_void, VoidFn>(proc)),
                    flush_menu_themes: std::mem::transmute::<*mut c_void, VoidFn>(ordinal(136)?),
                })
            }
        })
        .as_ref()
}

fn high_contrast() -> bool {
    let mut info = HighContrastW {
        size: std::mem::size_of::<HighContrastW>() as u32,
        flags: 0,
        default_scheme: std::ptr::null_mut(),
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.size,
            Some(&mut info as *mut HighContrastW as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
        && info.flags & HCF_HIGHCONTRASTON != 0
}

/// Whether `[r, g, b]` (sRGB) is a dark background: relative luminance below
/// that of mid grey (L* 50).
pub fn is_dark_rgb([r, g, b]: [u8; 3]) -> bool {
    let linear = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b) < 0.184
}

/// Makes native popup menus dark when the palette's background is dark.
/// Cheap to call on every theme change; only mode changes reach uxtheme.
pub fn sync_with_palette(palette: &ThemePalette) {
    let bg = palette.application_bg_color;
    sync(is_dark_rgb([bg.r(), bg.g(), bg.b()]));
}

/// Dark (`ForceDark`) or default (light) native popup menus for the whole
/// process; the next menu opened uses the mode. High contrast keeps the
/// default (only re-checked on the next theme change). No-op on unsupported
/// Windows builds.
pub fn sync(dark: bool) {
    // Default (not ForceLight) for light: identical to not opting in at all.
    let mode = if dark && !high_contrast() {
        MODE_FORCE_DARK
    } else {
        MODE_DEFAULT
    };
    // Nothing to undo before the first switch to dark, so a light theme never
    // loads uxtheme.
    let previous = APPLIED_MODE.swap(mode, Ordering::Relaxed);
    if previous == mode || (previous == -1 && mode == MODE_DEFAULT) {
        return;
    }
    let Some(uxtheme) = uxtheme() else {
        return;
    };
    unsafe {
        (uxtheme.set_preferred_app_mode)(mode);
        if let Some(refresh) = uxtheme.refresh_immersive_color_policy_state {
            refresh();
        }
        (uxtheme.flush_menu_themes)();
    }
}

#[cfg(test)]
mod tests {
    use super::is_dark_rgb;

    #[test]
    fn default_palette_backgrounds() {
        assert!(is_dark_rgb([20, 22, 26]));
        assert!(!is_dark_rgb([245, 245, 245]));
        assert!(!is_dark_rgb([128, 128, 128]));
        assert!(is_dark_rgb([100, 100, 100]));
    }
}
