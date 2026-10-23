use crate::core::drives::DriveInfo;
use crate::core::indexer::{DirectorySettingsSnapshot, WindowSizeMode};
use crate::gui::theme::{ThemeMode, ThemePalette};
use crate::gui::utils::SortColumn;
use crate::gui::windows::containers::enums::ItemViewerHeaderColumn;
use crate::gui::windows::containers::structs::FavoriteItem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct AboutWindow {
    pub open: bool,
}

pub struct ThemeCustomizer {
    pub open: bool,
    pub selected_mode: ThemeMode,
    pub light_palette: ThemePalette,
    pub dark_palette: ThemePalette,
}

impl Default for ThemeCustomizer {
    fn default() -> Self {
        Self {
            open: false,
            selected_mode: ThemeMode::Dark,
            light_palette: crate::gui::theme::get_palette(ThemeMode::Light),
            dark_palette: crate::gui::theme::get_palette(ThemeMode::Dark),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Navigation {
    pub current: PathBuf,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub folder_scanning_enabled: bool,
    pub show_hidden_files_folders: bool,
    pub show_item_viewer_icons: bool,
    pub start_path: Option<PathBuf>,
    pub window_size_mode: WindowSizeMode,
    pub pinned_tabs: Vec<PathBuf>,
    pub time_format_24h: bool,
    pub date_style: crate::core::fs::DateStyle,
    pub sort_column: SortColumn,
    pub sort_ascending: bool,
    pub language: String,
    pub item_viewer_file_column_order: Vec<ItemViewerHeaderColumn>,
    pub item_viewer_drive_column_order: Vec<ItemViewerHeaderColumn>,
    pub recycle_bin_column_order: Vec<ItemViewerHeaderColumn>,
    pub item_viewer_file_column_sizes: Vec<f32>,
    pub item_viewer_drive_column_sizes: Vec<f32>,
    pub recycle_bin_column_sizes: Vec<f32>,
    pub directory_settings: Vec<DirectorySettingsSnapshot>,
    /// Persisted separately in `layout.json` (not part of the postcard settings snapshot).
    pub vertical_tabs: bool,
    pub vertical_tabs_width: f32,
}

#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    pub current_settings: AppSettings,
    pub show_reset_favorites_confirmation: bool,
    pub show_reset_confirmation: bool,
    pub show_reset_hotkeys_confirmation: bool,
    pub show_clear_directory_cache_confirmation: bool,
    pub selected_tab: SettingsTab,
    pub hotkey_values: Vec<String>,
    pub recording_hotkey: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    ViewDefaults,
    Hotkeys,
    ContextMenu,
}

pub struct SidebarState {
    pub favorites: Vec<FavoriteItem>,
    pub item_clicked: Option<PathBuf>,
    pub dragging_favorite: Option<usize>,
    pub sidebar_default_width: f32,
    pub cached_drives: Vec<DriveInfo>,
    pub last_drive_refresh: Instant,
    pub non_ntfs_popup_path: Option<PathBuf>,
    /// Collapsed sidebar sections, persisted in `sidebar.json`.
    pub collapsed_sections: SidebarCollapsedSections,
    /// Collapsed section header currently hovered by a file drag, and since
    /// when (spring-loaded expansion).
    pub spring_hover: Option<(SidebarSection, Instant)>,
}

/// A collapsible sidebar section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarSection {
    Places,
    Favorites,
    Storage,
}

impl SidebarSection {
    pub const ALL: [SidebarSection; 3] = [
        SidebarSection::Places,
        SidebarSection::Favorites,
        SidebarSection::Storage,
    ];

    /// Stable key used in `sidebar.json` (also the header's i18n key).
    pub fn key(self) -> &'static str {
        match self {
            SidebarSection::Places => "places",
            SidebarSection::Favorites => "favorites",
            SidebarSection::Storage => "storage",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.key() == key)
    }
}

/// Collapsed state of the sidebar sections. Default: all expanded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SidebarCollapsedSections {
    places: bool,
    favorites: bool,
    storage: bool,
}

impl SidebarCollapsedSections {
    /// Builds the state from persisted keys; unknown keys are ignored.
    pub fn from_keys<S: AsRef<str>>(keys: &[S]) -> Self {
        let mut collapsed = Self::default();
        for section in keys
            .iter()
            .filter_map(|k| SidebarSection::from_key(k.as_ref()))
        {
            collapsed.set(section, true);
        }
        collapsed
    }

    /// Keys of the collapsed sections, in display order.
    pub fn to_keys(self) -> Vec<String> {
        SidebarSection::ALL
            .into_iter()
            .filter(|section| self.is_collapsed(*section))
            .map(|section| section.key().to_string())
            .collect()
    }

    pub fn is_collapsed(self, section: SidebarSection) -> bool {
        match section {
            SidebarSection::Places => self.places,
            SidebarSection::Favorites => self.favorites,
            SidebarSection::Storage => self.storage,
        }
    }

    pub fn set(&mut self, section: SidebarSection, collapsed: bool) {
        match section {
            SidebarSection::Places => self.places = collapsed,
            SidebarSection::Favorites => self.favorites = collapsed,
            SidebarSection::Storage => self.storage = collapsed,
        }
    }
}

impl Default for SidebarState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            favorites: vec![],
            dragging_favorite: None,
            item_clicked: None,
            sidebar_default_width: 250.0,
            cached_drives: Vec::new(),
            last_drive_refresh: now.checked_sub(Duration::from_secs(60)).unwrap_or(now),
            non_ntfs_popup_path: None,
            collapsed_sections: SidebarCollapsedSections::default(),
            spring_hover: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_collapsed_sections_from_keys() {
        let empty: [&str; 0] = [];
        assert_eq!(
            SidebarCollapsedSections::from_keys(&empty),
            SidebarCollapsedSections::default()
        );

        let collapsed =
            SidebarCollapsedSections::from_keys(&["storage", "bogus", "", "STORAGE", "places"]);
        assert!(collapsed.is_collapsed(SidebarSection::Places));
        assert!(!collapsed.is_collapsed(SidebarSection::Favorites));
        assert!(collapsed.is_collapsed(SidebarSection::Storage));
        assert_eq!(collapsed.to_keys(), vec!["places", "storage"]);

        let mut collapsed = collapsed;
        collapsed.set(SidebarSection::Places, false);
        assert_eq!(collapsed.to_keys(), vec!["storage"]);
        assert_eq!(
            SidebarCollapsedSections::from_keys(&collapsed.to_keys()),
            collapsed
        );
    }
}
