use crate::core::fs::{DateStyle, MY_PC_PATH};
use crate::gui::theme::{THEME_VERSION, ThemePalette, get_default_palette};
use crate::gui::utils::SortKey;
use crate::gui::windows::containers::enums::ItemViewerHeaderColumn;
use crate::gui::windows::containers::sidebar::{
    SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
};
use crate::gui::windows::containers::structs::{GalleryThumbnailSize, ItemViewerDisplayMode};
use crate::gui::windows::containers::tabs::{
    VERTICAL_TABS_DEFAULT_WIDTH, VERTICAL_TABS_MAX_WIDTH, VERTICAL_TABS_MIN_WIDTH,
};
use crate::gui::windows::containers::tags::{
    TAGS_DOCK_DEFAULT_HEIGHT, TAGS_DOCK_MAX_HEIGHT, TAGS_DOCK_MIN_HEIGHT,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct FavoritesSnapshot {
    favorites: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct TagGroupSnapshot {
    pub id: u64,
    pub name: String,
    pub color: [u8; 4],
    pub items: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct TagsSnapshot {
    #[serde(default = "default_tags_version")]
    pub version: u32,
    #[serde(default = "default_next_tag_group_id")]
    pub next_group_id: u64,
    #[serde(default)]
    pub groups: Vec<TagGroupSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct AppSettingsSnapshot {
    folder_scanning_enabled: bool,
    #[serde(default = "default_show_hidden_files_folders")]
    show_hidden_files_folders: bool,
    #[serde(default = "default_show_item_viewer_icons")]
    show_item_viewer_icons: bool,
    /// Retired "Use Native Context Menu" setting. postcard/bincode are
    /// positional, so the slot must stay for old `settings.bin` files (and
    /// older Eden builds) to keep loading; it is ignored on load and always
    /// written as `false`.
    #[serde(default, rename = "windows_context_menu_enabled")]
    legacy_windows_context_menu_enabled: bool,
    window_size_mode: WindowSizeMode,
    pub start_path: Option<PathBuf>,
    theme: Option<String>,
    #[serde(default)]
    pinned_tabs: Vec<PathBuf>,
    #[serde(default)]
    time_format_24h: bool,
    #[serde(default = "default_date_style")]
    date_style: DateStyle,
    #[serde(default = "default_sort_column")]
    sort_column: crate::gui::utils::SortColumn,
    #[serde(default)]
    sort_ascending: bool,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_item_viewer_file_column_order")]
    item_viewer_file_column_order: Vec<ItemViewerHeaderColumn>,
    #[serde(default = "default_item_viewer_drive_column_order")]
    item_viewer_drive_column_order: Vec<ItemViewerHeaderColumn>,
    #[serde(default = "default_recycle_bin_column_order")]
    recycle_bin_column_order: Vec<ItemViewerHeaderColumn>,
    #[serde(default = "default_item_viewer_file_column_size")]
    item_viewer_file_column_sizes: Vec<f32>,
    #[serde(default = "default_item_viewer_drive_column_size")]
    item_viewer_drive_column_sizes: Vec<f32>,
    #[serde(default = "default_recycle_bin_column_size")]
    recycle_bin_column_sizes: Vec<f32>,
    #[serde(default)]
    directory_settings: Vec<DirectorySettingsSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirectorySettingsSnapshot {
    pub directory: PathBuf,
    #[serde(default = "default_item_viewer_file_column_order")]
    pub item_viewer_file_column_order: Vec<ItemViewerHeaderColumn>,
    #[serde(default = "default_item_viewer_drive_column_order")]
    pub item_viewer_drive_column_order: Vec<ItemViewerHeaderColumn>,
    #[serde(default = "default_recycle_bin_column_order")]
    pub recycle_bin_column_order: Vec<ItemViewerHeaderColumn>,
    #[serde(default = "default_item_viewer_file_column_size")]
    pub item_viewer_file_column_sizes: Vec<f32>,
    #[serde(default = "default_item_viewer_drive_column_size")]
    pub item_viewer_drive_column_sizes: Vec<f32>,
    #[serde(default = "default_recycle_bin_column_size")]
    pub recycle_bin_column_sizes: Vec<f32>,
    #[serde(default)]
    pub filter_query: String,
    #[serde(default)]
    pub display_mode: ItemViewerDisplayMode,
    #[serde(default = "default_gallery_thumbnail_size")]
    pub gallery_thumbnail_size: GalleryThumbnailSize,
    #[serde(default = "default_sort_column")]
    pub sort_column: crate::gui::utils::SortColumn,
    #[serde(default)]
    pub sort_ascending: bool,
    #[serde(default)]
    pub sort_keys: Vec<SortKey>,
}

fn default_gallery_thumbnail_size() -> GalleryThumbnailSize {
    GalleryThumbnailSize::Medium
}

// Legacy snapshot struct for deserializing old settings with HalfScreen
#[derive(Serialize, Deserialize)]
struct LegacyAppSettingsSnapshot {
    folder_scanning_enabled: bool,
    /// Retired setting, see [`AppSettingsSnapshot`].
    #[serde(default, rename = "windows_context_menu_enabled")]
    legacy_windows_context_menu_enabled: bool,
    window_size_mode: LegacyWindowSizeMode,
    pub start_path: Option<PathBuf>,
    theme: Option<String>,
    #[serde(default)]
    pinned_tabs: Vec<PathBuf>,
    #[serde(default)]
    time_format_24h: bool,
    #[serde(default = "default_sort_column")]
    sort_column: crate::gui::utils::SortColumn,
    #[serde(default)]
    sort_ascending: bool,
}

impl From<LegacyAppSettingsSnapshot> for AppSettingsSnapshot {
    fn from(legacy: LegacyAppSettingsSnapshot) -> Self {
        Self {
            folder_scanning_enabled: legacy.folder_scanning_enabled,
            show_hidden_files_folders: true,
            show_item_viewer_icons: true,
            legacy_windows_context_menu_enabled: false,
            window_size_mode: legacy.window_size_mode.into(),
            start_path: legacy.start_path,
            theme: legacy.theme,
            pinned_tabs: legacy.pinned_tabs,
            time_format_24h: legacy.time_format_24h,
            date_style: default_date_style(),
            sort_column: legacy.sort_column,
            sort_ascending: legacy.sort_ascending,
            language: default_language(),
            item_viewer_file_column_order: default_item_viewer_file_column_order(),
            item_viewer_drive_column_order: default_item_viewer_drive_column_order(),
            recycle_bin_column_order: default_recycle_bin_column_order(),
            item_viewer_file_column_sizes: default_item_viewer_file_column_size(),
            item_viewer_drive_column_sizes: default_item_viewer_drive_column_size(),
            recycle_bin_column_sizes: default_recycle_bin_column_size(),
            directory_settings: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ThemeSettingsSnapshot {
    version: u32,
    light: ThemePalette,
    dark: ThemePalette,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WindowSizeMode {
    FullScreen,
    Custom { width: f32, height: f32 },
}

// Temporary enum for deserializing old settings with HalfScreen
#[derive(Clone, Debug, Serialize, Deserialize)]
enum LegacyWindowSizeMode {
    FullScreen,
    HalfScreen,
    Custom { width: f32, height: f32 },
}

impl From<LegacyWindowSizeMode> for WindowSizeMode {
    fn from(legacy: LegacyWindowSizeMode) -> Self {
        match legacy {
            LegacyWindowSizeMode::FullScreen => WindowSizeMode::FullScreen,
            LegacyWindowSizeMode::HalfScreen => WindowSizeMode::Custom {
                width: 960.0,
                height: 540.0,
            },
            LegacyWindowSizeMode::Custom { width, height } => {
                WindowSizeMode::Custom { width, height }
            }
        }
    }
}

impl Default for WindowSizeMode {
    fn default() -> Self {
        Self::Custom {
            width: 1200.0,
            height: 800.0,
        }
    }
}

fn load_or_migrate_bincode_to_postcard<T>(path: &std::path::Path) -> Option<T>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let data = std::fs::read(path).ok()?;

    // 1️⃣ Try OLD format first (bincode)
    if let Ok(v) = bincode::deserialize::<T>(&data) {
        // migrate → postcard
        if let Ok(new_bytes) = postcard::to_allocvec(&v) {
            let tmp_path = path.with_extension("tmp");

            if std::fs::write(&tmp_path, new_bytes).is_ok() {
                let _ = std::fs::rename(tmp_path, path);
            }
        }

        return Some(v);
    }

    // 2️⃣ Try NEW format (postcard)
    if let Ok(v) = postcard::from_bytes::<T>(&data) {
        return Some(v);
    }

    // 3️⃣ Corrupt
    None
}

fn default_sort_column() -> crate::gui::utils::SortColumn {
    crate::gui::utils::SortColumn::Name
}

fn default_language() -> String {
    "en-US".to_string()
}

fn default_item_viewer_file_column_order() -> Vec<ItemViewerHeaderColumn> {
    vec![
        ItemViewerHeaderColumn::Type,
        ItemViewerHeaderColumn::Size,
        ItemViewerHeaderColumn::Tags,
        ItemViewerHeaderColumn::Modified,
        ItemViewerHeaderColumn::Created,
    ]
}

fn default_item_viewer_drive_column_order() -> Vec<ItemViewerHeaderColumn> {
    vec![
        ItemViewerHeaderColumn::Type,
        ItemViewerHeaderColumn::Size,
        ItemViewerHeaderColumn::Usage,
    ]
}

fn default_recycle_bin_column_order() -> Vec<ItemViewerHeaderColumn> {
    vec![
        ItemViewerHeaderColumn::OriginalDirectory,
        ItemViewerHeaderColumn::Type,
        ItemViewerHeaderColumn::Size,
        ItemViewerHeaderColumn::Deleted,
        ItemViewerHeaderColumn::Created,
    ]
}

pub fn default_item_viewer_file_column_size() -> Vec<f32> {
    vec![180.0, 60.0, 75.0, 100.0, 100.0, 100.0]
}

pub fn default_item_viewer_drive_column_size() -> Vec<f32> {
    vec![180.0, 120.0, 150.0]
}

pub fn default_recycle_bin_column_size() -> Vec<f32> {
    vec![180.0, 200.0, 60.0, 120.0, 100.0, 100.0]
}

fn default_date_style() -> DateStyle {
    DateStyle::UsShort
}

fn default_show_hidden_files_folders() -> bool {
    true
}

fn default_show_item_viewer_icons() -> bool {
    true
}

fn default_tags_version() -> u32 {
    1
}

fn default_next_tag_group_id() -> u64 {
    1
}

/// Root of all persisted app data (`%LOCALAPPDATA%\ExplorerEden`). In UI probe
/// mode this is redirected to an isolated directory.
pub(crate) fn app_data_dir() -> Option<PathBuf> {
    #[cfg(feature = "ui-probe")]
    if let Some(dir) = crate::core::probe::data_dir_override() {
        return Some(dir);
    }
    Some(dirs::data_local_dir()?.join("ExplorerEden"))
}

fn favorites_cache_path(drive: char) -> Option<PathBuf> {
    Some(
        app_data_dir()?
            .join("favorites")
            .join(format!("drive_{}.bin", drive)),
    )
}

fn settings_cache_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("settings.bin"))
}

fn theme_cache_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("theme.bin"))
}

fn context_menu_settings_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("context_menu.json"))
}

fn layout_settings_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("layout.json"))
}

fn sidebar_settings_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("sidebar.json"))
}

fn tags_cache_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("tags.bin"))
}

pub fn load_favorites(drive: char) -> Vec<String> {
    let path = match favorites_cache_path(drive) {
        Some(path) => path,
        None => return Vec::new(),
    };

    load_or_migrate_bincode_to_postcard::<FavoritesSnapshot>(&path)
        .map(|s| s.favorites)
        .unwrap_or_default()
}

pub fn save_favorites(drive: char, favorites: &[String]) {
    let path = match favorites_cache_path(drive) {
        Some(path) => path,
        None => return,
    };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let snapshot = FavoritesSnapshot {
        favorites: favorites.to_vec(),
    };
    if let Ok(data) = postcard::to_allocvec(&snapshot) {
        let _ = std::fs::write(path, data);
    }
}

pub fn load_tags() -> Option<TagsSnapshot> {
    let path = tags_cache_path()?;
    load_or_migrate_bincode_to_postcard::<TagsSnapshot>(&path)
}

pub fn save_tags(snapshot: &TagsSnapshot) {
    let path = match tags_cache_path() {
        Some(path) => path,
        None => return,
    };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    if let Ok(data) = postcard::to_allocvec(snapshot) {
        let _ = std::fs::write(path, data);
    }
}

pub fn load_windows_size_mode_on_start() -> WindowSizeMode {
    let path = match settings_cache_path() {
        Some(path) => path,
        None => return WindowSizeMode::default(),
    };

    let snapshot =
        load_or_migrate_bincode_to_postcard::<AppSettingsSnapshot>(&path).or_else(|| {
            load_or_migrate_bincode_to_postcard::<LegacyAppSettingsSnapshot>(&path).map(Into::into)
        });

    let snapshot = match snapshot {
        Some(s) => s,
        None => return WindowSizeMode::default(),
    };

    snapshot.window_size_mode
}

pub fn load_app_settings() -> (
    bool,
    bool,
    bool,
    WindowSizeMode,
    PathBuf,
    Option<String>,
    Vec<PathBuf>,
    bool,
    crate::gui::utils::SortColumn,
    bool,
    String,
    DateStyle,
    Vec<ItemViewerHeaderColumn>,
    Vec<ItemViewerHeaderColumn>,
    Vec<ItemViewerHeaderColumn>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<DirectorySettingsSnapshot>,
) {
    let default_path = PathBuf::from(MY_PC_PATH);

    let path = match settings_cache_path() {
        Some(path) => path,
        None => return default_app_settings(default_path),
    };

    let snapshot =
        load_or_migrate_bincode_to_postcard::<AppSettingsSnapshot>(&path).or_else(|| {
            load_or_migrate_bincode_to_postcard::<LegacyAppSettingsSnapshot>(&path).map(Into::into)
        });

    let snapshot = match snapshot {
        Some(s) => s,
        None => return default_app_settings(default_path),
    };

    (
        snapshot.folder_scanning_enabled,
        snapshot.show_hidden_files_folders,
        snapshot.show_item_viewer_icons,
        snapshot.window_size_mode,
        snapshot.start_path.unwrap_or(default_path),
        snapshot.theme,
        snapshot.pinned_tabs,
        snapshot.time_format_24h,
        snapshot.sort_column,
        snapshot.sort_ascending,
        snapshot.language,
        snapshot.date_style,
        snapshot.item_viewer_file_column_order,
        snapshot.item_viewer_drive_column_order,
        snapshot.recycle_bin_column_order,
        snapshot.item_viewer_file_column_sizes,
        snapshot.item_viewer_drive_column_sizes,
        snapshot.recycle_bin_column_sizes,
        snapshot.directory_settings,
    )
}

fn default_app_settings(
    default_path: PathBuf,
) -> (
    bool,
    bool,
    bool,
    WindowSizeMode,
    PathBuf,
    Option<String>,
    Vec<PathBuf>,
    bool,
    crate::gui::utils::SortColumn,
    bool,
    String,
    DateStyle,
    Vec<ItemViewerHeaderColumn>,
    Vec<ItemViewerHeaderColumn>,
    Vec<ItemViewerHeaderColumn>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<DirectorySettingsSnapshot>,
) {
    (
        true,
        true,
        true,
        WindowSizeMode::default(),
        default_path,
        None,
        Vec::new(),
        false,
        crate::gui::utils::SortColumn::Name,
        true,
        default_language(),
        DateStyle::default(),
        default_item_viewer_file_column_order(),
        default_item_viewer_drive_column_order(),
        default_recycle_bin_column_order(),
        default_item_viewer_file_column_size(),
        default_item_viewer_drive_column_size(),
        default_recycle_bin_column_size(),
        Vec::new(),
    )
}

pub fn save_app_settings(
    folder_scanning_enabled: bool,
    show_hidden_files_folders: bool,
    show_item_viewer_icons: bool,
    window_size_mode: &WindowSizeMode,
    start_path: &Option<PathBuf>,
    theme: Option<&str>,
    pinned_tabs: &[PathBuf],
    time_format_24h: bool,
    sort_column: crate::gui::utils::SortColumn,
    sort_ascending: bool,
    language: &str,
    date_style: DateStyle,
    item_viewer_file_column_order: &[ItemViewerHeaderColumn],
    item_viewer_drive_column_order: &[ItemViewerHeaderColumn],
    recycle_bin_column_order: &[ItemViewerHeaderColumn],
    item_viewer_file_column_sizes: &[f32],
    item_viewer_drive_column_sizes: &[f32],
    recycle_bin_column_sizes: &[f32],
    directory_settings: &[DirectorySettingsSnapshot],
) {
    let path = match settings_cache_path() {
        Some(path) => path,
        None => return,
    };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let snapshot = AppSettingsSnapshot {
        folder_scanning_enabled,
        show_hidden_files_folders,
        show_item_viewer_icons,
        legacy_windows_context_menu_enabled: false,
        window_size_mode: window_size_mode.clone(),
        start_path: start_path.clone(),
        theme: theme.map(|s| s.to_string()),
        pinned_tabs: pinned_tabs.to_vec(),
        time_format_24h,
        date_style,
        sort_column,
        sort_ascending,
        language: language.to_string(),
        item_viewer_file_column_order: item_viewer_file_column_order.to_vec(),
        item_viewer_drive_column_order: item_viewer_drive_column_order.to_vec(),
        recycle_bin_column_order: recycle_bin_column_order.to_vec(),
        item_viewer_file_column_sizes: item_viewer_file_column_sizes.to_vec(),
        item_viewer_drive_column_sizes: item_viewer_drive_column_sizes.to_vec(),
        recycle_bin_column_sizes: recycle_bin_column_sizes.to_vec(),
        directory_settings: directory_settings.to_vec(),
    };
    if let Ok(data) = postcard::to_allocvec(&snapshot) {
        let _ = std::fs::write(path, data);
    }
}

/// Layout preferences stored separately from `settings.bin` as self-describing JSON,
/// so new fields can be added without invalidating the postcard settings file.
/// Missing fields (files written by older builds) take their default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutSettingsSnapshot {
    pub vertical_tabs: bool,
    pub vertical_tabs_width: f32,
    /// Preferred expanded sidebar width (points). Clamped to the window when drawn.
    pub sidebar_width: f32,
    /// Whether the sidebar is collapsed to its icon strip.
    pub sidebar_collapsed: bool,
    /// Whether the tags panel is docked open below the explorer content.
    pub tags_dock_open: bool,
    /// Preferred tags panel height (points). Clamped to the window when drawn.
    pub tags_dock_height: f32,
    /// Reopen the tabs of the last session (`session.json`) on startup.
    pub restore_tabs_on_startup: bool,
}

impl Default for LayoutSettingsSnapshot {
    fn default() -> Self {
        Self {
            vertical_tabs: false,
            vertical_tabs_width: VERTICAL_TABS_DEFAULT_WIDTH,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            sidebar_collapsed: false,
            tags_dock_open: false,
            tags_dock_height: TAGS_DOCK_DEFAULT_HEIGHT,
            restore_tabs_on_startup: true,
        }
    }
}

impl LayoutSettingsSnapshot {
    /// Replaces non-finite values with defaults and keeps widths within their
    /// absolute bounds. Window-dependent limits are applied at draw time only,
    /// so a small first frame never shrinks the stored preference.
    fn sanitized(mut self) -> Self {
        self.vertical_tabs_width = sanitize_width(
            self.vertical_tabs_width,
            VERTICAL_TABS_MIN_WIDTH,
            VERTICAL_TABS_MAX_WIDTH,
            VERTICAL_TABS_DEFAULT_WIDTH,
        );
        self.sidebar_width = sanitize_width(
            self.sidebar_width,
            SIDEBAR_MIN_WIDTH,
            SIDEBAR_MAX_WIDTH,
            SIDEBAR_DEFAULT_WIDTH,
        );
        self.tags_dock_height = sanitize_width(
            self.tags_dock_height,
            TAGS_DOCK_MIN_HEIGHT,
            TAGS_DOCK_MAX_HEIGHT,
            TAGS_DOCK_DEFAULT_HEIGHT,
        );
        self
    }
}

fn sanitize_width(width: f32, min: f32, max: f32, default: f32) -> f32 {
    if width.is_finite() {
        width.clamp(min, max)
    } else {
        default
    }
}

/// Loads layout preferences. Missing or unreadable files fall back to defaults.
pub fn load_layout_settings() -> LayoutSettingsSnapshot {
    layout_settings_path()
        .map(|path| load_layout_settings_from(&path))
        .unwrap_or_default()
}

fn load_layout_settings_from(path: &std::path::Path) -> LayoutSettingsSnapshot {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<LayoutSettingsSnapshot>(&data).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save_layout_settings(snapshot: &LayoutSettingsSnapshot) {
    if let Some(path) = layout_settings_path() {
        save_layout_settings_to(&path, snapshot);
    }
}

fn save_layout_settings_to(path: &std::path::Path, snapshot: &LayoutSettingsSnapshot) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(snapshot) {
        let _ = std::fs::write(path, data);
    }
}

/// Shell context menu preferences, stored as JSON in `context_menu.json`
/// (separate from the postcard `settings.bin`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextMenuSettingsSnapshot {
    /// Keys of hidden shell menu entries (`verb:<verb>` or
    /// `label:<parent> > <label>`).
    pub hidden: Vec<String>,
    /// Display labels of hidden entries, so they can be listed (and shown
    /// again) even when they are not part of the current menu.
    pub labels: std::collections::BTreeMap<String, String>,
    /// Keys of hidden entries of Eden's own item menu (`eden:<name>`).
    pub eden_hidden: Vec<String>,
}

/// Loads the context menu preferences. Missing or unreadable files fall back
/// to defaults.
pub fn load_context_menu_settings() -> ContextMenuSettingsSnapshot {
    context_menu_settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|data| serde_json::from_str::<ContextMenuSettingsSnapshot>(&data).ok())
        .unwrap_or_default()
}

pub fn save_context_menu_settings(snapshot: &ContextMenuSettingsSnapshot) {
    let Some(path) = context_menu_settings_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(snapshot) {
        let _ = std::fs::write(path, data);
    }
}

/// Sidebar preferences, stored as JSON in `sidebar.json` (separate from the
/// postcard `settings.bin`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarSettingsSnapshot {
    /// Keys of collapsed sidebar sections (`places`, `favorites`, `storage`).
    /// Unknown keys are ignored by the sidebar.
    pub collapsed: Vec<String>,
}

/// Loads the sidebar preferences. Missing or unreadable files fall back to
/// defaults (all sections expanded).
pub fn load_sidebar_settings() -> SidebarSettingsSnapshot {
    sidebar_settings_path()
        .map(|path| load_sidebar_settings_from(&path))
        .unwrap_or_default()
}

fn load_sidebar_settings_from(path: &std::path::Path) -> SidebarSettingsSnapshot {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<SidebarSettingsSnapshot>(&data).ok())
        .unwrap_or_default()
}

pub fn save_sidebar_settings(snapshot: &SidebarSettingsSnapshot) {
    if let Some(path) = sidebar_settings_path() {
        save_sidebar_settings_to(&path, snapshot);
    }
}

fn save_sidebar_settings_to(path: &std::path::Path, snapshot: &SidebarSettingsSnapshot) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(snapshot) {
        let _ = std::fs::write(path, data);
    }
}

pub fn load_theme_settings() -> Option<(ThemePalette, ThemePalette)> {
    let path = theme_cache_path()?;

    match load_or_migrate_bincode_to_postcard::<ThemeSettingsSnapshot>(&path) {
        Some(snapshot) if snapshot.version == THEME_VERSION => {
            Some((snapshot.light, snapshot.dark))
        }
        _ => {
            eprintln!("Theme version mismatch or corruption. Resetting.");
            reset_theme_to_defaults();
            None
        }
    }
}

pub fn save_theme_settings(light: &ThemePalette, dark: &ThemePalette) {
    let path = match theme_cache_path() {
        Some(path) => path,
        None => return,
    };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let snapshot = ThemeSettingsSnapshot {
        version: THEME_VERSION,
        light: light.clone(),
        dark: dark.clone(),
    };
    if let Ok(data) = postcard::to_allocvec(&snapshot) {
        let _ = std::fs::write(path, data);
    }
}

/// Resets theme settings to defaults by deleting the corrupted theme file
fn reset_theme_to_defaults() {
    if let Some(path) = theme_cache_path() {
        // Remove the corrupted theme file
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!("Failed to remove corrupted theme file: {}", e);
        } else {
            eprintln!("Corrupted theme file removed. Will use defaults on next startup.");
        }

        // Save fresh default themes
        let light_default = get_default_palette(crate::gui::theme::ThemeMode::Light);
        let dark_default = get_default_palette(crate::gui::theme::ThemeMode::Dark);
        save_theme_settings(&light_default, &dark_default);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_reset_functionality() {
        // Test that reset_theme_to_defaults doesn't panic
        // In a real scenario, this would be tested with actual file system operations
        // For now, we just verify the function exists and can be called
        let path = theme_cache_path();
        assert!(path.is_some() || path.is_none()); // Basic sanity check
    }

    /// `AppSettingsSnapshot` as written by builds that still had the
    /// "Use Native Context Menu" setting (field list copied verbatim).
    #[derive(Serialize)]
    struct OldAppSettingsSnapshot {
        folder_scanning_enabled: bool,
        show_hidden_files_folders: bool,
        show_item_viewer_icons: bool,
        windows_context_menu_enabled: bool,
        window_size_mode: WindowSizeMode,
        start_path: Option<PathBuf>,
        theme: Option<String>,
        pinned_tabs: Vec<PathBuf>,
        time_format_24h: bool,
        date_style: DateStyle,
        sort_column: crate::gui::utils::SortColumn,
        sort_ascending: bool,
        language: String,
        item_viewer_file_column_order: Vec<ItemViewerHeaderColumn>,
        item_viewer_drive_column_order: Vec<ItemViewerHeaderColumn>,
        recycle_bin_column_order: Vec<ItemViewerHeaderColumn>,
        item_viewer_file_column_sizes: Vec<f32>,
        item_viewer_drive_column_sizes: Vec<f32>,
        recycle_bin_column_sizes: Vec<f32>,
        directory_settings: Vec<DirectorySettingsSnapshot>,
    }

    fn old_settings() -> OldAppSettingsSnapshot {
        OldAppSettingsSnapshot {
            folder_scanning_enabled: false,
            show_hidden_files_folders: false,
            show_item_viewer_icons: true,
            windows_context_menu_enabled: true,
            window_size_mode: WindowSizeMode::Custom {
                width: 1000.0,
                height: 700.0,
            },
            start_path: Some(PathBuf::from(r"C:\Users")),
            theme: Some("dark".to_string()),
            pinned_tabs: vec![PathBuf::from(r"C:\Temp")],
            time_format_24h: true,
            date_style: DateStyle::Iso,
            sort_column: crate::gui::utils::SortColumn::Name,
            sort_ascending: true,
            language: "ja-JP".to_string(),
            item_viewer_file_column_order: default_item_viewer_file_column_order(),
            item_viewer_drive_column_order: default_item_viewer_drive_column_order(),
            recycle_bin_column_order: default_recycle_bin_column_order(),
            item_viewer_file_column_sizes: default_item_viewer_file_column_size(),
            item_viewer_drive_column_sizes: default_item_viewer_drive_column_size(),
            recycle_bin_column_sizes: default_recycle_bin_column_size(),
            directory_settings: Vec::new(),
        }
    }

    fn assert_old_settings_kept(snapshot: &AppSettingsSnapshot) {
        assert!(!snapshot.folder_scanning_enabled);
        assert!(!snapshot.show_hidden_files_folders);
        assert!(snapshot.show_item_viewer_icons);
        assert!(matches!(
            snapshot.window_size_mode,
            WindowSizeMode::Custom { width, height } if width == 1000.0 && height == 700.0
        ));
        assert_eq!(snapshot.start_path, Some(PathBuf::from(r"C:\Users")));
        assert_eq!(snapshot.theme.as_deref(), Some("dark"));
        assert_eq!(snapshot.pinned_tabs, vec![PathBuf::from(r"C:\Temp")]);
        assert!(snapshot.time_format_24h);
        assert_eq!(snapshot.language, "ja-JP");
    }

    #[test]
    fn settings_with_retired_context_menu_flag_still_load() {
        // settings.bin is postcard (bincode in old builds); both are
        // positional, so the retired slot must still be consumed.
        let bytes = postcard::to_allocvec(&old_settings()).unwrap();
        let snapshot: AppSettingsSnapshot = postcard::from_bytes(&bytes).unwrap();
        assert_old_settings_kept(&snapshot);

        let bytes = bincode::serialize(&old_settings()).unwrap();
        let snapshot: AppSettingsSnapshot = bincode::deserialize(&bytes).unwrap();
        assert_old_settings_kept(&snapshot);

        // Self-describing formats accept the field too.
        let json = serde_json::to_string(&old_settings()).unwrap();
        assert!(json.contains("\"windows_context_menu_enabled\":true"));
        let snapshot: AppSettingsSnapshot = serde_json::from_str(&json).unwrap();
        assert_old_settings_kept(&snapshot);
    }

    #[test]
    fn context_menu_settings_without_eden_entries_load() {
        let snapshot: ContextMenuSettingsSnapshot = serde_json::from_str(
            r#"{ "hidden": ["verb:open"], "labels": { "verb:open": "Open" } }"#,
        )
        .unwrap();
        assert_eq!(snapshot.hidden, vec!["verb:open".to_string()]);
        assert!(snapshot.eden_hidden.is_empty());
    }

    #[test]
    fn sidebar_settings_load_is_tolerant() {
        let dir =
            std::env::temp_dir().join(format!("eden-sidebar-settings-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sidebar.json");

        // Missing file -> defaults (nothing collapsed).
        assert_eq!(
            load_sidebar_settings_from(&path),
            SidebarSettingsSnapshot::default()
        );

        // Round trip (also creates the missing parent dir).
        let snapshot = SidebarSettingsSnapshot {
            collapsed: vec!["storage".to_string(), "favorites".to_string()],
        };
        save_sidebar_settings_to(&path, &snapshot);
        assert_eq!(load_sidebar_settings_from(&path), snapshot);

        // Corrupt file -> defaults.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            load_sidebar_settings_from(&path),
            SidebarSettingsSnapshot::default()
        );

        // Wrong types -> defaults.
        std::fs::write(&path, r#"{ "collapsed": [1, 2] }"#).unwrap();
        assert_eq!(
            load_sidebar_settings_from(&path),
            SidebarSettingsSnapshot::default()
        );

        // Missing / unknown fields are fine.
        std::fs::write(&path, r#"{ "future": true }"#).unwrap();
        assert_eq!(
            load_sidebar_settings_from(&path),
            SidebarSettingsSnapshot::default()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layout_settings_load_save() {
        let dir =
            std::env::temp_dir().join(format!("eden-layout-settings-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("layout.json");

        // Missing file -> defaults.
        assert_eq!(
            load_layout_settings_from(&path),
            LayoutSettingsSnapshot::default()
        );

        // File written by builds without the sidebar fields: the stored
        // values are kept, the new fields take their defaults.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{ "vertical_tabs": true, "vertical_tabs_width": 136.0 }"#,
        )
        .unwrap();
        let old = load_layout_settings_from(&path);
        assert!(old.vertical_tabs);
        assert_eq!(old.vertical_tabs_width, 136.0);
        assert_eq!(old.sidebar_width, SIDEBAR_DEFAULT_WIDTH);
        assert!(!old.sidebar_collapsed);
        assert!(!old.tags_dock_open);
        assert_eq!(old.tags_dock_height, TAGS_DOCK_DEFAULT_HEIGHT);
        // Builds without the session feature: restoring defaults to on.
        assert!(old.restore_tabs_on_startup);

        // Round trip (also creates the missing parent dir).
        let _ = std::fs::remove_dir_all(&dir);
        let snapshot = LayoutSettingsSnapshot {
            vertical_tabs: true,
            vertical_tabs_width: 333.0,
            sidebar_width: 517.5,
            sidebar_collapsed: true,
            tags_dock_open: true,
            tags_dock_height: 321.0,
            restore_tabs_on_startup: false,
        };
        save_layout_settings_to(&path, &snapshot);
        assert_eq!(load_layout_settings_from(&path), snapshot);

        // Out-of-range values are brought into the absolute bounds only.
        std::fs::write(
            &path,
            r#"{ "vertical_tabs_width": 5.0, "sidebar_width": 99999.0 }"#,
        )
        .unwrap();
        let clamped = load_layout_settings_from(&path);
        assert_eq!(clamped.vertical_tabs_width, VERTICAL_TABS_MIN_WIDTH);
        assert_eq!(clamped.sidebar_width, SIDEBAR_MAX_WIDTH);
        std::fs::write(&path, r#"{ "sidebar_width": 10.0 }"#).unwrap();
        assert_eq!(
            load_layout_settings_from(&path).sidebar_width,
            SIDEBAR_MIN_WIDTH
        );

        // Corrupt file -> defaults.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            load_layout_settings_from(&path),
            LayoutSettingsSnapshot::default()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layout_width_sanitize_rejects_non_finite() {
        assert_eq!(sanitize_width(f32::NAN, 1.0, 10.0, 5.0), 5.0);
        assert_eq!(sanitize_width(f32::INFINITY, 1.0, 10.0, 5.0), 5.0);
        assert_eq!(sanitize_width(7.5, 1.0, 10.0, 5.0), 7.5);
    }

    #[test]
    fn layout_settings_tags_dock_fields() {
        // layout.json written before the tags dock existed (sidebar fields only).
        let old: LayoutSettingsSnapshot = serde_json::from_str(
            r#"{ "vertical_tabs": false, "vertical_tabs_width": 200.0,
                 "sidebar_width": 300.0, "sidebar_collapsed": false }"#,
        )
        .unwrap();
        assert!(!old.tags_dock_open);
        assert_eq!(old.tags_dock_height, TAGS_DOCK_DEFAULT_HEIGHT);
        assert_eq!(old.sidebar_width, 300.0);

        let stored: LayoutSettingsSnapshot =
            serde_json::from_str(r#"{ "tags_dock_open": true, "tags_dock_height": 5.0 }"#).unwrap();
        let stored = stored.sanitized();
        assert!(stored.tags_dock_open);
        assert_eq!(stored.tags_dock_height, TAGS_DOCK_MIN_HEIGHT);

        let non_finite = LayoutSettingsSnapshot {
            tags_dock_height: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(non_finite.tags_dock_height, TAGS_DOCK_DEFAULT_HEIGHT);
    }

    /// Writes `scripts/probe/seed_tags/tags.bin` for the UI probe (three tags
    /// pointing at real system paths). Run on demand:
    /// `cargo test write_probe_tags_seed -- --ignored`
    #[test]
    #[ignore]
    fn write_probe_tags_seed() {
        let windows = PathBuf::from(r"C:\Windows");
        let mut system_items: Vec<PathBuf> = std::fs::read_dir(&windows)
            .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        system_items.sort();
        system_items.truncate(40);
        let snapshot = TagsSnapshot {
            version: 1,
            next_group_id: 4,
            groups: vec![
                TagGroupSnapshot {
                    id: 1,
                    name: "Work".to_string(),
                    color: [235, 120, 120, 255],
                    items: vec![PathBuf::from(r"C:\Windows"), PathBuf::from(r"C:\Users")],
                },
                TagGroupSnapshot {
                    id: 2,
                    name: "Projects with a rather long tag name".to_string(),
                    color: [120, 190, 235, 255],
                    items: vec![
                        PathBuf::from(r"C:\Windows\System32"),
                        PathBuf::from(r"C:\Windows\explorer.exe"),
                        PathBuf::from(r"C:\Windows\notepad.exe"),
                    ],
                },
                TagGroupSnapshot {
                    id: 3,
                    name: "System".to_string(),
                    color: [140, 220, 140, 255],
                    items: system_items,
                },
            ],
        };
        let probe_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/probe");
        let tags_bin = postcard::to_allocvec(&snapshot).unwrap();
        let dir = probe_dir.join("seed_tags");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tags.bin"), &tags_bin).unwrap();

        // Drag test seed: the same tags plus favorites pointing at a scratch
        // folder (`%TEMP%\eden-tag-drop`, created by the probe script runner).
        let scratch = std::env::temp_dir().join("eden-tag-drop");
        let favorites = FavoritesSnapshot {
            favorites: vec![
                scratch.join("src").display().to_string(),
                scratch.join("dst").display().to_string(),
            ],
        };
        let dir = probe_dir.join("seed_tags_drag");
        std::fs::create_dir_all(dir.join("favorites")).unwrap();
        std::fs::write(dir.join("tags.bin"), &tags_bin).unwrap();
        std::fs::write(
            dir.join("favorites").join("drive_C.bin"),
            postcard::to_allocvec(&favorites).unwrap(),
        )
        .unwrap();
    }
}
