use crate::core::indexer::{load_context_menu_settings, save_context_menu_settings};
use eframe::egui;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::mem::{ManuallyDrop, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODA_DRAWENTIRE, ODS_FLAGS, ODT_MENU,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_EXTENDEDVERBS, CMF_NORMAL, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DEFCONTEXTMENU,
    GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, ILCreateFromPathW, ILFindLastID,
    IShellFolder, SHBindToParent, SHCreateDefaultContextMenu,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DeleteMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU,
    MENUITEMINFOW, MF_BYPOSITION, MFS_DEFAULT, MFS_DISABLED, MFT_OWNERDRAW, MFT_SEPARATOR,
    MIIM_BITMAP, MIIM_DATA, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, RemoveMenu,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_DRAWITEM, WM_INITMENUPOPUP,
    WM_MEASUREITEM, WM_MENUCHAR,
};
use windows::core::{Error, HRESULT, Interface, PCSTR, PCWSTR, PSTR, PWSTR};

const CONTEXT_MENU_ID_BASE: u32 = 1;
const CONTEXT_MENU_ID_MAX: u32 = 0x7FFF;

/// Logical size (points) of the icons shown next to shell menu entries.
pub const MENU_ICON_SIZE: f32 = 16.0;

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

// ---------------------------------------------------------------------------
// Hidden entries (user preference, persisted in `context_menu.json`)
// ---------------------------------------------------------------------------

/// An entry of the most recently built shell menu (root and every submenu
/// populated since), listed on the settings page so it can be hidden.
#[derive(Clone, Debug)]
pub struct SeenMenuEntry {
    /// Matching key, see [`entry_key`].
    pub key: String,
    /// The entry's own label.
    pub label: String,
    /// Label including the parent submenus, e.g. `送る > ドキュメント`.
    pub path_label: String,
    /// Nesting level (0 = top level of the shell menu).
    pub depth: usize,
    pub is_submenu: bool,
    /// Key of the parent submenu (empty for top-level entries).
    parent: String,
}

struct MenuPrefs {
    hidden: HashSet<String>,
    /// Display labels of hidden keys (persisted with them).
    labels: std::collections::BTreeMap<String, String>,
    /// Bumped on every change of `hidden`, so cached shell menus know they
    /// are outdated.
    version: u64,
    seen: Vec<SeenMenuEntry>,
    /// Hidden entries of Eden's own item menu (`eden:` keys, see
    /// [`crate::gui::windows::eden_menu_entries`]). Kept apart from `hidden`
    /// so they never show up among the shell entries in the settings.
    eden_hidden: HashSet<String>,
    /// Bumped on every change of `eden_hidden`. Separate from `version`:
    /// toggling Eden's entries must not rebuild the cached shell menu.
    eden_version: u64,
}

impl MenuPrefs {
    fn load() -> Self {
        let snapshot = load_context_menu_settings();
        Self {
            hidden: snapshot.hidden.into_iter().collect(),
            labels: snapshot.labels,
            version: 0,
            seen: Vec::new(),
            eden_hidden: snapshot.eden_hidden.into_iter().collect(),
            eden_version: 0,
        }
    }

    /// Saves after a change of the shell entries.
    fn save_shell(&mut self) {
        self.version += 1;
        self.save();
    }

    /// Saves after a change of Eden's entries.
    fn save_eden(&mut self) {
        self.eden_version += 1;
        self.save();
    }

    fn save(&mut self) {
        let mut hidden: Vec<String> = self.hidden.iter().cloned().collect();
        hidden.sort();
        let hidden_keys = &self.hidden;
        self.labels.retain(|key, _| hidden_keys.contains(key));
        let mut eden_hidden: Vec<String> = self.eden_hidden.iter().cloned().collect();
        eden_hidden.sort();
        save_context_menu_settings(&crate::core::indexer::ContextMenuSettingsSnapshot {
            hidden,
            labels: self.labels.clone(),
            eden_hidden,
        });
    }
}

thread_local! {
    /// Loaded on first use. The menu, the native menu loop and the settings
    /// page all run on the UI thread.
    static MENU_PREFS: RefCell<Option<MenuPrefs>> = const { RefCell::new(None) };
}

fn with_prefs<R>(f: impl FnOnce(&mut MenuPrefs) -> R) -> R {
    MENU_PREFS.with(|slot| {
        let mut slot = slot.borrow_mut();
        f(slot.get_or_insert_with(MenuPrefs::load))
    })
}

/// Changes whenever the set of hidden entries changes.
pub fn hidden_entries_version() -> u64 {
    with_prefs(|prefs| prefs.version)
}

/// Hides or shows a shell menu entry and saves the preference immediately.
pub fn set_entry_hidden(key: &str, path_label: &str, hidden: bool) {
    with_prefs(|prefs| {
        let changed = if hidden {
            prefs.labels.insert(key.to_string(), path_label.to_string());
            prefs.hidden.insert(key.to_string())
        } else {
            prefs.hidden.remove(key)
        };
        if changed {
            prefs.save_shell();
        }
    });
}

/// Shows every hidden shell entry again.
pub fn reset_hidden_entries() {
    with_prefs(|prefs| {
        if !prefs.hidden.is_empty() {
            prefs.hidden.clear();
            prefs.save_shell();
        }
    });
}

/// Changes whenever the set of hidden Eden entries changes (independent of
/// [`hidden_entries_version`]).
pub fn eden_entries_version() -> u64 {
    with_prefs(|prefs| prefs.eden_version)
}

/// Keys of the hidden Eden item-menu entries.
pub fn hidden_eden_entries() -> HashSet<String> {
    with_prefs(|prefs| prefs.eden_hidden.clone())
}

/// Hides or shows one of Eden's own item-menu entries and saves the
/// preference immediately. Does not invalidate cached shell menus.
pub fn set_eden_entry_hidden(key: &str, hidden: bool) {
    with_prefs(|prefs| {
        let changed = if hidden {
            prefs.eden_hidden.insert(key.to_string())
        } else {
            prefs.eden_hidden.remove(key)
        };
        if changed {
            prefs.save_eden();
        }
    });
}

/// Shows every hidden Eden entry again.
pub fn reset_hidden_eden_entries() {
    with_prefs(|prefs| {
        if !prefs.eden_hidden.is_empty() {
            prefs.eden_hidden.clear();
            prefs.save_eden();
        }
    });
}

/// Hidden keys with their display labels, sorted by label.
pub fn hidden_entries() -> Vec<(String, String)> {
    with_prefs(|prefs| {
        let mut entries: Vec<(String, String)> = prefs
            .hidden
            .iter()
            .map(|key| {
                let label = prefs
                    .labels
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| key_display_fallback(key));
                (key.clone(), label)
            })
            .collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        entries
    })
}

/// Entries of the most recently built shell menu, in menu order (submenu
/// children follow their parent).
pub fn seen_entries() -> Vec<SeenMenuEntry> {
    with_prefs(|prefs| prefs.seen.clone())
}

fn key_display_fallback(key: &str) -> String {
    key.strip_prefix("label:").unwrap_or(key).to_string()
}

fn hidden_set() -> HashSet<String> {
    with_prefs(|prefs| prefs.hidden.clone())
}

fn record_seen_root(entries: Vec<SeenMenuEntry>) {
    with_prefs(|prefs| prefs.seen = entries);
}

/// Records the children of a submenu right after the submenu's own entry
/// (replacing the children recorded by an earlier read).
fn record_seen_children(parent_key: &str, mut entries: Vec<SeenMenuEntry>) {
    for entry in &mut entries {
        entry.parent = parent_key.to_string();
    }
    with_prefs(|prefs| {
        prefs.seen.retain(|entry| entry.parent != parent_key);
        let insert_at = prefs
            .seen
            .iter()
            .position(|entry| entry.key == parent_key)
            .map_or(prefs.seen.len(), |pos| pos + 1);
        prefs.seen.splice(insert_at..insert_at, entries);
    });
}

/// Matching key of a shell menu entry: the canonical verb when the command
/// has one (stable across languages), otherwise the label path, e.g.
/// `label:送る` or `label:送る > ドキュメント`.
fn entry_key(verb: Option<&str>, parents: &[String], label: &str) -> String {
    match verb {
        Some(verb) => format!("verb:{verb}"),
        None => format!("label:{}", path_label(parents, label)),
    }
}

fn path_label(parents: &[String], label: &str) -> String {
    let mut text = String::new();
    for parent in parents {
        text.push_str(parent);
        text.push_str(" > ");
    }
    text.push_str(label);
    text
}

/// Canonical verb of a command (`GCS_VERBW`), if the handler provides one.
fn command_verb(context_menu: &IContextMenu, offset: u32) -> Option<String> {
    let mut buffer = [0u16; 260];
    unsafe {
        context_menu.GetCommandString(
            offset as usize,
            GCS_VERBW,
            None,
            PSTR(buffer.as_mut_ptr().cast()),
            (buffer.len() - 1) as u32,
        )
    }
    .ok()?;
    parse_verb(&buffer)
}

/// Accepts printable ASCII verbs only: some handlers write ANSI text into the
/// wide buffer, which decodes to unrelated (mostly CJK) characters.
fn parse_verb(buffer: &[u16]) -> Option<String> {
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let verb = String::from_utf16(&buffer[..len]).ok()?;
    let verb = verb.trim();
    (!verb.is_empty() && verb.chars().all(|c| c.is_ascii_graphic() || c == ' '))
        .then(|| verb.to_string())
}

/// What is needed to compute keys for the items of one menu level.
struct LevelContext<'a> {
    /// Verb source; `None` keys every entry by label (tests).
    context_menu: Option<&'a IContextMenu>,
    id_base: u32,
    /// Labels of the enclosing submenus.
    parents: &'a [String],
}

/// Computes the keys of one level. `items` are `(command id, label)` with
/// `None` for submenus. A verb shared by several items of the level is not
/// distinctive, so those items fall back to label keys.
fn level_keys(
    level: &LevelContext<'_>,
    items: &[(Option<u32>, &str)],
    stats: &mut CollectStats,
) -> Vec<String> {
    let started = Instant::now();
    let verbs: Vec<Option<String>> = items
        .iter()
        .map(|(id, _)| {
            let id = (*id)?;
            if !(level.id_base..=CONTEXT_MENU_ID_MAX).contains(&id) {
                return None;
            }
            command_verb(level.context_menu?, id - level.id_base)
        })
        .collect();
    let mut verb_counts: HashMap<&str, u32> = HashMap::new();
    for verb in verbs.iter().flatten() {
        *verb_counts.entry(verb.as_str()).or_default() += 1;
    }
    let keys = items
        .iter()
        .zip(&verbs)
        .map(|((_, label), verb)| {
            let verb = verb
                .as_deref()
                .filter(|verb| verb_counts.get(verb).copied() == Some(1));
            if verb.is_some() {
                stats.verbs += 1;
            }
            entry_key(verb, level.parents, label)
        })
        .collect();
    stats.verb_time += started.elapsed();
    keys
}

// ---------------------------------------------------------------------------
// Menu tree
// ---------------------------------------------------------------------------

/// One entry of the shell context menu, mirroring the native HMENU tree.
pub enum ShellMenuNode {
    Command {
        /// Global command id (unique across all submenus of the menu).
        id: u32,
        label: String,
        disabled: bool,
        /// The default verb (rendered bold by Explorer).
        is_default: bool,
        icon: MenuIcon,
    },
    Submenu(ShellSubmenu),
    Separator,
}

/// Icon of a shell menu entry, converted to an egui texture on first use and
/// then cached for the lifetime of the menu.
pub struct MenuIcon {
    source: IconSource,
    texture: OnceCell<Option<egui::TextureHandle>>,
}

impl MenuIcon {
    fn new(source: IconSource) -> Self {
        Self {
            source,
            texture: OnceCell::new(),
        }
    }

    /// An entry without icon.
    #[cfg(test)]
    pub fn none() -> Self {
        Self::new(IconSource::None)
    }
}

#[derive(Clone, Copy)]
enum IconSource {
    None,
    /// A real bitmap (`MENUITEMINFOW::hbmpItem`), owned by the menu item.
    Bitmap(HBITMAP),
    /// `HBMMENU_CALLBACK`: the owner measures and draws just the bitmap on
    /// `WM_MEASUREITEM` / `WM_DRAWITEM`.
    Callback {
        hmenu: HMENU,
        id: u32,
        item_data: usize,
    },
    /// `MFT_OWNERDRAW` item with a text label: the owner draws the complete
    /// item; the icon is cropped from its left end.
    OwnerDraw {
        hmenu: HMENU,
        id: u32,
        item_data: usize,
    },
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
    pub icon: MenuIcon,
    /// Hide-matching key of the submenu entry itself.
    key: String,
    /// Labels of the enclosing submenus plus this one (parents of children).
    path: Vec<String>,
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

// ---------------------------------------------------------------------------
// Native (TrackPopupMenuEx) mode
// ---------------------------------------------------------------------------

/// Removes hidden entries from the native HMENU tree: the root before the
/// menu is shown and each submenu after the shell populated it
/// (`WM_INITMENUPOPUP`). Items a shell extension adds later asynchronously
/// are not seen (best effort).
struct NativePruner {
    context_menu: Option<IContextMenu>,
    id_base: u32,
    hidden: HashSet<String>,
    /// Menus that may be pruned: HMENU -> (entry key, label path). The root
    /// has an empty key and path.
    menus: RefCell<HashMap<usize, (String, Vec<String>)>>,
    /// Removed (not destroyed) submenus; the shell may still reference them
    /// while the menu is alive, so they are destroyed with the menu.
    detached: RefCell<Vec<HMENU>>,
}

impl NativePruner {
    fn new(context_menu: IContextMenu, id_base: u32, root: HMENU) -> Self {
        Self::with_hidden(Some(context_menu), id_base, root, hidden_set())
    }

    fn with_hidden(
        context_menu: Option<IContextMenu>,
        id_base: u32,
        root: HMENU,
        hidden: HashSet<String>,
    ) -> Self {
        let mut menus = HashMap::new();
        menus.insert(root.0 as usize, (String::new(), Vec::new()));
        Self {
            context_menu,
            id_base,
            hidden,
            menus: RefCell::new(menus),
            detached: RefCell::new(Vec::new()),
        }
    }

    fn prune(&self, menu: HMENU) {
        let Some((menu_key, parents)) = self.menus.borrow().get(&(menu.0 as usize)).cloned() else {
            return;
        };

        let count = unsafe { GetMenuItemCount(Some(menu)) };
        let mut buffer = Vec::new();
        // (position, label, command id, submenu)
        let mut items: Vec<(u32, String, Option<u32>, Option<HMENU>)> = Vec::new();
        for index in 0..count.max(0) as u32 {
            let Some(item) = read_menu_item(menu, index, MIIM_FTYPE | MIIM_ID, &mut buffer) else {
                continue;
            };
            if item.info.fType.contains(MFT_SEPARATOR) || item.label.is_empty() {
                continue;
            }
            let submenu = (!item.info.hSubMenu.is_invalid()).then_some(item.info.hSubMenu);
            let id = (submenu.is_none() && item.info.wID != 0).then_some(item.info.wID);
            items.push((index, item.label, id, submenu));
        }

        let level = LevelContext {
            context_menu: self.context_menu.as_ref(),
            id_base: self.id_base,
            parents: &parents,
        };
        let mut stats = CollectStats::default();
        let key_input: Vec<(Option<u32>, &str)> = items
            .iter()
            .map(|(_, label, id, _)| (*id, label.as_str()))
            .collect();
        let keys = level_keys(&level, &key_input, &mut stats);

        if !menu_key.is_empty() {
            record_seen_children(
                &menu_key,
                items
                    .iter()
                    .zip(&keys)
                    .map(|((_, label, _, submenu), key)| SeenMenuEntry {
                        key: key.clone(),
                        label: label.clone(),
                        path_label: path_label(&parents, label),
                        depth: parents.len(),
                        is_submenu: submenu.is_some(),
                        parent: menu_key.clone(),
                    })
                    .collect(),
            );
        }

        // Back to front, so the positions of the remaining items stay valid.
        let mut removed = 0;
        for ((index, label, _, submenu), key) in items.iter().zip(&keys).rev() {
            if self.hidden.contains(key) {
                let result = unsafe {
                    match submenu {
                        Some(submenu) => {
                            self.detached.borrow_mut().push(*submenu);
                            RemoveMenu(menu, *index, MF_BYPOSITION)
                        }
                        None => DeleteMenu(menu, *index, MF_BYPOSITION),
                    }
                };
                if result.is_ok() {
                    removed += 1;
                }
            } else if let Some(submenu) = submenu {
                let mut path = parents.clone();
                path.push(label.clone());
                self.menus
                    .borrow_mut()
                    .insert(submenu.0 as usize, (key.clone(), path));
            }
        }
        if removed > 0 {
            normalize_native_separators(menu);
            debug_log(format_args!(
                "native: removed {removed} hidden entries from '{}'",
                parents.join(" > ")
            ));
        }
    }
}

/// Removes leading, trailing and repeated separators from a native menu.
fn normalize_native_separators(menu: HMENU) {
    let is_separator = |index: u32| {
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE,
            ..Default::default()
        };
        unsafe { GetMenuItemInfoW(menu, index, true, &mut info) }.is_ok()
            && info.fType.contains(MFT_SEPARATOR)
    };
    let mut index = 0u32;
    let mut previous_was_separator = true;
    while (index as i32) < unsafe { GetMenuItemCount(Some(menu)) } {
        let separator = is_separator(index);
        if separator && previous_was_separator {
            if unsafe { DeleteMenu(menu, index, MF_BYPOSITION) }.is_err() {
                index += 1;
            }
            continue;
        }
        previous_was_separator = separator;
        index += 1;
    }
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    if count > 0 && is_separator(count as u32 - 1) {
        let _ = unsafe { DeleteMenu(menu, count as u32 - 1, MF_BYPOSITION) };
    }
}

#[derive(Clone)]
struct ActiveNativeMenu {
    handler: MenuMsgHandler,
    /// `None` for the unfiltered standard menu.
    pruner: Option<Rc<NativePruner>>,
}

thread_local! {
    /// The shell menu currently tracked by a native `TrackPopupMenuEx` loop.
    static ACTIVE_NATIVE_MENU: RefCell<Option<ActiveNativeMenu>> = const { RefCell::new(None) };
}

/// Makes `active` the target of [`forward_native_menu_message`] until dropped.
struct ActiveNativeMenuGuard {
    previous: Option<ActiveNativeMenu>,
}

impl ActiveNativeMenuGuard {
    fn install(active: ActiveNativeMenu) -> Self {
        let previous = ACTIVE_NATIVE_MENU.with(|slot| slot.replace(Some(active)));
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
    let active =
        ACTIVE_NATIVE_MENU.with(|slot| slot.try_borrow().ok().and_then(|active| active.clone()))?;
    let result = active.handler.handle(msg, wparam, lparam);
    if msg == WM_INITMENUPOPUP
        && let Some(pruner) = &active.pruner
    {
        // The shell has populated the submenu now; drop hidden entries.
        pruner.prune(HMENU(wparam.0 as *mut _));
    }
    let result = result?;

    Some(match msg {
        WM_MENUCHAR => result,
        WM_INITMENUPOPUP => LRESULT(0),
        // WM_DRAWITEM / WM_MEASUREITEM: TRUE when processed.
        _ => LRESULT(1),
    })
}

// ---------------------------------------------------------------------------
// Shell context menu
// ---------------------------------------------------------------------------

/// Accumulated icon conversion work (diagnostics only).
#[derive(Clone, Copy, Default)]
struct IconLoadStats {
    converted: u32,
    failed: u32,
    elapsed: Duration,
}

pub struct ShellContextMenu {
    context_menu: IContextMenu,
    handler: MenuMsgHandler,
    menu: HMENU,
    nodes: Vec<ShellMenuNode>,
    id_base: u32,
    full_pidls: Vec<*mut ITEMIDLIST>,
    parent_pidl: *mut ITEMIDLIST,
    /// [`hidden_entries_version`] the node tree was filtered with.
    hidden_version: u64,
    kind: ShellMenuKind,
    /// Submenus removed by the native mode's pruning (destroyed on drop).
    detached_submenus: RefCell<Vec<HMENU>>,
    icon_stats: Cell<IconLoadStats>,
}

/// How a [`ShellContextMenu`] is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellMenuKind {
    /// `CMF_NORMAL`, hidden entries removed, node tree read for the egui
    /// menu and the entries recorded for the settings page.
    Filtered,
    /// `CMF_NORMAL | CMF_EXTENDEDVERBS` (Explorer's Shift+right-click menu),
    /// unfiltered and only shown natively: no node tree, nothing recorded
    /// (extended-only verbs would otherwise flood the settings list).
    Standard,
}

impl ShellMenuKind {
    fn query_flags(self) -> u32 {
        match self {
            Self::Filtered => CMF_NORMAL,
            Self::Standard => CMF_NORMAL | CMF_EXTENDEDVERBS,
        }
    }
}

impl ShellContextMenu {
    /// The shell menu for Eden's egui context menu: hidden entries removed,
    /// its entries listed on the settings page.
    pub fn for_paths(paths: &[PathBuf], hwnd: HWND) -> windows::core::Result<Self> {
        Self::build(paths, hwnd, ShellMenuKind::Filtered)
    }

    /// The complete Windows menu including the extended verbs, as Explorer
    /// shows it on Shift+right-click. Nothing is hidden. Only for
    /// [`Self::show_native`]: the egui node tree is not read ([`Self::nodes`]
    /// is empty). Build a new one for every showing; shell extensions may
    /// append their items again to a reused HMENU.
    pub fn standard_for_paths(paths: &[PathBuf], hwnd: HWND) -> windows::core::Result<Self> {
        Self::build(paths, hwnd, ShellMenuKind::Standard)
    }

    fn build(paths: &[PathBuf], hwnd: HWND, kind: ShellMenuKind) -> windows::core::Result<Self> {
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
            hidden_version: hidden_entries_version(),
            kind,
            detached_submenus: RefCell::new(Vec::new()),
            icon_stats: Cell::new(IconLoadStats::default()),
        };

        this.menu = unsafe { CreatePopupMenu()? };
        let created = started.elapsed();

        unsafe {
            let _ = this.context_menu.QueryContextMenu(
                this.menu,
                0,
                CONTEXT_MENU_ID_BASE,
                CONTEXT_MENU_ID_MAX,
                kind.query_flags(),
            );
        }
        let queried = started.elapsed();

        if kind == ShellMenuKind::Standard {
            debug_log(format_args!(
                "standard (extended verbs): {} items in {:.2} ms (create {:.2} ms, QueryContextMenu {:.2} ms)",
                unsafe { GetMenuItemCount(Some(this.menu)) },
                started.elapsed().as_secs_f64() * 1000.0,
                created.as_secs_f64() * 1000.0,
                (queried - created).as_secs_f64() * 1000.0,
            ));
            return Ok(this);
        }

        let mut stats = CollectStats::default();
        let level = LevelContext {
            context_menu: Some(&this.context_menu),
            id_base: this.id_base,
            parents: &[],
        };
        let (nodes, seen) = collect_level(this.menu, &level, &mut stats);
        this.nodes = nodes;
        record_seen_root(seen);
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

    /// The [`hidden_entries_version`] this menu was built with; a different
    /// current version means the menu must be rebuilt.
    pub fn hidden_version(&self) -> u64 {
        self.hidden_version
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
        let level = LevelContext {
            context_menu: Some(&self.context_menu),
            id_base: self.id_base,
            parents: &submenu.path,
        };
        let (children, seen) = collect_level(submenu.hmenu, &level, &mut stats);
        let children = Rc::new(children);
        record_seen_children(&submenu.key, seen);
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

    /// The icon of an entry as a texture, converted from the shell's bitmap
    /// (or rendered offscreen for owner-drawn icons) on first use and cached
    /// with the menu.
    pub fn icon_texture(
        &self,
        ctx: &egui::Context,
        icon: &MenuIcon,
    ) -> Option<egui::TextureHandle> {
        icon.texture
            .get_or_init(|| {
                if matches!(icon.source, IconSource::None) {
                    return None;
                }
                let started = Instant::now();
                let image = match icon.source {
                    IconSource::None => None,
                    IconSource::Bitmap(bitmap) => bitmap_to_image(bitmap),
                    IconSource::Callback {
                        hmenu,
                        id,
                        item_data,
                    } => self.render_owner_drawn(hmenu, id, item_data, false, ctx),
                    IconSource::OwnerDraw {
                        hmenu,
                        id,
                        item_data,
                    } => self.render_owner_drawn(hmenu, id, item_data, true, ctx),
                };
                let texture = image.map(|image| {
                    ctx.load_texture("shell_menu_icon", image, egui::TextureOptions::LINEAR)
                });
                let mut stats = self.icon_stats.get();
                if texture.is_some() {
                    stats.converted += 1;
                } else {
                    stats.failed += 1;
                }
                stats.elapsed += started.elapsed();
                self.icon_stats.set(stats);
                texture
            })
            .clone()
    }

    /// Logs (debug/probe builds) and resets the icon conversion work done
    /// since the last call.
    pub fn log_icon_stats(&self, level: &str) {
        let stats = self.icon_stats.take();
        if stats.converted + stats.failed > 0 {
            debug_log(format_args!(
                "icons '{level}': {} converted, {} failed in {:.2} ms",
                stats.converted,
                stats.failed,
                stats.elapsed.as_secs_f64() * 1000.0
            ));
        }
    }

    /// Renders an owner-drawn icon offscreen through `WM_MEASUREITEM` /
    /// `WM_DRAWITEM`. The item is drawn on black and on white so the alpha
    /// channel can be recovered even when the handler draws with plain GDI.
    fn render_owner_drawn(
        &self,
        hmenu: HMENU,
        id: u32,
        item_data: usize,
        crop_icon: bool,
        ctx: &egui::Context,
    ) -> Option<egui::ColorImage> {
        let icon_px = (MENU_ICON_SIZE * ctx.pixels_per_point()).round().max(1.0) as i32;
        let mut measure = MEASUREITEMSTRUCT {
            CtlType: ODT_MENU,
            CtlID: 0,
            itemID: id,
            itemWidth: 0,
            itemHeight: 0,
            itemData: item_data,
        };
        self.handler.handle(
            WM_MEASUREITEM,
            WPARAM(0),
            LPARAM(&mut measure as *mut MEASUREITEMSTRUCT as isize),
        )?;
        let mut width = measure.itemWidth as i32;
        let mut height = measure.itemHeight as i32;
        if width <= 0 || height <= 0 {
            if crop_icon {
                return None;
            }
            width = icon_px;
            height = icon_px;
        }
        let width = width.clamp(1, 1024);
        let height = height.clamp(1, 256);

        let on_black = draw_item_offscreen(&self.handler, hmenu, id, item_data, width, height, 0)?;
        let on_white =
            draw_item_offscreen(&self.handler, hmenu, id, item_data, width, height, 0xFF)?;
        let mut rgba = composite_black_white(&on_black, &on_white);
        let (mut w, mut h) = (width as usize, height as usize);
        if crop_icon {
            (rgba, w, h) = crop_owner_drawn_icon(&rgba, w, h)?;
        }
        if rgba.chunks_exact(4).all(|px| px[3] == 0) {
            return None;
        }
        Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
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
    /// populated and owner-drawn submenus work as in Explorer. For a
    /// [`Self::for_paths`] menu, hidden entries are removed from the root
    /// before showing it and from submenus when the shell has populated them;
    /// a [`Self::standard_for_paths`] menu is shown complete.
    ///
    /// Show a menu at most once (see [`Self::standard_for_paths`]).
    pub fn show_native(&self, hwnd: HWND) -> windows::core::Result<()> {
        // A modal native menu at the real cursor would hijack the user's session.
        if crate::core::probe_active() {
            return Ok(());
        }
        let mut point = windows::Win32::Foundation::POINT::default();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point)?;
        }
        // No early return between pruning and handing the detached submenus
        // over to `self` (destroyed in `Drop`, after `invoke`).
        let pruner = (self.kind == ShellMenuKind::Filtered).then(|| {
            Rc::new(NativePruner::new(
                self.context_menu.clone(),
                self.id_base,
                self.menu,
            ))
        });
        if let Some(pruner) = &pruner {
            pruner.prune(self.menu);
        }
        let started = Instant::now();
        let command = {
            let _active = ActiveNativeMenuGuard::install(ActiveNativeMenu {
                handler: self.handler.clone(),
                pruner: pruner.clone(),
            });
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
        if let Some(pruner) = &pruner {
            self.detached_submenus
                .borrow_mut()
                .extend(pruner.detached.borrow_mut().drain(..));
        }
        let command = command.0 as u32;
        debug_log(format_args!(
            "native {:?} menu closed after {:.0} ms, command {command}",
            self.kind,
            started.elapsed().as_secs_f64() * 1000.0
        ));
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
            for submenu in self.detached_submenus.get_mut().drain(..) {
                let _ = DestroyMenu(submenu);
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

// ---------------------------------------------------------------------------
// Reading HMENUs
// ---------------------------------------------------------------------------

/// Diagnostics for one collected level.
#[derive(Default)]
struct CollectStats {
    /// Owner-drawn items without a text label.
    owner_draw_skipped: u32,
    /// Bitmap-only items (`hbmpItem` set, no text).
    bitmap_skipped: u32,
    /// Other items without a text label (or without a command id).
    unlabeled_skipped: u32,
    /// Hidden by the user.
    hidden: u32,
    /// Icon sources of the listed entries.
    icon_bitmap: u32,
    icon_callback: u32,
    icon_owner_draw: u32,
    icon_system: u32,
    icon_none: u32,
    /// Entries keyed by their canonical verb, and the time to look them up.
    verbs: u32,
    verb_time: Duration,
}

impl std::fmt::Display for CollectStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "; icons: bitmap {}, callback {}, owner-draw {}, system {}, none {}; verbs {} in {:.2} ms",
            self.icon_bitmap,
            self.icon_callback,
            self.icon_owner_draw,
            self.icon_system,
            self.icon_none,
            self.verbs,
            self.verb_time.as_secs_f64() * 1000.0,
        )?;
        if self.hidden > 0 {
            write!(f, "; hidden {}", self.hidden)?;
        }
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

struct MenuItemRead {
    info: MENUITEMINFOW,
    /// Sanitized label (empty when the item has no text).
    label: String,
}

/// Reads the item at `index` with `mask` (plus `MIIM_STRING`/`MIIM_SUBMENU`)
/// and its sanitized label.
fn read_menu_item(
    menu: HMENU,
    index: u32,
    mask: windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_MASK,
    buffer: &mut Vec<u16>,
) -> Option<MenuItemRead> {
    // First query the text length, then read the text into a buffer of
    // exactly that size (labels are not limited to a fixed length).
    let mut info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: mask | MIIM_STRING | MIIM_SUBMENU | MIIM_FTYPE,
        ..Default::default()
    };
    unsafe { GetMenuItemInfoW(menu, index, true, &mut info) }.ok()?;
    if info.fType.contains(MFT_SEPARATOR) || info.cch == 0 {
        return Some(MenuItemRead {
            info,
            label: String::new(),
        });
    }

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
    let label = if unsafe { GetMenuItemInfoW(menu, index, true, &mut text_info) }.is_err() {
        String::new()
    } else {
        let len = (text_info.cch as usize).min(buffer.len());
        sanitize_label(&String::from_utf16_lossy(&buffer[..len]))
    };
    Some(MenuItemRead { info, label })
}

/// Classifies the icon of a menu item.
fn icon_source(menu: HMENU, info: &MENUITEMINFOW, stats: &mut CollectStats) -> IconSource {
    if info.fType.contains(MFT_OWNERDRAW) {
        stats.icon_owner_draw += 1;
        return IconSource::OwnerDraw {
            hmenu: menu,
            id: info.wID,
            item_data: info.dwItemData,
        };
    }
    match info.hbmpItem.0 as isize {
        0 => {
            stats.icon_none += 1;
            IconSource::None
        }
        // HBMMENU_CALLBACK
        -1 => {
            stats.icon_callback += 1;
            IconSource::Callback {
                hmenu: menu,
                id: info.wID,
                item_data: info.dwItemData,
            }
        }
        // HBMMENU_SYSTEM .. HBMMENU_POPUP_MINIMIZE: window-menu glyphs.
        1..=11 => {
            stats.icon_system += 1;
            IconSource::None
        }
        _ => {
            stats.icon_bitmap += 1;
            IconSource::Bitmap(info.hbmpItem)
        }
    }
}

/// Reads one level of `menu`. Submenus are recorded with their HMENU but
/// not descended into (see [`ShellContextMenu::submenu_children`]).
///
/// Returns the visible nodes (hidden entries removed) and every labeled
/// entry of the level for the settings page.
fn collect_level(
    menu: HMENU,
    level: &LevelContext<'_>,
    stats: &mut CollectStats,
) -> (Vec<ShellMenuNode>, Vec<SeenMenuEntry>) {
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    if count <= 0 {
        return (Vec::new(), Vec::new());
    }

    // Read every item first; keys need the whole level (duplicate verbs).
    struct Entry {
        node: ShellMenuNode,
        /// `(command id, label)` for labeled entries, `None` for separators.
        key_input: Option<(Option<u32>, String)>,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(count as usize);
    let mut buffer: Vec<u16> = Vec::new();
    for index in 0..count as u32 {
        let mask = MIIM_ID | MIIM_STATE | MIIM_BITMAP | MIIM_DATA;
        let Some(MenuItemRead { info, label }) = read_menu_item(menu, index, mask, &mut buffer)
        else {
            continue;
        };

        if info.fType.contains(MFT_SEPARATOR) {
            entries.push(Entry {
                node: ShellMenuNode::Separator,
                key_input: None,
            });
            continue;
        }

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
            let mut path = level.parents.to_vec();
            path.push(label.clone());
            entries.push(Entry {
                node: ShellMenuNode::Submenu(ShellSubmenu {
                    label: label.clone(),
                    disabled,
                    icon: MenuIcon::new(icon_source(menu, &info, stats)),
                    key: String::new(),
                    path,
                    hmenu: info.hSubMenu,
                    index_in_parent: index,
                    initialized_at: OnceCell::new(),
                    children: RefCell::new(None),
                }),
                key_input: Some((None, label)),
            });
            continue;
        }

        if info.wID == 0 {
            stats.unlabeled_skipped += 1;
            continue;
        }

        entries.push(Entry {
            node: ShellMenuNode::Command {
                id: info.wID,
                label: label.clone(),
                disabled,
                is_default: info.fState.contains(MFS_DEFAULT),
                icon: MenuIcon::new(icon_source(menu, &info, stats)),
            },
            key_input: Some((Some(info.wID), label)),
        });
    }

    let key_input: Vec<(Option<u32>, &str)> = entries
        .iter()
        .filter_map(|entry| entry.key_input.as_ref())
        .map(|(id, label)| (*id, label.as_str()))
        .collect();
    let mut keys = level_keys(level, &key_input, stats).into_iter();

    let hidden = hidden_set();
    let mut seen = Vec::new();
    let mut nodes = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some((_, label)) = &entry.key_input else {
            nodes.push(entry.node);
            continue;
        };
        let key = keys.next().unwrap_or_default();
        seen.push(SeenMenuEntry {
            key: key.clone(),
            label: label.clone(),
            path_label: path_label(level.parents, label),
            depth: level.parents.len(),
            is_submenu: matches!(entry.node, ShellMenuNode::Submenu(_)),
            parent: String::new(),
        });
        if hidden.contains(&key) {
            stats.hidden += 1;
            continue;
        }
        let mut node = entry.node;
        if let ShellMenuNode::Submenu(submenu) = &mut node {
            submenu.key = key;
        }
        nodes.push(node);
    }

    normalize_separators(&mut nodes);
    (nodes, seen)
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

// ---------------------------------------------------------------------------
// Bitmaps
// ---------------------------------------------------------------------------

/// Converts a menu item bitmap (usually a 32bpp premultiplied DIB section) to
/// an egui image.
fn bitmap_to_image(bitmap: HBITMAP) -> Option<egui::ColorImage> {
    unsafe {
        let mut bmp = BITMAP::default();
        if GetObjectW(
            HGDIOBJ(bitmap.0),
            size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut BITMAP as *mut _),
        ) == 0
        {
            return None;
        }
        let width = bmp.bmWidth;
        let height = bmp.bmHeight.abs();
        if !(1..=256).contains(&width) || !(1..=256).contains(&height) {
            return None;
        }

        // Negative height: rows come back top-down whatever the source
        // orientation is.
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }

        bgra_to_straight_rgba(&mut pixels, bmp.bmBitsPixel == 32);
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            &pixels,
        ))
    }
}

/// BGRA → RGBA in place. Bitmaps without alpha (or with an all-zero alpha
/// channel) are opaque; premultiplied pixels are converted to straight alpha.
fn bgra_to_straight_rgba(pixels: &mut [u8], has_alpha_channel: bool) {
    let alpha_present = has_alpha_channel && pixels.chunks_exact(4).any(|px| px[3] != 0);
    // Premultiplied data never has a color channel above alpha.
    let premultiplied = alpha_present
        && pixels
            .chunks_exact(4)
            .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]);
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        if !alpha_present {
            px[3] = 255;
        } else if premultiplied && px[3] > 0 && px[3] < 255 {
            let alpha = px[3] as u32;
            for channel in &mut px[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// Sends `WM_DRAWITEM` for a menu item into a 32bpp DIB filled with
/// `background` (all four channels) and returns the BGRA pixels.
fn draw_item_offscreen(
    handler: &MenuMsgHandler,
    hmenu: HMENU,
    id: u32,
    item_data: usize,
    width: i32,
    height: i32,
    background: u8,
) -> Option<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) if !bits.is_null() => dib,
            Ok(dib) => {
                let _ = DeleteObject(dib.into());
                let _ = DeleteDC(dc);
                return None;
            }
            Err(_) => {
                let _ = DeleteDC(dc);
                return None;
            }
        };
        let previous = SelectObject(dc, dib.into());
        let len = width as usize * height as usize * 4;
        std::ptr::write_bytes(bits.cast::<u8>(), background, len);

        let draw = DRAWITEMSTRUCT {
            CtlType: ODT_MENU,
            CtlID: 0,
            itemID: id,
            itemAction: ODA_DRAWENTIRE,
            itemState: ODS_FLAGS(0),
            hwndItem: HWND(hmenu.0),
            hDC: dc,
            rcItem: RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
            itemData: item_data,
        };
        let handled = handler
            .handle(
                WM_DRAWITEM,
                WPARAM(0),
                LPARAM(&draw as *const DRAWITEMSTRUCT as isize),
            )
            .is_some();
        let _ = GdiFlush();
        let pixels = handled.then(|| std::slice::from_raw_parts(bits.cast::<u8>(), len).to_vec());

        SelectObject(dc, previous);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(dc);
        pixels
    }
}

/// Recovers straight RGBA from the same drawing on black and on white
/// (BGRA): `alpha = 1 - (white - black)`, `color = black / alpha`.
fn composite_black_white(on_black: &[u8], on_white: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(on_black.len());
    for (black, white) in on_black.chunks_exact(4).zip(on_white.chunks_exact(4)) {
        let diff: u32 = (0..3)
            .map(|c| white[c].saturating_sub(black[c]) as u32)
            .sum::<u32>();
        let alpha = 255 - (diff / 3).min(255);
        let straight = |c: u8| {
            if alpha == 0 {
                0
            } else {
                ((c as u32 * 255 + alpha / 2) / alpha).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[
            straight(black[2]),
            straight(black[1]),
            straight(black[0]),
            alpha as u8,
        ]);
    }
    rgba
}

/// Crops the icon from a fully owner-drawn item: within the left square of
/// the item, the bounding box of the pixels that differ from the item
/// background (top-left pixel), made square. Background pixels become
/// transparent.
fn crop_owner_drawn_icon(
    rgba: &[u8],
    width: usize,
    height: usize,
) -> Option<(Vec<u8>, usize, usize)> {
    let square = height.min(width);
    let background = &rgba[..4];
    let pixel = |x: usize, y: usize| &rgba[(y * width + x) * 4..(y * width + x) * 4 + 4];
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..height {
        for x in 0..square {
            let px = pixel(x, y);
            if px[3] != 0 && px != background {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x == usize::MAX {
        return None;
    }
    let side = (max_x - min_x + 1).max(max_y - min_y + 1);
    let x0 = min_x.min(width.saturating_sub(side));
    let y0 = (min_y + max_y + 1).saturating_sub(side) / 2;
    let y0 = y0.min(height.saturating_sub(side));
    let side = side.min(width - x0).min(height - y0);

    let mut out = Vec::with_capacity(side * side * 4);
    for y in y0..y0 + side {
        for x in x0..x0 + side {
            let px = pixel(x, y);
            if px == background {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                out.extend_from_slice(px);
            }
        }
    }
    Some((out, side, side))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::GetSubMenu;

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
            icon: MenuIcon::none(),
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

    #[test]
    fn entry_keys() {
        assert_eq!(entry_key(Some("open"), &[], "開く"), "verb:open");
        assert_eq!(entry_key(None, &[], "送る"), "label:送る");
        assert_eq!(
            entry_key(None, &["送る".to_string()], "ドキュメント"),
            "label:送る > ドキュメント"
        );
    }

    #[test]
    fn eden_keys_cannot_collide_with_shell_keys() {
        use crate::gui::windows::eden_menu_entries::{EDEN_KEY_PREFIX, EdenMenuEntry};
        // Shell keys always start with `verb:` or `label:`, whatever the
        // verb or label is (even one that looks like an Eden key).
        for (verb, parents, label) in [
            (Some("eden:copy"), vec![], "x"),
            (None, vec![], "eden:copy"),
            (None, vec!["eden:copy".to_string()], "x"),
            (Some("copy"), vec![], "Copy"),
        ] {
            let key = entry_key(verb, &parents, label);
            assert!(
                key.starts_with("verb:") || key.starts_with("label:"),
                "{key}"
            );
            assert!(!key.starts_with(EDEN_KEY_PREFIX), "{key}");
        }
        for entry in EdenMenuEntry::ALL {
            assert!(!entry.key().starts_with("verb:") && !entry.key().starts_with("label:"));
        }
    }

    #[test]
    fn verbs_must_be_printable_ascii() {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        assert_eq!(parse_verb(&wide("open")), Some("open".to_string()));
        assert_eq!(parse_verb(&wide("  ")), None);
        // "open" written as ANSI into the wide buffer.
        assert_eq!(parse_verb(&[0x706F, 0x6E65, 0]), None);
    }

    #[test]
    fn bitmap_alpha_handling() {
        // Premultiplied half-transparent white -> straight white.
        let mut px = vec![128, 128, 128, 128];
        bgra_to_straight_rgba(&mut px, true);
        assert_eq!(px, vec![255, 255, 255, 128]);

        // All-zero alpha: opaque, BGR swapped.
        let mut px = vec![10, 20, 30, 0, 40, 50, 60, 0];
        bgra_to_straight_rgba(&mut px, true);
        assert_eq!(px, vec![30, 20, 10, 255, 60, 50, 40, 255]);

        // Straight (non-premultiplied) data is kept.
        let mut px = vec![200, 0, 0, 100];
        bgra_to_straight_rgba(&mut px, true);
        assert_eq!(px, vec![0, 0, 200, 100]);
    }

    /// Builds a native menu from `(label, kind)`; kind: 'c' command,
    /// 's' submenu (with one child), '-' separator.
    fn native_menu(items: &[(&str, char)]) -> HMENU {
        use windows::Win32::UI::WindowsAndMessaging::{
            AppendMenuW, MF_POPUP, MF_SEPARATOR, MF_STRING,
        };
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        unsafe {
            let menu = CreatePopupMenu().unwrap();
            for (index, (label, kind)) in items.iter().enumerate() {
                let text = wide(label);
                match kind {
                    '-' => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).unwrap(),
                    's' => {
                        let sub = CreatePopupMenu().unwrap();
                        let child = wide("child");
                        AppendMenuW(sub, MF_STRING, 900, PCWSTR(child.as_ptr())).unwrap();
                        AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(text.as_ptr())).unwrap();
                    }
                    _ => AppendMenuW(menu, MF_STRING, index + 1, PCWSTR(text.as_ptr())).unwrap(),
                }
            }
            menu
        }
    }

    fn native_labels(menu: HMENU) -> Vec<String> {
        let mut buffer = Vec::new();
        (0..unsafe { GetMenuItemCount(Some(menu)) } as u32)
            .map(|index| {
                let item = read_menu_item(menu, index, MIIM_FTYPE, &mut buffer).unwrap();
                if item.info.fType.contains(MFT_SEPARATOR) {
                    "-".to_string()
                } else {
                    item.label
                }
            })
            .collect()
    }

    #[test]
    fn native_pruning_removes_hidden_entries() {
        use windows::Win32::UI::WindowsAndMessaging::IsMenu;
        let menu = native_menu(&[
            ("-", '-'),
            ("Open", 'c'),
            ("-", '-'),
            ("&Hide A", 'c'),
            ("Hide B", 'c'),
            ("-", '-'),
            ("Send to", 's'),
            ("-", '-'),
            ("Keep", 's'),
            ("Properties", 'c'),
            ("-", '-'),
            ("Hide C", 'c'),
        ]);
        let send_to = unsafe { GetSubMenu(menu, 6) };
        let hidden: HashSet<String> = [
            "label:Hide A",
            "label:Hide B",
            "label:Send to",
            "label:Hide C",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let pruner = NativePruner::with_hidden(None, CONTEXT_MENU_ID_BASE, menu, hidden);
        pruner.prune(menu);

        assert_eq!(native_labels(menu), vec!["Open", "-", "Keep", "Properties"]);
        // The hidden submenu is detached, not destroyed (the shell may still
        // reference it); the visible one can be pruned on WM_INITMENUPOPUP.
        let detached = pruner.detached.borrow().clone();
        assert_eq!(detached, vec![send_to]);
        assert!(unsafe { IsMenu(send_to) }.as_bool());
        let keep = unsafe { GetSubMenu(menu, 2) };
        assert!(pruner.menus.borrow().contains_key(&(keep.0 as usize)));

        unsafe {
            DestroyMenu(menu).unwrap();
            for submenu in detached {
                DestroyMenu(submenu).unwrap();
                assert!(!IsMenu(submenu).as_bool());
            }
        }
    }

    #[test]
    fn black_white_compositing() {
        // Opaque red, transparent, 50% black.
        let black = [0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let white = [0, 0, 255, 255, 255, 255, 255, 255, 128, 128, 128, 255];
        let rgba = composite_black_white(&black, &white);
        assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(rgba[7], 0);
        assert_eq!(&rgba[8..12], &[0, 0, 0, 127]);
    }
}
