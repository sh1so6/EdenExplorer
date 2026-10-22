use std::cell::{OnceCell, RefCell};
use std::mem::{ManuallyDrop, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_NORMAL, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DEFCONTEXTMENU, IContextMenu,
    IContextMenu2, IContextMenu3, ILCreateFromPathW, ILFindLastID, IShellFolder, SHBindToParent,
    SHCreateDefaultContextMenu,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU, MENUITEMINFOW,
    MFS_DEFAULT, MFS_DISABLED, MFT_OWNERDRAW, MFT_SEPARATOR, MIIM_BITMAP, MIIM_DATA, MIIM_FTYPE,
    MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};
use windows::core::{Error, HRESULT, Interface, PCSTR, PCWSTR, PWSTR};

const CONTEXT_MENU_ID_BASE: u32 = 1;
const CONTEXT_MENU_ID_MAX: u32 = 0x7FFF;

/// Debug/probe-only diagnostics. Release builds compile the call to a no-op,
/// but the arguments still count as used so no cfg juggling is needed at the
/// call sites.
#[inline(always)]
fn debug_log(args: std::fmt::Arguments<'_>) {
    #[cfg(any(debug_assertions, feature = "ui-probe"))]
    eprintln!("[shell-menu] {args}");
    #[cfg(not(any(debug_assertions, feature = "ui-probe")))]
    let _ = args;
}

/// One entry of the shell context menu, mirroring the native HMENU tree.
pub enum ShellMenuNode {
    Command {
        /// Global command id (unique across all submenus of the menu).
        id: u32,
        label: String,
        disabled: bool,
        /// The default verb (rendered bold by Explorer).
        is_default: bool,
    },
    Submenu(ShellSubmenu),
    Separator,
}

/// A cascading shell submenu whose children are read lazily.
///
/// Many shell submenus ("Send to", "Open with", "New", most third-party
/// cascades) are empty until the menu host sends `WM_INITMENUPOPUP` for them,
/// so `WM_INITMENUPOPUP` is sent (once) when the submenu is first shown.
///
/// Some of them keep filling the HMENU asynchronously afterwards (on Windows
/// 11 "Send to" first contains only "Bluetooth device" and receives the
/// SendTo folder entries and drives a few hundred ms later), so the children
/// are re-read whenever the HMENU's item count changes.
pub struct ShellSubmenu {
    pub label: String,
    pub disabled: bool,
    /// Owned by the root menu of the [`ShellContextMenu`]; destroyed with it.
    hmenu: HMENU,
    index_in_parent: u32,
    /// When `WM_INITMENUPOPUP` was sent (populated at most once).
    initialized_at: OnceCell<Instant>,
    /// Collected children and the HMENU item count they were read from.
    children: RefCell<Option<(i32, Rc<Vec<ShellMenuNode>>)>>,
}

/// How long after initialization a submenu is watched for asynchronously
/// added items (see [`ShellSubmenu::is_settling`]).
/// Generous because "Send to" also enumerates (possibly slow) network drives.
const SUBMENU_SETTLE_TIME: Duration = Duration::from_secs(10);

impl ShellSubmenu {
    fn new(label: String, disabled: bool, hmenu: HMENU, index_in_parent: u32) -> Self {
        Self {
            label,
            disabled,
            hmenu,
            index_in_parent,
            initialized_at: OnceCell::new(),
            children: RefCell::new(None),
        }
    }

    /// True when the submenu was populated, has no visible entries and the
    /// HMENU has not changed since (cheap; does not populate).
    pub fn is_known_empty(&self) -> bool {
        match &*self.children.borrow() {
            Some((count, children)) => children.is_empty() && *count == self.item_count(),
            None => false,
        }
    }

    /// True shortly after initialization, while the shell may still be adding
    /// items asynchronously. The UI keeps repainting meanwhile so new items
    /// show up without user input.
    pub fn is_settling(&self) -> bool {
        self.initialized_at
            .get()
            .is_some_and(|at| at.elapsed() < SUBMENU_SETTLE_TIME)
    }

    /// Stable per-menu key, e.g. for egui id salts.
    pub fn key(&self) -> usize {
        self.hmenu.0 as usize
    }

    fn item_count(&self) -> i32 {
        unsafe { GetMenuItemCount(Some(self.hmenu)) }
    }
}

/// `HandleMenuMsg(2)` forwarding for a shell context menu.
#[derive(Clone)]
struct MenuMsgHandler {
    cm2: Option<IContextMenu2>,
    cm3: Option<IContextMenu3>,
}

impl MenuMsgHandler {
    fn new(context_menu: &IContextMenu) -> Self {
        Self {
            cm2: context_menu.cast::<IContextMenu2>().ok(),
            cm3: context_menu.cast::<IContextMenu3>().ok(),
        }
    }

    /// Forwards a menu message. Returns the message result when the menu
    /// handled it. `IContextMenu2` is used only when `IContextMenu3` is not
    /// implemented; it cannot produce a result, so `WM_MENUCHAR` needs `3`.
    fn handle(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if let Some(cm3) = &self.cm3 {
            let mut result = LRESULT(0);
            return unsafe { cm3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)) }
                .ok()
                .map(|_| result);
        }
        if msg == WM_MENUCHAR {
            return None;
        }
        let cm2 = self.cm2.as_ref()?;
        unsafe { cm2.HandleMenuMsg(msg, wparam, lparam) }
            .ok()
            .map(|_| LRESULT(0))
    }
}

thread_local! {
    /// The shell menu currently tracked by a native `TrackPopupMenuEx` loop.
    static ACTIVE_NATIVE_MENU: RefCell<Option<MenuMsgHandler>> = const { RefCell::new(None) };
}

/// Makes `handler` the target of [`forward_native_menu_message`] until dropped.
struct ActiveNativeMenuGuard {
    previous: Option<MenuMsgHandler>,
}

impl ActiveNativeMenuGuard {
    fn install(handler: MenuMsgHandler) -> Self {
        let previous = ACTIVE_NATIVE_MENU.with(|slot| slot.replace(Some(handler)));
        Self { previous }
    }
}

impl Drop for ActiveNativeMenuGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE_NATIVE_MENU.with(|slot| {
            *slot.borrow_mut() = previous;
        });
    }
}

/// Called from the main window procedure. While a native shell context menu
/// is being tracked, forwards the owner-window menu messages that shell
/// extensions rely on (lazy submenu population, owner-drawn items, menu
/// accelerators) and returns the result to give back to Windows. Returns
/// `None` when no shell menu is active or the message was not handled, in
/// which case the caller must continue with its default handling.
pub fn forward_native_menu_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_INITMENUPOPUP | WM_MENUCHAR => {}
        // wParam is 0 when the message was sent for a menu (not a control).
        WM_DRAWITEM | WM_MEASUREITEM if wparam.0 == 0 => {}
        _ => return None,
    }

    // Clone the handler out so the RefCell is not borrowed while the shell
    // extension runs (it may pump messages and re-enter the wndproc).
    let handler =
        ACTIVE_NATIVE_MENU.with(|slot| slot.try_borrow().ok().and_then(|active| active.clone()))?;
    let result = handler.handle(msg, wparam, lparam)?;

    Some(match msg {
        WM_MENUCHAR => result,
        WM_INITMENUPOPUP => LRESULT(0),
        // WM_DRAWITEM / WM_MEASUREITEM: TRUE when processed.
        _ => LRESULT(1),
    })
}

pub struct ShellContextMenu {
    context_menu: IContextMenu,
    handler: MenuMsgHandler,
    menu: HMENU,
    nodes: Vec<ShellMenuNode>,
    id_base: u32,
    full_pidls: Vec<*mut ITEMIDLIST>,
    parent_pidl: *mut ITEMIDLIST,
}

impl ShellContextMenu {
    pub fn for_paths(paths: &[PathBuf], hwnd: HWND) -> windows::core::Result<Self> {
        let started = Instant::now();
        let parent =
            shared_parent(paths).ok_or_else(|| Error::from(HRESULT(0x80004005u32 as i32)))?;
        let parent_pidl = pidl_from_path(&parent)?;

        let mut full_pidls = Vec::with_capacity(paths.len());
        for path in paths {
            match pidl_from_path(path) {
                Ok(pidl) => full_pidls.push(pidl),
                Err(err) => {
                    free_pidls(&mut full_pidls, parent_pidl);
                    return Err(err);
                }
            }
        }

        // From here on `Self` owns the PIDLs and the HMENU, so every early
        // return releases them through `Drop`.
        let context_menu = match create_default_context_menu(&full_pidls, parent_pidl, hwnd) {
            Ok(menu) => menu,
            Err(err) => {
                free_pidls(&mut full_pidls, parent_pidl);
                return Err(err);
            }
        };
        let handler = MenuMsgHandler::new(&context_menu);
        let mut this = Self {
            context_menu,
            handler,
            menu: HMENU::default(),
            nodes: Vec::new(),
            id_base: CONTEXT_MENU_ID_BASE,
            full_pidls,
            parent_pidl,
        };

        this.menu = unsafe { CreatePopupMenu()? };
        let created = started.elapsed();

        unsafe {
            let _ = this.context_menu.QueryContextMenu(
                this.menu,
                0,
                CONTEXT_MENU_ID_BASE,
                CONTEXT_MENU_ID_MAX,
                CMF_NORMAL,
            );
        }
        let queried = started.elapsed();

        let mut stats = CollectStats::default();
        this.nodes = collect_level(this.menu, &mut stats);
        debug_log(format_args!(
            "root: {} nodes in {:.2} ms (create {:.2} ms, QueryContextMenu {:.2} ms, collect {:.2} ms){}",
            this.nodes.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            created.as_secs_f64() * 1000.0,
            (queried - created).as_secs_f64() * 1000.0,
            (started.elapsed() - queried).as_secs_f64() * 1000.0,
            stats,
        ));

        Ok(this)
    }

    /// Top-level entries of the shell menu.
    pub fn nodes(&self) -> &[ShellMenuNode] {
        &self.nodes
    }

    /// Children of `submenu`, populated on first use: sends
    /// `WM_INITMENUPOPUP` to the shell menu once (as a native menu host
    /// would) and reads the HMENU. Later calls return the cached children and
    /// only re-read the HMENU when its item count changed (asynchronously
    /// filled submenus), so calling this every frame is cheap.
    pub fn submenu_children(&self, submenu: &ShellSubmenu) -> Rc<Vec<ShellMenuNode>> {
        let started = Instant::now();
        let mut init_result = None;
        if submenu.initialized_at.get().is_none() {
            // MAKELPARAM(index, FALSE): not the window menu.
            let lparam = LPARAM((submenu.index_in_parent & 0xFFFF) as isize);
            let handled = self
                .handler
                .handle(WM_INITMENUPOPUP, WPARAM(submenu.hmenu.0 as usize), lparam)
                .is_some();
            let _ = submenu.initialized_at.set(Instant::now());
            init_result = Some((handled, started.elapsed()));
        }

        let count = submenu.item_count();
        if let Some((cached_count, children)) = &*submenu.children.borrow()
            && *cached_count == count
        {
            return Rc::clone(children);
        }

        let mut stats = CollectStats::default();
        let children = Rc::new(collect_level(submenu.hmenu, &mut stats));
        *submenu.children.borrow_mut() = Some((count, Rc::clone(&children)));

        let init = match init_result {
            Some((handled, elapsed)) => format!(
                "WM_INITMENUPOPUP {} {:.2} ms",
                if handled { "handled" } else { "unhandled" },
                elapsed.as_secs_f64() * 1000.0
            ),
            None => format!(
                "re-read after async change, {:.0} ms after init",
                submenu
                    .initialized_at
                    .get()
                    .map_or(0.0, |at| at.elapsed().as_secs_f64() * 1000.0)
            ),
        };
        debug_log(format_args!(
            "submenu '{}': {} nodes in {:.2} ms ({init}){}",
            submenu.label,
            children.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            stats,
        ));
        children
    }

    pub fn invoke(&self, hwnd: HWND, id: u32) -> windows::core::Result<()> {
        let verb_offset = id.saturating_sub(self.id_base) as usize;
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: 0,
            hwnd,
            lpVerb: PCSTR(verb_offset as *const u8),
            lpVerbW: PCWSTR::null(),
            nShow: 1,
            ..Default::default()
        };

        unsafe {
            self.context_menu
                .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
        }
    }

    /// Displays the complete shell-provided menu at the current pointer
    /// position and invokes the command selected by the user.
    ///
    /// While the menu is tracked, the owner window's procedure forwards menu
    /// messages to this menu (see [`forward_native_menu_message`]), so lazily
    /// populated and owner-drawn submenus work as in Explorer.
    pub fn show_native(&self, hwnd: HWND) -> windows::core::Result<()> {
        // A modal native menu at the real cursor would hijack the user's session.
        if crate::core::probe_active() {
            return Ok(());
        }
        let mut point = windows::Win32::Foundation::POINT::default();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point)?;
        }
        let command = {
            let _active = ActiveNativeMenuGuard::install(self.handler.clone());
            unsafe {
                TrackPopupMenuEx(
                    self.menu,
                    (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                    point.x,
                    point.y,
                    hwnd,
                    None,
                )
            }
        };
        let command = command.0 as u32;
        if command != 0 {
            self.invoke(hwnd, command)?;
        }
        Ok(())
    }
}

impl Drop for ShellContextMenu {
    fn drop(&mut self) {
        unsafe {
            if !self.menu.is_invalid() {
                // Destroys every submenu HMENU referenced by the node tree.
                let _ = DestroyMenu(self.menu);
            }
        }
        free_pidls(&mut self.full_pidls, self.parent_pidl);
        self.parent_pidl = std::ptr::null_mut();
    }
}

fn create_default_context_menu(
    full_pidls: &[*mut ITEMIDLIST],
    parent_pidl: *mut ITEMIDLIST,
    hwnd: HWND,
) -> windows::core::Result<IContextMenu> {
    let mut child_pidls: Vec<*mut ITEMIDLIST> = full_pidls
        .iter()
        .map(|pidl| unsafe { ILFindLastID(*pidl) })
        .collect();

    let parent_folder: IShellFolder =
        unsafe { SHBindToParent::<IShellFolder>(full_pidls[0], None)? };

    let mut dcm = DEFCONTEXTMENU::default();
    dcm.hwnd = hwnd;
    dcm.pcmcb = ManuallyDrop::new(None);
    dcm.pidlFolder = parent_pidl;
    dcm.psf = ManuallyDrop::new(Some(parent_folder));
    dcm.cidl = child_pidls.len() as u32;
    dcm.apidl = child_pidls.as_mut_ptr();
    dcm.punkAssociationInfo = ManuallyDrop::new(None);
    dcm.cKeys = 0;
    dcm.aKeys = std::ptr::null();

    let result = unsafe { SHCreateDefaultContextMenu(&dcm) };
    // DEFCONTEXTMENU does not release its interface fields on drop.
    unsafe { ManuallyDrop::drop(&mut dcm.psf) };
    result
}

fn free_pidls(full_pidls: &mut Vec<*mut ITEMIDLIST>, parent_pidl: *mut ITEMIDLIST) {
    unsafe {
        for pidl in full_pidls.drain(..) {
            CoTaskMemFree(Some(pidl as _));
        }
        if !parent_pidl.is_null() {
            CoTaskMemFree(Some(parent_pidl as _));
        }
    }
}

fn shared_parent(paths: &[PathBuf]) -> Option<PathBuf> {
    let parent = paths.first()?.parent()?.to_path_buf();
    if paths.iter().all(|p| p.parent() == Some(parent.as_path())) {
        Some(parent)
    } else {
        None
    }
}

fn pidl_from_path(path: &Path) -> windows::core::Result<*mut ITEMIDLIST> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let pidl = unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) };
    if pidl.is_null() {
        Err(Error::from(HRESULT(0x80004005u32 as i32)))
    } else {
        Ok(pidl)
    }
}

/// Items that could not be represented as text (diagnostics only).
#[derive(Default)]
struct CollectStats {
    /// Owner-drawn items without a text label.
    owner_draw_skipped: u32,
    /// Bitmap-only items (`hbmpItem` set, no text).
    bitmap_skipped: u32,
    /// Other items without a text label (or without a command id).
    unlabeled_skipped: u32,
}

impl std::fmt::Display for CollectStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.owner_draw_skipped + self.bitmap_skipped + self.unlabeled_skipped == 0 {
            return Ok(());
        }
        write!(
            f,
            "; skipped owner-draw {}, bitmap-only {}, unlabeled {}",
            self.owner_draw_skipped, self.bitmap_skipped, self.unlabeled_skipped
        )
    }
}

/// Reads one level of `menu`. Submenus are recorded with their HMENU but
/// not descended into (see [`ShellContextMenu::submenu_children`]).
fn collect_level(menu: HMENU, stats: &mut CollectStats) -> Vec<ShellMenuNode> {
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    if count <= 0 {
        return Vec::new();
    }

    let mut nodes = Vec::with_capacity(count as usize);
    let mut buffer: Vec<u16> = Vec::new();
    for index in 0..count as u32 {
        let mask = MIIM_FTYPE
            | MIIM_ID
            | MIIM_STATE
            | MIIM_STRING
            | MIIM_SUBMENU
            | MIIM_BITMAP
            | MIIM_DATA;

        // First query the text length, then read the text into a buffer of
        // exactly that size (labels are not limited to a fixed length).
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: mask,
            ..Default::default()
        };
        if unsafe { GetMenuItemInfoW(menu, index, true, &mut info) }.is_err() {
            continue;
        }

        if info.fType.contains(MFT_SEPARATOR) {
            nodes.push(ShellMenuNode::Separator);
            continue;
        }

        let raw_label = if info.cch == 0 {
            String::new()
        } else {
            buffer.clear();
            buffer.resize(info.cch as usize + 1, 0);
            // Separate struct: the call does not preserve unrequested fields.
            let mut text_info = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING,
                dwTypeData: PWSTR(buffer.as_mut_ptr()),
                cch: buffer.len() as u32,
                ..Default::default()
            };
            if unsafe { GetMenuItemInfoW(menu, index, true, &mut text_info) }.is_err() {
                String::new()
            } else {
                let len = (text_info.cch as usize).min(buffer.len());
                String::from_utf16_lossy(&buffer[..len])
            }
        };
        let label = sanitize_label(&raw_label);

        // MFS_DISABLED == MFS_GRAYED == MF_GRAYED | MF_DISABLED; either bit
        // disables the item.
        let disabled = info.fState.0 & MFS_DISABLED.0 != 0;

        if label.is_empty() {
            if info.fType.contains(MFT_OWNERDRAW) {
                stats.owner_draw_skipped += 1;
            } else if !info.hbmpItem.is_invalid() {
                stats.bitmap_skipped += 1;
            } else {
                stats.unlabeled_skipped += 1;
            }
            continue;
        }

        if !info.hSubMenu.is_invalid() {
            nodes.push(ShellMenuNode::Submenu(ShellSubmenu::new(
                label,
                disabled,
                info.hSubMenu,
                index,
            )));
            continue;
        }

        if info.wID == 0 {
            stats.unlabeled_skipped += 1;
            continue;
        }

        nodes.push(ShellMenuNode::Command {
            id: info.wID,
            label,
            disabled,
            is_default: info.fState.contains(MFS_DEFAULT),
        });
    }

    normalize_separators(&mut nodes);
    nodes
}

/// Drops leading/trailing separators and collapses runs of separators
/// (skipped items can leave adjacent separators behind).
fn normalize_separators(nodes: &mut Vec<ShellMenuNode>) {
    let mut previous_was_separator = true; // drops leading separators
    nodes.retain(|node| {
        let is_separator = matches!(node, ShellMenuNode::Separator);
        let keep = !(is_separator && previous_was_separator);
        if keep {
            previous_was_separator = is_separator;
        }
        keep
    });
    if matches!(nodes.last(), Some(ShellMenuNode::Separator)) {
        nodes.pop();
    }
}

/// Turns a Win32 menu label into display text: removes mnemonic markers
/// (`&x` → `x`, `&&` → `&`, and the CJK-style `(&X)` suffix entirely) and
/// cuts the accelerator text after a tab.
fn sanitize_label(raw: &str) -> String {
    let text = raw.split('\t').next().unwrap_or_default();

    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.next() {
                Some('&') => out.push('&'),
                Some(next) => out.push(next),
                None => {}
            }
            continue;
        }
        if c == '(' {
            // "(&N)" style mnemonic suffix used by Japanese/Chinese/Korean UIs.
            let mut lookahead = chars.clone();
            if lookahead.next() == Some('&')
                && lookahead.next().is_some_and(|m| m != '&')
                && lookahead.next() == Some(')')
            {
                chars = lookahead;
                continue;
            }
        }
        out.push(c);
    }

    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_label_strips_mnemonics() {
        assert_eq!(sanitize_label("&Open"), "Open");
        assert_eq!(sanitize_label("Save && Close"), "Save & Close");
        assert_eq!(sanitize_label("送る(&N)"), "送る");
        assert_eq!(
            sanitize_label("プログラムから開く(&H)..."),
            "プログラムから開く..."
        );
        assert_eq!(sanitize_label("Copy\tCtrl+C"), "Copy");
        assert_eq!(sanitize_label("A (&&B)"), "A (&B)");
        assert_eq!(sanitize_label("Trailing&"), "Trailing");
        assert_eq!(sanitize_label("Properties..."), "Properties...");
        assert_eq!(sanitize_label(""), "");
        assert_eq!(sanitize_label("  &Pad  "), "Pad");
    }

    #[test]
    fn separators_are_normalized() {
        let cmd = |id| ShellMenuNode::Command {
            id,
            label: String::new(),
            disabled: false,
            is_default: false,
        };
        let mut nodes = vec![
            ShellMenuNode::Separator,
            cmd(1),
            ShellMenuNode::Separator,
            ShellMenuNode::Separator,
            cmd(2),
            ShellMenuNode::Separator,
        ];
        normalize_separators(&mut nodes);
        let shape: Vec<Option<u32>> = nodes
            .iter()
            .map(|n| match n {
                ShellMenuNode::Command { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(shape, vec![Some(1), None, Some(2)]);

        let mut only_separators = vec![ShellMenuNode::Separator, ShellMenuNode::Separator];
        normalize_separators(&mut only_separators);
        assert!(only_separators.is_empty());
    }
}
