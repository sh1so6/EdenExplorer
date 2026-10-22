//! The Windows shell context menu of files and folders, shown natively
//! (`TrackPopupMenuEx`) with an optional application submenu at the top.
//!
//! Also keeps the user's hidden-entry preferences (`context_menu.json`) and
//! the entries of the most recently opened menu for the settings page.

use crate::core::indexer::{load_context_menu_settings, save_context_menu_settings};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::mem::{ManuallyDrop, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE,
    CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    DEFCONTEXTMENU, GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, ILCreateFromPathW,
    ILFindLastID, IShellFolder, SHBindToParent, SHCreateDefaultContextMenu,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DeleteMenu, DestroyMenu, GetCursorPos, GetMenuItemCount,
    GetMenuItemInfoW, HMENU, InsertMenuItemW, MENUITEMINFOW, MF_BYPOSITION, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, MFT_SEPARATOR, MFT_STRING, MIIM_FTYPE, MIIM_ID, MIIM_STRING,
    MIIM_SUBMENU, RemoveMenu, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_DRAWITEM,
    WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};
use windows::core::{Error, HRESULT, Interface, PCSTR, PCWSTR, PSTR, PWSTR};

/// Command ids handed to the shell (`QueryContextMenu`'s inclusive
/// `idCmdFirst..=idCmdLast`). 0 is what `TrackPopupMenuEx` returns when the
/// menu is cancelled, so no command may use it.
const CONTEXT_MENU_ID_BASE: u32 = 1;
const CONTEXT_MENU_ID_MAX: u32 = 0x7FFF;
/// Command ids of the application submenu's entries, right after the shell
/// range (never handed to the shell, so they cannot collide with its ids).
/// Menu ids are 16-bit in `WM_COMMAND`; the range stays below `0xFFFF`.
const APP_MENU_ID_BASE: u32 = 0x8000;
const APP_MENU_ID_MAX: u32 = 0xBFFF;
/// `CMIC_MASK_UNICODE` (same value as `SEE_MASK_UNICODE`; not exported by
/// the `windows` crate).
const CMIC_MASK_UNICODE: u32 = 0x0000_4000;

/// Debug/probe-only diagnostics. Release builds compile the call to a no-op,
/// but the arguments still count as used so no cfg juggling is needed at the
/// call sites. Lines from other threads than the UI thread are tagged with the
/// thread name.
#[inline(always)]
pub fn debug_log(args: std::fmt::Arguments<'_>) {
    #[cfg(any(debug_assertions, feature = "ui-probe"))]
    match std::thread::current().name() {
        Some("main") => eprintln!("[shell-menu] {args}"),
        name => eprintln!("[shell-menu@{}] {args}", name.unwrap_or("?")),
    }
    #[cfg(not(any(debug_assertions, feature = "ui-probe")))]
    let _ = args;
}

// ---------------------------------------------------------------------------
// Hidden entries (user preference, persisted in `context_menu.json`)
// ---------------------------------------------------------------------------

/// An entry of the most recently shown shell menu (root and every submenu
/// opened since), listed on the settings page so it can be hidden.
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
    seen: Vec<SeenMenuEntry>,
    /// Hidden entries of Eden's own item menu (`eden:` keys, see
    /// [`crate::gui::windows::eden_menu_entries`]). Kept apart from `hidden`
    /// so they never show up among the shell entries in the settings.
    eden_hidden: HashSet<String>,
    /// Bumped on every change of `eden_hidden` (see
    /// [`eden_entries_version`]).
    eden_version: u64,
}

impl MenuPrefs {
    fn load() -> Self {
        let snapshot = load_context_menu_settings();
        Self {
            hidden: snapshot.hidden.into_iter().collect(),
            labels: snapshot.labels,
            seen: Vec::new(),
            eden_hidden: snapshot.eden_hidden.into_iter().collect(),
            eden_version: 0,
        }
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
            prefs.save();
        }
    });
}

/// Shows every hidden shell entry again.
pub fn reset_hidden_entries() {
    with_prefs(|prefs| {
        if !prefs.hidden.is_empty() {
            prefs.hidden.clear();
            prefs.save();
        }
    });
}

/// Changes whenever the set of hidden Eden entries changes.
pub fn eden_entries_version() -> u64 {
    with_prefs(|prefs| prefs.eden_version)
}

/// Keys of the hidden Eden item-menu entries.
pub fn hidden_eden_entries() -> HashSet<String> {
    with_prefs(|prefs| prefs.eden_hidden.clone())
}

/// Hides or shows one of Eden's own item-menu entries and saves the
/// preference immediately.
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

/// Replaces the top-level entries after the root of the same menu was read
/// again, keeping the children recorded for its submenus since.
fn update_seen_root(entries: Vec<SeenMenuEntry>) {
    with_prefs(|prefs| {
        // Children follow their top-level entry (depth first).
        let mut children: HashMap<String, Vec<SeenMenuEntry>> = HashMap::new();
        let mut top_level: Option<String> = None;
        for entry in std::mem::take(&mut prefs.seen) {
            if entry.depth == 0 {
                top_level = Some(entry.key.clone());
            } else if let Some(key) = &top_level {
                children.entry(key.clone()).or_default().push(entry);
            }
        }
        for entry in entries {
            let kept = children.remove(&entry.key);
            prefs.seen.push(entry);
            prefs.seen.extend(kept.into_iter().flatten());
        }
    });
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
// Shell menu messages
// ---------------------------------------------------------------------------

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
// Native (TrackPopupMenuEx) menu
// ---------------------------------------------------------------------------

/// Removes hidden entries from the native HMENU tree and records the entries
/// for the settings page: the root before the menu is shown and each submenu
/// after the shell populated it (`WM_INITMENUPOPUP`). Items a shell extension
/// adds later asynchronously are not seen (best effort).
struct NativePruner {
    context_menu: Option<IContextMenu>,
    id_base: u32,
    hidden: HashSet<String>,
    root: HMENU,
    /// Menus that may be pruned: HMENU -> (entry key, label path). The root
    /// has an empty key and path.
    menus: RefCell<HashMap<usize, (String, Vec<String>)>>,
    /// Removed (not destroyed) submenus; the shell may still reference them
    /// while the menu is alive, so they are destroyed with the menu.
    detached: RefCell<Vec<HMENU>>,
    /// Entries removed from a menu by an earlier pass. A submenu shown again
    /// is pruned (and recorded) again, but the HMENU no longer contains them;
    /// they stay listed in place of the settings page all the same.
    removed: RefCell<HashMap<usize, Vec<SeenMenuEntry>>>,
    /// The application submenu inserted into the root: never pruned,
    /// recorded or registered for pruning.
    app_menu: Cell<Option<HMENU>>,
    /// Root item count once the menu was prepared. `WM_INITMENUPOPUP` for the
    /// root finds nothing new to prune while the count is unchanged.
    settled_root_count: Cell<Option<i32>>,
    /// Whether the root was recorded (a new menu replaces the entries of the
    /// previous one; a later read of the root keeps the submenu children).
    root_recorded: Cell<bool>,
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
            root,
            menus: RefCell::new(menus),
            detached: RefCell::new(Vec::new()),
            removed: RefCell::new(HashMap::new()),
            app_menu: Cell::new(None),
            settled_root_count: Cell::new(None),
            root_recorded: Cell::new(false),
        }
    }

    fn prune(&self, menu: HMENU) {
        let Some((menu_key, parents)) = self.menus.borrow().get(&(menu.0 as usize)).cloned() else {
            return;
        };
        let count = unsafe { GetMenuItemCount(Some(menu)) };
        let is_root = menu == self.root;
        if is_root && self.settled_root_count.get() == Some(count) {
            return;
        }
        let started = Instant::now();
        let app_menu = self.app_menu.get();

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
            if submenu.is_some() && submenu == app_menu {
                continue;
            }
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

        let mut seen: Vec<SeenMenuEntry> = items
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
            .collect();

        // Back to front, so the positions of the remaining items stay valid.
        let mut removed = 0;
        let mut removed_entries = Vec::new();
        for (((index, label, _, submenu), key), entry) in
            items.iter().zip(&keys).zip(&seen).rev()
        {
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
                    removed_entries.push(entry.clone());
                }
            } else if let Some(submenu) = submenu {
                let mut path = parents.clone();
                path.push(label.clone());
                self.menus
                    .borrow_mut()
                    .insert(submenu.0 as usize, (key.clone(), path));
            }
        }

        {
            let mut removed_by_menu = self.removed.borrow_mut();
            let earlier = removed_by_menu.entry(menu.0 as usize).or_default();
            for entry in earlier.iter() {
                if !seen.iter().any(|seen| seen.key == entry.key) {
                    seen.push(entry.clone());
                }
            }
            earlier.extend(removed_entries.into_iter().rev());
        }
        if is_root {
            if self.root_recorded.replace(true) {
                update_seen_root(seen);
            } else {
                record_seen_root(seen);
            }
        } else {
            record_seen_children(&menu_key, seen);
        }

        if removed > 0 {
            normalize_native_separators(menu);
        }
        debug_log(format_args!(
            "prune '{}': {} entries, {removed} hidden removed in {:.2} ms ({stats})",
            parents.join(" > "),
            items.len(),
            started.elapsed().as_secs_f64() * 1000.0,
        ));
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
    /// The application submenu (as `usize`), whose messages are not the
    /// shell's business.
    app_menu: Option<usize>,
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
///
/// Messages about the application submenu are left to the default handling:
/// the shell does not know that menu.
pub fn forward_native_menu_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let target_menu = match msg {
        WM_INITMENUPOPUP => Some(wparam.0),
        WM_MENUCHAR => Some(lparam.0 as usize),
        // wParam is 0 when the message was sent for a menu (not a control).
        WM_DRAWITEM | WM_MEASUREITEM if wparam.0 == 0 => None,
        _ => return None,
    };

    // Clone the handler out so the RefCell is not borrowed while the shell
    // extension runs (it may pump messages and re-enter the wndproc).
    let active =
        ACTIVE_NATIVE_MENU.with(|slot| slot.try_borrow().ok().and_then(|active| active.clone()))?;
    if target_menu.is_some() && target_menu == active.app_menu {
        return None;
    }
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
// Application submenu
// ---------------------------------------------------------------------------

/// An entry of the application's own submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppMenuItem {
    Command { label: String, enabled: bool },
    Separator,
}

/// The application's own entries, shown as a submenu at the top of the shell
/// menu.
#[derive(Clone, Copy)]
pub struct AppSubmenu<'a> {
    pub label: &'a str,
    pub items: &'a [AppMenuItem],
}

/// What the user picked in a native menu.
pub enum NativeMenuChoice {
    /// Closed without a command.
    Cancelled,
    /// The application entry at this index of [`AppSubmenu::items`].
    App(usize),
    /// A shell command; run it with [`ShellContextMenu::invoke`] (on the same
    /// menu) unless the caller handles its verb itself.
    Shell(ShellCommand),
}

/// A shell command picked in a [`ShellContextMenu`].
pub struct ShellCommand {
    id: u32,
    verb: Option<String>,
}

impl ShellCommand {
    /// The canonical verb (`GCS_VERBW`), e.g. `rename`, when the handler
    /// provides one.
    pub fn verb(&self) -> Option<&str> {
        self.verb.as_deref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandTarget {
    Cancelled,
    App(usize),
    Shell(u32),
    /// Outside both ranges (a misbehaving shell extension).
    Unknown,
}

fn command_target(id: u32) -> CommandTarget {
    match id {
        0 => CommandTarget::Cancelled,
        CONTEXT_MENU_ID_BASE..=CONTEXT_MENU_ID_MAX => CommandTarget::Shell(id),
        APP_MENU_ID_BASE..=APP_MENU_ID_MAX => {
            CommandTarget::App((id - APP_MENU_ID_BASE) as usize)
        }
        _ => CommandTarget::Unknown,
    }
}

fn app_command_id(index: usize) -> Option<u32> {
    u32::try_from(index)
        .ok()
        .and_then(|index| APP_MENU_ID_BASE.checked_add(index))
        .filter(|id| *id <= APP_MENU_ID_MAX)
}

/// Menu text for a plain label: `&` would start a mnemonic.
fn escape_mnemonics(label: &str) -> String {
    label.replace('&', "&&")
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// A popup menu with the application entries (ids from [`app_command_id`]).
fn create_app_menu(items: &[AppMenuItem]) -> windows::core::Result<HMENU> {
    let menu = unsafe { CreatePopupMenu()? };
    for (index, item) in items.iter().enumerate() {
        let result = match item {
            AppMenuItem::Separator => unsafe {
                AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())
            },
            AppMenuItem::Command { label, enabled } => {
                let Some(id) = app_command_id(index) else {
                    break;
                };
                let text = to_wide(&escape_mnemonics(label));
                let flags = if *enabled {
                    MF_STRING
                } else {
                    MF_STRING | MF_GRAYED
                };
                unsafe { AppendMenuW(menu, flags, id as usize, PCWSTR(text.as_ptr())) }
            }
        };
        if let Err(err) = result {
            let _ = unsafe { DestroyMenu(menu) };
            return Err(err);
        }
    }
    Ok(menu)
}

/// Inserts the application entries as a submenu at the top of `root`,
/// followed by a separator. Returns the submenu, owned by `root` from then
/// on (destroyed with it).
fn insert_app_submenu(root: HMENU, app: AppSubmenu<'_>) -> windows::core::Result<HMENU> {
    let submenu = create_app_menu(app.items)?;
    let mut text = to_wide(&escape_mnemonics(app.label));
    let info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_FTYPE | MIIM_STRING | MIIM_SUBMENU,
        fType: MFT_STRING,
        hSubMenu: submenu,
        dwTypeData: PWSTR(text.as_mut_ptr()),
        ..Default::default()
    };
    if let Err(err) = unsafe { InsertMenuItemW(root, 0, true, &info) } {
        let _ = unsafe { DestroyMenu(submenu) };
        return Err(err);
    }
    if unsafe { GetMenuItemCount(Some(root)) } > 1 {
        let separator = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE,
            fType: MFT_SEPARATOR,
            ..Default::default()
        };
        let _ = unsafe { InsertMenuItemW(root, 1, true, &separator) };
    }
    Ok(submenu)
}

/// Where a native menu opens: `point` (screen coordinates) or the cursor.
fn menu_position(point: Option<(i32, i32)>) -> windows::core::Result<POINT> {
    match point {
        Some((x, y)) => Ok(POINT { x, y }),
        None => {
            let mut point = POINT::default();
            unsafe { GetCursorPos(&mut point)? };
            Ok(point)
        }
    }
}

/// Shows only the application entries as a native menu (for when no shell
/// menu can be built). Returns the index of the picked entry.
pub fn show_app_menu(
    hwnd: HWND,
    point: Option<(i32, i32)>,
    items: &[AppMenuItem],
) -> windows::core::Result<Option<usize>> {
    // A modal native menu at the real cursor would hijack the user's session.
    if items.is_empty() || crate::core::probe_active() {
        return Ok(None);
    }
    let point = menu_position(point)?;
    let menu = create_app_menu(items)?;
    let command = unsafe {
        TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            point.x,
            point.y,
            hwnd,
            None,
        )
    };
    let _ = unsafe { DestroyMenu(menu) };
    Ok(match command_target(command.0 as u32) {
        CommandTarget::App(index) => Some(index),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Shell context menu
// ---------------------------------------------------------------------------

/// A shell context menu for some files or folders of one folder. Build a new
/// one for every showing: shell extensions may append their items again to a
/// reused HMENU.
pub struct ShellContextMenu {
    context_menu: IContextMenu,
    handler: MenuMsgHandler,
    menu: HMENU,
    id_base: u32,
    full_pidls: Vec<*mut ITEMIDLIST>,
    parent_pidl: *mut ITEMIDLIST,
    /// The folder containing the items (working directory of commands).
    directory: PathBuf,
    kind: ShellMenuKind,
    /// Submenus removed by the pruning (destroyed on drop).
    detached_submenus: RefCell<Vec<HMENU>>,
}

/// How a [`ShellContextMenu`] is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellMenuKind {
    /// `CMF_NORMAL`, hidden entries removed and the entries recorded for the
    /// settings page.
    Filtered,
    /// `CMF_NORMAL | CMF_EXTENDEDVERBS` (Explorer's Shift+right-click menu),
    /// unfiltered and nothing recorded (extended-only verbs would otherwise
    /// flood the settings list).
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
    /// The normal right-click menu: hidden entries removed, its entries
    /// listed on the settings page.
    pub fn for_paths(paths: &[PathBuf], hwnd: HWND) -> windows::core::Result<Self> {
        Self::build(paths, hwnd, ShellMenuKind::Filtered)
    }

    /// The complete Windows menu including the extended verbs, as Explorer
    /// shows it on Shift+right-click. Nothing is hidden or recorded.
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
            id_base: CONTEXT_MENU_ID_BASE,
            full_pidls,
            parent_pidl,
            directory: parent,
            kind,
            detached_submenus: RefCell::new(Vec::new()),
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

        debug_log(format_args!(
            "build {kind:?}: {} items in {:.2} ms (create {:.2} ms, QueryContextMenu {:.2} ms)",
            unsafe { GetMenuItemCount(Some(this.menu)) },
            started.elapsed().as_secs_f64() * 1000.0,
            created.as_secs_f64() * 1000.0,
            (queried - created).as_secs_f64() * 1000.0,
        ));
        Ok(this)
    }

    /// Runs a shell command picked in this menu.
    pub fn invoke(
        &self,
        hwnd: HWND,
        command: &ShellCommand,
        point: Option<(i32, i32)>,
    ) -> windows::core::Result<()> {
        let offset = command.id.saturating_sub(self.id_base) as usize;
        let directory: Vec<u16> = self
            .directory
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let key_down = |key| unsafe { GetKeyState(i32::from(key)) } < 0;
        let mut mask = CMIC_MASK_UNICODE;
        if key_down(VK_SHIFT.0) {
            mask |= CMIC_MASK_SHIFT_DOWN;
        }
        if key_down(VK_CONTROL.0) {
            mask |= CMIC_MASK_CONTROL_DOWN;
        }
        let mut info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            hwnd,
            // MAKEINTRESOURCE(offset) in both the ANSI and the wide field.
            lpVerb: PCSTR(offset as *const u8),
            lpVerbW: PCWSTR(offset as *const u16),
            lpDirectoryW: PCWSTR(directory.as_ptr()),
            nShow: 1, // SW_SHOWNORMAL
            ..Default::default()
        };
        if let Some((x, y)) = point {
            mask |= CMIC_MASK_PTINVOKE;
            info.ptInvoke = POINT { x, y };
        }
        info.fMask = mask;

        unsafe {
            self.context_menu
                .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
        }
    }

    /// Displays the shell menu at `point` (screen coordinates; the current
    /// cursor position when `None`), with `app` as a submenu at the top, and
    /// returns what the user picked. Shell commands are not run here (see
    /// [`NativeMenuChoice::Shell`]).
    ///
    /// While the menu is tracked, the owner window's procedure forwards menu
    /// messages to this menu (see [`forward_native_menu_message`]), so lazily
    /// populated and owner-drawn submenus work as in Explorer. For a
    /// [`Self::for_paths`] menu, hidden entries are removed from the root
    /// before showing it and from submenus when the shell has populated them;
    /// a [`Self::standard_for_paths`] menu is shown complete.
    ///
    /// Show a menu at most once.
    pub fn show_native(
        &self,
        hwnd: HWND,
        point: Option<(i32, i32)>,
        app: Option<AppSubmenu<'_>>,
    ) -> windows::core::Result<NativeMenuChoice> {
        // A modal native menu at the real cursor would hijack the user's
        // session; probe runs prepare the menu (for the timings) only.
        let probe = crate::core::probe_active();
        let position = if probe {
            POINT::default()
        } else {
            menu_position(point)?
        };
        let started = Instant::now();
        // No early return between pruning and handing the detached submenus
        // over to `self` (destroyed in `Drop`).
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
        let pruned = started.elapsed();
        let app_menu = app.filter(|app| !app.items.is_empty()).and_then(|app| {
            insert_app_submenu(self.menu, app)
                .inspect_err(|err| eprintln!("Adding the application submenu failed: {err}"))
                .ok()
        });
        if let Some(pruner) = &pruner {
            pruner.app_menu.set(app_menu);
            pruner
                .settled_root_count
                .set(Some(unsafe { GetMenuItemCount(Some(self.menu)) }));
        }
        debug_log(format_args!(
            "native {:?} menu prepared in {:.2} ms (prune {:.2} ms), {} root items",
            self.kind,
            started.elapsed().as_secs_f64() * 1000.0,
            pruned.as_secs_f64() * 1000.0,
            unsafe { GetMenuItemCount(Some(self.menu)) },
        ));

        let tracking = Instant::now();
        let command = if probe {
            0
        } else {
            let _active = ActiveNativeMenuGuard::install(ActiveNativeMenu {
                handler: self.handler.clone(),
                pruner: pruner.clone(),
                app_menu: app_menu.map(|menu| menu.0 as usize),
            });
            unsafe {
                TrackPopupMenuEx(
                    self.menu,
                    (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                    position.x,
                    position.y,
                    hwnd,
                    None,
                )
            }
            .0 as u32
        };
        if let Some(pruner) = &pruner {
            self.detached_submenus
                .borrow_mut()
                .extend(pruner.detached.borrow_mut().drain(..));
        }
        debug_log(format_args!(
            "native {:?} menu closed after {:.0} ms, command {command}",
            self.kind,
            tracking.elapsed().as_secs_f64() * 1000.0
        ));

        Ok(match command_target(command) {
            CommandTarget::Cancelled | CommandTarget::Unknown => NativeMenuChoice::Cancelled,
            CommandTarget::App(index) => NativeMenuChoice::App(index),
            CommandTarget::Shell(id) => NativeMenuChoice::Shell(ShellCommand {
                id,
                verb: command_verb(&self.context_menu, id - self.id_base),
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// Background warm-up
// ---------------------------------------------------------------------------

/// How long after startup the background warm-up starts (leaves the first
/// frames and the first listing alone).
const WARM_UP_DELAY: Duration = Duration::from_secs(1);
/// Folders remembered as warmed (oldest forgotten first).
const WARMED_FOLDERS_CAP: usize = 256;

/// Requests for [`warm_up_worker`]. Set once by [`warm_up_in_background`];
/// unset when per-folder warm-ups are disabled.
static WARM_UP_REQUESTS: std::sync::OnceLock<WarmUpRequests> = std::sync::OnceLock::new();

/// Sending side of the warm-up worker's requests: items whose menus are
/// built and dropped, and an auto-reset event that wakes the worker (it
/// waits for that event and its window messages together).
struct WarmUpRequests {
    sender: crossbeam_channel::Sender<PathBuf>,
    wake: WakeEvent,
}

/// An event handle (kernel object, usable from any thread). Lives as long as
/// the process.
#[derive(Clone, Copy)]
struct WakeEvent(windows::Win32::Foundation::HANDLE);

// SAFETY: event handles may be signaled and waited on from any thread.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

thread_local! {
    /// Folder of the latest [`request_folder_warm_up`] on this (the UI)
    /// thread, so the listing batches and reloads of one folder send a
    /// single request.
    static LAST_WARM_UP_FOLDER: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Starts the background warm-up of shell context menus: one long-lived STA
/// thread that builds and drops throwaway menus, so the menus the user opens
/// on the UI thread are faster.
///
/// - Shortly after startup a folder and a file menu: the shell extension
///   DLLs stay loaded for the process. The folder menu is built twice (see
///   [`warm_up_worker`]); together this takes the first folder menu on the
///   UI thread from about 400 ms down to about 60 ms.
/// - Then a folder menu for every folder the user browses to (see
///   [`request_folder_warm_up`]): in some folders the first folder menu
///   costs about 300 ms more than later ones (measured in the user profile
///   folder; per-folder work in the handlers, not DLL loading).
///
/// A warm-up racing a menu the user opens is harmless: separate COM objects,
/// PIDLs and HMENUs in another apartment, and [`ShellContextMenu::build`]
/// touches no shared state (the settings page's entry lists are only
/// recorded when a menu is shown). At worst the user's menu is as slow as
/// without warm-up.
///
/// Call once at startup. Probe runs can opt out to measure cold menus:
/// `EDEN_PROBE_NO_WARM_UP=folders` skips the per-folder warm-ups only, any
/// other value both.
pub fn warm_up_in_background() {
    let opt_out = crate::core::probe_active()
        .then(|| std::env::var_os("EDEN_PROBE_NO_WARM_UP"))
        .flatten();
    let folder_warm_ups = match &opt_out {
        None => true,
        Some(value) if value == "folders" => false,
        Some(_) => return,
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let (sender, requests) = crossbeam_channel::unbounded();
    // Without stored requests the worker exits after the startup warm-up.
    let wake = if folder_warm_ups {
        use windows::Win32::System::Threading::CreateEventW;
        let event = match unsafe { CreateEventW(None, false, false, None) } {
            Ok(event) => WakeEvent(event),
            Err(err) => {
                eprintln!("Shell menu warm-up event failed: {err}");
                return;
            }
        };
        if WARM_UP_REQUESTS
            .set(WarmUpRequests {
                sender,
                wake: event,
            })
            .is_err()
        {
            return; // Already running.
        }
        Some(event)
    } else {
        None
    };
    let spawned = std::thread::Builder::new()
        .name("shell-menu-warm-up".to_string())
        .spawn(move || warm_up_worker(exe, requests, wake));
    if let Err(err) = spawned {
        eprintln!("Shell menu warm-up thread failed to start: {err}");
    }
}

/// Asks the warm-up worker to build a menu for a subfolder of `folder`, the
/// filesystem folder a view just listed, so the first folder menu there does
/// not pay the per-folder cost while the user waits. `subfolder` picks the
/// item (a directory directly in `folder`) and is only called for a folder
/// other than the previous request's; `None` skips the folder.
///
/// UI thread only. Never blocks: the request is queued, the worker keeps
/// only the latest pending one and skips folders it already warmed.
pub fn request_folder_warm_up(folder: &Path, subfolder: impl FnOnce() -> Option<PathBuf>) {
    let Some(queue) = WARM_UP_REQUESTS.get() else {
        return;
    };
    let repeated = LAST_WARM_UP_FOLDER.with_borrow(|last| last.as_deref() == Some(folder));
    if repeated {
        return;
    }
    let Some(item) = subfolder() else {
        return;
    };
    LAST_WARM_UP_FOLDER.set(Some(folder.to_path_buf()));
    debug_log(format_args!("warm-up requested: {}", item.display()));
    // Unbounded: never blocks. Fails only when the worker is gone.
    if queue.sender.send(item).is_ok() {
        let _ = unsafe { windows::Win32::System::Threading::SetEvent(queue.wake.0) };
    }
}

/// The warm-up thread (see [`warm_up_in_background`]). Holds no locks and
/// shares no state with the UI thread; every menu is its own set of COM
/// objects in this thread's apartment. Without `wake` (per-folder warm-ups
/// disabled) it ends after the startup warm-up.
fn warm_up_worker(
    exe: PathBuf,
    requests: crossbeam_channel::Receiver<PathBuf>,
    wake: Option<WakeEvent>,
) {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    std::thread::sleep(WARM_UP_DELAY);
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
        return;
    }
    let mut warmed = WarmedFolders::default();

    // Startup: folder and file menus load the handler DLLs of both. The
    // folder is the browsed one when it was requested in the meantime, else
    // the exe's folder. The process's first folder menu comes out short of
    // some entries (29 instead of 34 here: handlers still initializing) and
    // the second one, on whichever thread, often pays another 300-800 ms.
    // With a single build that second one was the user's first folder menu,
    // so the folder menu is built twice.
    let started = Instant::now();
    let startup_folder = requests
        .try_recv()
        .ok()
        .map(|item| latest_request(item, &requests))
        .filter(|item| is_warm_up_target(item))
        .or_else(|| exe.parent().map(Path::to_path_buf));
    if let Some(folder) = &startup_folder {
        warm_up_item(folder, &mut warmed);
    }
    build_throwaway_menu(&exe);
    if let Some(folder) = &startup_folder {
        build_throwaway_menu(folder);
    }
    debug_log(format_args!(
        "startup warm-up done in {:.2} ms",
        started.elapsed().as_secs_f64() * 1000.0
    ));

    // Per-folder warm-ups for as long as the process lives. An STA thread
    // must keep pumping messages while it waits: shell extensions may have
    // created windows on it or expect calls into the apartment.
    if let Some(wake) = wake {
        while pump_messages() {
            if let Ok(item) = requests.try_recv() {
                let item = latest_request(item, &requests);
                if is_warm_up_target(&item) {
                    warm_up_item(&item, &mut warmed);
                }
                // Requests sent during the build also set the event; look
                // at the queue (and the messages) again before waiting.
                continue;
            }
            if !wait_for_event_or_message(wake) {
                break;
            }
        }
    }
    unsafe { CoUninitialize() };
}

/// Dispatches the thread's queued window messages. `false` on `WM_QUIT`.
fn pump_messages() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
    };
    let mut msg = MSG::default();
    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        if msg.message == WM_QUIT {
            return false;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

/// Blocks until `wake` is signaled or a message arrives for this thread.
/// `false` when the wait failed (the worker then ends rather than spin).
fn wait_for_event_or_message(wake: WakeEvent) -> bool {
    use windows::Win32::Foundation::WAIT_FAILED;
    use windows::Win32::System::Threading::INFINITE;
    use windows::Win32::UI::WindowsAndMessaging::{
        MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, QS_ALLINPUT,
    };
    let result = unsafe {
        MsgWaitForMultipleObjectsEx(Some(&[wake.0]), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
    };
    result != WAIT_FAILED
}

/// `received` or, when newer requests are queued, the newest one (superseded
/// requests are dropped: only the folder the user is in now matters).
fn latest_request(received: PathBuf, requests: &crossbeam_channel::Receiver<PathBuf>) -> PathBuf {
    let mut latest = received;
    while let Ok(newer) = requests.try_recv() {
        let dropped = std::mem::replace(&mut latest, newer);
        debug_log(format_args!("warm-up superseded: {}", dropped.display()));
    }
    latest
}

/// Whether a requested item is a plain filesystem directory (not a portable
/// device object or a virtual location).
fn is_warm_up_target(item: &Path) -> bool {
    item.is_absolute()
        && !crate::core::portable::is_portable_path(&item.to_path_buf())
        && std::fs::metadata(item).is_ok_and(|meta| meta.is_dir())
}

/// Builds and drops the menu of `item` unless its parent folder was warmed
/// already.
fn warm_up_item(item: &Path, warmed: &mut WarmedFolders) {
    if let Some(parent) = item.parent()
        && !warmed.insert(parent)
    {
        debug_log(format_args!("warm-up skipped (warmed): {}", item.display()));
        return;
    }
    build_throwaway_menu(item);
}

/// Builds and drops the menu of `item`. Built like a normal right-click
/// menu, but nothing is pruned or recorded (that happens when a menu is
/// shown, see `show_native`).
fn build_throwaway_menu(item: &Path) {
    let started = Instant::now();
    let result = ShellContextMenu::build(
        &[item.to_path_buf()],
        HWND::default(),
        ShellMenuKind::Filtered,
    );
    debug_log(format_args!(
        "warm-up {} in {:.2} ms: {}",
        if result.is_ok() { "done" } else { "failed" },
        started.elapsed().as_secs_f64() * 1000.0,
        item.display()
    ));
}

/// Folders whose menus were warmed this session (case-insensitive, at most
/// [`WARMED_FOLDERS_CAP`], oldest forgotten first).
#[derive(Default)]
struct WarmedFolders(std::collections::VecDeque<String>);

impl WarmedFolders {
    /// Records `folder`; `false` when it was recorded already.
    fn insert(&mut self, folder: &Path) -> bool {
        let key = folder.to_string_lossy().to_lowercase();
        if self.0.contains(&key) {
            return false;
        }
        if self.0.len() >= WARMED_FOLDERS_CAP {
            self.0.pop_front();
        }
        self.0.push_back(key);
        true
    }
}

impl Drop for ShellContextMenu {
    fn drop(&mut self) {
        unsafe {
            if !self.menu.is_invalid() {
                // Destroys every submenu still attached, the application
                // submenu included.
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

/// Diagnostics for one read menu level.
#[derive(Default)]
struct CollectStats {
    /// Entries keyed by their canonical verb, and the time to look them up.
    verbs: u32,
    verb_time: Duration,
}

impl std::fmt::Display for CollectStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "verbs {} in {:.2} ms",
            self.verbs,
            self.verb_time.as_secs_f64() * 1000.0,
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
    use windows::Win32::UI::WindowsAndMessaging::GetSubMenu;

    #[test]
    fn warmed_folders_dedupe_case_insensitively_and_forget_the_oldest() {
        let mut warmed = WarmedFolders::default();
        assert!(warmed.insert(Path::new(r"C:\Users\Me")));
        assert!(!warmed.insert(Path::new(r"c:\users\me")));
        assert!(warmed.insert(Path::new(r"\\server\share\dir")));
        for index in 0..WARMED_FOLDERS_CAP {
            warmed.insert(&PathBuf::from(format!(r"D:\{index}")));
        }
        assert_eq!(warmed.0.len(), WARMED_FOLDERS_CAP);
        // The first folders were forgotten, the latest are remembered.
        assert!(warmed.insert(Path::new(r"C:\Users\Me")));
        assert!(!warmed.insert(&PathBuf::from(format!(r"D:\{}", WARMED_FOLDERS_CAP - 1))));
    }

    #[test]
    fn warm_up_requests_coalesce_to_the_latest() {
        let (sender, requests) = crossbeam_channel::unbounded();
        assert_eq!(
            latest_request(PathBuf::from(r"C:\a"), &requests),
            PathBuf::from(r"C:\a")
        );
        for path in [r"C:\b", r"C:\c", r"C:\d"] {
            sender.send(PathBuf::from(path)).unwrap();
        }
        assert_eq!(
            latest_request(PathBuf::from(r"C:\a"), &requests),
            PathBuf::from(r"C:\d")
        );
        assert!(requests.is_empty());
    }

    #[test]
    fn warm_up_targets_are_plain_directories() {
        let dir = std::env::temp_dir();
        assert!(is_warm_up_target(&dir));
        assert!(!is_warm_up_target(&std::env::current_exe().unwrap()));
        assert!(!is_warm_up_target(Path::new(crate::core::fs::MY_PC_PATH)));
        assert!(!is_warm_up_target(Path::new(
            crate::core::fs::MY_RECYCLE_BIN_PATH
        )));
        assert!(!is_warm_up_target(Path::new("portable://device/object")));
        assert!(!is_warm_up_target(&dir.join("eden-no-such-folder-7f3a")));
    }

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
    fn command_id_ranges_do_not_overlap() {
        // 0 is "cancelled"; the shell range and the application range are
        // disjoint and fit the 16-bit menu ids of WM_COMMAND.
        const {
            assert!(CONTEXT_MENU_ID_BASE > 0);
            assert!(CONTEXT_MENU_ID_BASE <= CONTEXT_MENU_ID_MAX);
            assert!(CONTEXT_MENU_ID_MAX < APP_MENU_ID_BASE);
            assert!(APP_MENU_ID_BASE <= APP_MENU_ID_MAX);
            assert!(APP_MENU_ID_MAX <= 0xFFFF);
        }

        assert_eq!(command_target(0), CommandTarget::Cancelled);
        assert_eq!(
            command_target(CONTEXT_MENU_ID_BASE),
            CommandTarget::Shell(CONTEXT_MENU_ID_BASE)
        );
        assert_eq!(
            command_target(CONTEXT_MENU_ID_MAX),
            CommandTarget::Shell(CONTEXT_MENU_ID_MAX)
        );
        assert_eq!(command_target(APP_MENU_ID_BASE), CommandTarget::App(0));
        assert_eq!(command_target(APP_MENU_ID_MAX + 1), CommandTarget::Unknown);

        for index in [0, 1, 10, (APP_MENU_ID_MAX - APP_MENU_ID_BASE) as usize] {
            let id = app_command_id(index).unwrap();
            assert_eq!(command_target(id), CommandTarget::App(index));
        }
        assert_eq!(
            app_command_id((APP_MENU_ID_MAX - APP_MENU_ID_BASE) as usize + 1),
            None
        );
    }

    #[test]
    fn app_submenu_is_inserted_on_top() {
        use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemID, GetMenuState, MF_GRAYED};
        let root = native_menu(&[("Open", 'c'), ("Properties", 'c')]);
        let items = [
            AppMenuItem::Command {
                label: "Copy & paste".to_string(),
                enabled: true,
            },
            AppMenuItem::Separator,
            AppMenuItem::Command {
                label: "Paste".to_string(),
                enabled: false,
            },
        ];
        let app = insert_app_submenu(
            root,
            AppSubmenu {
                label: "EdenExplorer",
                items: &items,
            },
        )
        .unwrap();

        assert_eq!(
            native_labels(root),
            vec!["EdenExplorer", "-", "Open", "Properties"]
        );
        assert_eq!(unsafe { GetSubMenu(root, 0) }, app);
        // `&` is literal text, not a mnemonic.
        assert_eq!(native_labels(app), vec!["Copy & paste", "-", "Paste"]);
        unsafe {
            assert_eq!(GetMenuItemID(app, 0), APP_MENU_ID_BASE);
            assert_eq!(GetMenuItemID(app, 2), APP_MENU_ID_BASE + 2);
            assert_eq!(GetMenuState(app, 0, MF_BYPOSITION) & MF_GRAYED.0, 0);
            assert_ne!(GetMenuState(app, 2, MF_BYPOSITION) & MF_GRAYED.0, 0);
            // The root owns the application submenu.
            DestroyMenu(root).unwrap();
        }
    }

    #[test]
    fn native_pruning_records_entries_but_not_the_app_submenu() {
        let root = native_menu(&[("Open", 'c'), ("Send to", 's'), ("Hide", 'c')]);
        let send_to = unsafe { GetSubMenu(root, 1) };
        let hidden: HashSet<String> = ["label:Hide".to_string()].into_iter().collect();
        let pruner = Rc::new(NativePruner::with_hidden(
            None,
            CONTEXT_MENU_ID_BASE,
            root,
            hidden,
        ));
        let seen_keys = || -> Vec<String> {
            seen_entries()
                .into_iter()
                .map(|entry| format!("{}{}", "  ".repeat(entry.depth), entry.key))
                .collect()
        };

        // Root before showing: everything listed, the hidden entry included.
        pruner.prune(root);
        assert_eq!(native_labels(root), vec!["Open", "Send to"]);
        assert_eq!(
            seen_keys(),
            vec!["label:Open", "label:Send to", "label:Hide"]
        );

        let items = [AppMenuItem::Command {
            label: "Rename".to_string(),
            enabled: true,
        }];
        let app = insert_app_submenu(
            root,
            AppSubmenu {
                label: "EdenExplorer",
                items: &items,
            },
        )
        .unwrap();
        pruner.app_menu.set(Some(app));
        pruner
            .settled_root_count
            .set(Some(unsafe { GetMenuItemCount(Some(root)) }));

        let _active = ActiveNativeMenuGuard::install(ActiveNativeMenu {
            handler: MenuMsgHandler {
                cm2: None,
                cm3: None,
            },
            pruner: Some(Rc::clone(&pruner)),
            app_menu: Some(app.0 as usize),
        });
        let init_popup = |menu: HMENU| {
            forward_native_menu_message(WM_INITMENUPOPUP, WPARAM(menu.0 as usize), LPARAM(0))
        };

        // The root again (as TrackPopupMenuEx does): nothing new.
        init_popup(root);
        // A shell submenu opened by the user: its children are recorded.
        init_popup(send_to);
        // The application submenu: neither pruned nor recorded.
        init_popup(app);
        assert!(!pruner.menus.borrow().contains_key(&(app.0 as usize)));
        assert_eq!(native_labels(app), vec!["Rename"]);
        assert_eq!(
            seen_keys(),
            vec![
                "label:Open",
                "label:Send to",
                "  label:Send to > child",
                "label:Hide"
            ]
        );

        // A changed root is read again; the hidden entry and the recorded
        // children stay listed and the application submenu stays out.
        pruner.settled_root_count.set(None);
        pruner.prune(root);
        assert_eq!(
            native_labels(root),
            vec!["EdenExplorer", "-", "Open", "Send to"]
        );
        assert_eq!(
            seen_keys(),
            vec![
                "label:Open",
                "label:Send to",
                "  label:Send to > child",
                "label:Hide"
            ]
        );

        unsafe { DestroyMenu(root).unwrap() };
    }
}
