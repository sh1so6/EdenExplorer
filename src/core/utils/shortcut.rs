//! Windows shell shortcut (`.lnk`) helpers.
//!
//! Only used when the user opens an item (double-click, Enter, tag panel
//! click), never per frame.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, GetDriveTypeW, WIN32_FIND_DATAW,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::System::WindowsProgramming::DRIVE_REMOTE;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{Interface, PCWSTR};

/// Returns `true` when `path` has a `.lnk` extension (case-insensitive).
pub fn is_shortcut_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

/// If `path` is a `.lnk` shortcut whose target is a file-system folder,
/// returns that folder. Returns `None` for every other case (not a shortcut,
/// target is a file or program, non-file-system target such as a Control
/// Panel item, unreadable or broken shortcut); callers then let the shell
/// open the shortcut itself.
///
/// The shortcut is read with `IShellLinkW::GetPath` only. `Resolve` is never
/// called, so there is no link-tracking search, no network access and no UI.
/// Local targets are confirmed with one attribute lookup. Remote targets
/// (UNC paths and network drives) are not touched on the calling thread: the
/// folder attribute stored in the shortcut is trusted instead.
///
/// Requires COM to be initialized on the calling thread (the UI thread is an
/// STA).
pub fn resolve_shortcut_directory(path: &Path) -> Option<PathBuf> {
    if !is_shortcut_path(path) {
        return None;
    }

    let (target, stored_attributes) = read_shortcut_target(path)?;
    // The attributes the target had when the shortcut was saved. Zero means
    // the shortcut does not record them.
    let stored_is_dir = stored_attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    if stored_attributes != 0 && !stored_is_dir {
        return None;
    }

    if is_remote_path(&target) {
        return stored_is_dir.then_some(target);
    }

    target.is_dir().then_some(target)
}

/// Reads the target path and the stored target attributes of a `.lnk` file.
fn read_shortcut_target(path: &Path) -> Option<(PathBuf, u32)> {
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(PCWSTR(wide_path.as_ptr()), STGM_READ).ok()?;

        // Long paths in shortcuts are rare, but `GetPath` truncates silently,
        // so leave room beyond MAX_PATH.
        let mut buffer = vec![0u16; 32 * 1024];
        let mut find_data = WIN32_FIND_DATAW::default();
        // Flags 0: expanded path, no short (8.3) form. `GetPath` returns S_FALSE
        // with an empty buffer when the target is not a file-system object.
        link.GetPath(&mut buffer, &mut find_data, 0).ok()?;

        let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        if length == 0 {
            return None;
        }
        let target = PathBuf::from(OsString::from_wide(&buffer[..length]));
        target
            .is_absolute()
            .then_some((target, find_data.dwFileAttributes))
    }
}

/// `true` for UNC paths and paths on mapped network drives. Determined from
/// the path text and `GetDriveTypeW`, which does not contact the server.
fn is_remote_path(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(r"\\") {
        // `\\?\C:\...` is a local verbatim path; `\\?\UNC\...` and
        // `\\server\share` are remote.
        return match text.strip_prefix(r"\\?\") {
            Some(rest) => rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(r"UNC\"),
            None => true,
        };
    }

    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            let root: Vec<u16> = format!("{drive}:\\")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_REMOTE }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize, IPersistFile,
    };
    use windows::core::HSTRING;

    struct ComGuard;
    impl ComGuard {
        fn new() -> Self {
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .expect("CoInitializeEx");
            }
            Self
        }
    }
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("eden-shortcut-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_shortcut(link: &Path, target: &Path) {
        unsafe {
            let shell_link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            shell_link
                .SetPath(&HSTRING::from(target.as_os_str()))
                .unwrap();
            let persist: IPersistFile = shell_link.cast().unwrap();
            persist
                .Save(&HSTRING::from(link.as_os_str()), true)
                .unwrap();
        }
    }

    #[test]
    fn detects_shortcut_extension() {
        assert!(is_shortcut_path(Path::new(r"C:\a\b.lnk")));
        assert!(is_shortcut_path(Path::new(r"C:\a\b.LNK")));
        assert!(!is_shortcut_path(Path::new(r"C:\a\b.txt")));
        assert!(!is_shortcut_path(Path::new(r"C:\a\lnk")));
    }

    #[test]
    fn remote_path_detection_from_text() {
        assert!(is_remote_path(Path::new(r"\\server\share\dir")));
        assert!(is_remote_path(Path::new(r"\\?\UNC\server\share")));
        assert!(!is_remote_path(Path::new(r"\\?\C:\Windows")));
        assert!(!is_remote_path(Path::new("relative")));
    }

    #[test]
    fn resolves_folder_shortcuts_only() {
        let _com = ComGuard::new();
        let dir = scratch_dir("resolve");
        let folder = dir.join("target folder");
        let file = dir.join("note.txt");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&file, b"hi").unwrap();

        let folder_link = dir.join("folder.lnk");
        let file_link = dir.join("file.lnk");
        let program_link = dir.join("program.lnk");
        create_shortcut(&folder_link, &folder);
        create_shortcut(&file_link, &file);
        create_shortcut(
            &program_link,
            &PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32")
                .join("cmd.exe"),
        );

        assert_eq!(
            resolve_shortcut_directory(&folder_link),
            Some(folder.clone())
        );
        assert_eq!(resolve_shortcut_directory(&file_link), None);
        assert_eq!(resolve_shortcut_directory(&program_link), None);
        // Not a shortcut at all, and a missing .lnk file.
        assert_eq!(resolve_shortcut_directory(&file), None);
        assert_eq!(resolve_shortcut_directory(&dir.join("missing.lnk")), None);

        // Broken: the folder target was removed after the shortcut was saved.
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(resolve_shortcut_directory(&folder_link), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
