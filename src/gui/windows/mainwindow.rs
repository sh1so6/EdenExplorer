use crate::core::indexer::{
    load_app_settings, load_favorites, load_layout_settings, load_sidebar_settings, load_tags,
    load_theme_settings,
};
use crate::core::launch::take_forwarded_paths;
use crate::core::utils::tabs::update_tab_infos_cache;
use crate::gui::dragdrop::{DragDropBackend, DropTargets};
use crate::gui::i18n::I18n;
use crate::gui::icons::IconCache;
use crate::gui::theme::{
    ThemeMode, apply_font_to_context, apply_theme, get_default_palette, get_palette, set_palette,
};
use crate::gui::utils::{SortColumn, sort_files_by_keys};
use crate::gui::windows::containers::enums::ItemViewerAction;
use crate::gui::windows::containers::explorer::draw_tab_content;
use crate::gui::windows::containers::itemviewer_navbar::NAVBAR_MIN_STRIP_WIDTH;
use crate::gui::windows::containers::sidebar::{SIDEBAR_MIN_WIDTH, SidebarTags, draw_sidebar};
use crate::gui::windows::containers::structs::{
    FavoriteItem, FileOpState, ItemViewerColumnState, ItemViewerFolderSizeState,
    ItemViewerNavBarAction, RenameState, SidebarAction, SplitSide, TabInfo, TabState, TabsAction,
    TagsState,
};
use crate::gui::windows::containers::tabs::{
    VERTICAL_TABS_MAX_WIDTH, VERTICAL_TABS_MIN_WIDTH, draw_tabs, draw_vertical_tabs,
    handle_window_drag_area,
};
use crate::gui::windows::containers::tags::{
    draw_delete_confirmation_popup, draw_tag_picker_popup, draw_tags, layout_tags_dock,
};
use crate::gui::windows::containers::topbar::draw_topbar;
use crate::gui::windows::mainwindow_imp::{
    apply_directory_settings_to_view, directory_settings_snapshot_for_view, draw_file_op_window,
    handle_draw_customizetheme_window, handle_pending_actions, persist_directory_settings_snapshot,
};
use crate::gui::windows::structs::{
    AboutWindow, AppSettings, Navigation, SettingsWindow, SidebarCollapsedSections, SidebarState,
    ThemeCustomizer,
};
use crate::gui::windows::windowsoverrides::{
    apply_window_override, apply_window_theme, consume_clipboard_dirty,
    handle_draw_windows_buttons, install_wndproc,
};
use eframe::egui;
use eframe::egui::emath::GuiRounding as _;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos};

pub struct MainWindow {
    // MainWindow General Variables
    pub(crate) theme: ThemeMode,
    pub(crate) theme_dirty: bool,
    pub(crate) window_override_set: bool,
    pub(crate) theme_customizer: ThemeCustomizer,
    pub(crate) settings_window: SettingsWindow,
    pub(crate) about_window: AboutWindow,
    pub(crate) dropped_files: Vec<PathBuf>,
    pub(crate) external_drag_to_internal_hover: bool,
    pub(crate) dropped_files_pending_ui_refresh: bool,
    pub(crate) shutdown: Arc<AtomicBool>,
    pub(crate) hwnd: Option<HWND>,
    pub(crate) sidebar_collapsed: bool,
    /// Tags panel docked below the explorer content (persisted in `layout.json`).
    pub(crate) tags_dock_open: bool,
    /// Preferred dock height, persisted. Drawn clamped to the window.
    pub(crate) tags_dock_height: f32,
    /// The last pointer press landed in the tags dock: explorer keyboard
    /// shortcuts are suppressed until the user presses in the explorer again.
    pub(crate) tags_dock_focused: bool,
    /// Rect of the docked tags panel in the previous frame (for focus tracking).
    pub(crate) tags_dock_rect: Option<egui::Rect>,

    // File Explorer Variables (per-tab/per-view state lives on TabState/TabView)
    pub(crate) tabs: Vec<TabState>,
    pub(crate) active_tab: usize,
    pub(crate) tab_infos_cache: Vec<TabInfo>,
    pub(crate) tab_infos_dirty: bool,
    pub(crate) tab_scroll_offset: f32,
    pub(crate) pending_tab_scroll_id: Option<u64>,
    pub(crate) focused_split: SplitSide,
    /// Vertical tabs mode: whether the address bars currently live in the
    /// title-bar strip (false: too narrow, drawn inside the panes).
    pub(crate) nav_strip_active: bool,
    pub(crate) next_tab_id: u64,
    pub(crate) folder_sizes: HashMap<PathBuf, ItemViewerFolderSizeState>,
    pub(crate) rename_state: Option<RenameState>,
    pub(crate) dragdrop: Option<Box<dyn DragDropBackend>>,
    pub(crate) file_type_cache: HashMap<String, String>,
    pub(crate) tags_state: TagsState,
    pub(crate) file_op: Option<FileOpState>,
    pub(crate) clipboard_paths: Vec<PathBuf>,
    pub(crate) clipboard_set: HashSet<PathBuf>,
    pub(crate) clipboard_is_cut: bool,
    pub(crate) clipboard_has_files: bool,
    pub(crate) file_size_text_cache: HashMap<PathBuf, (u64, String)>,
    pub(crate) folder_size_text_cache: HashMap<PathBuf, (u64, bool, String)>,
    pub(crate) drive_size_text_cache: HashMap<PathBuf, (u64, u64, String)>,
    pub(crate) column_settings_save_due: Option<Instant>,

    // Sidebar Variables
    pub(crate) sidebar_state: SidebarState,

    // Misc. Variables
    pub(crate) icon_cache: Option<IconCache>,

    // i18n
    pub(crate) i18n: I18n,
}

impl Default for MainWindow {
    fn default() -> Self {
        // Load saved settings
        let (
            folder_scanning_enabled,
            show_hidden_files_folders,
            show_item_viewer_icons,
            window_size_mode,
            start_path,
            saved_theme,
            pinned_tabs,
            time_format_24h,
            sort_column,
            sort_ascending,
            language,
            date_style,
            item_viewer_file_column_order,
            item_viewer_drive_column_order,
            recycle_bin_column_order,
            item_viewer_file_column_sizes,
            item_viewer_drive_column_sizes,
            recycle_bin_column_sizes,
            directory_settings,
        ) = load_app_settings();
        let layout_settings = load_layout_settings();
        let loaded_settings = AppSettings {
            folder_scanning_enabled,
            show_hidden_files_folders,
            show_item_viewer_icons,
            window_size_mode: window_size_mode.clone(),
            start_path: Some(start_path.clone()), // important
            pinned_tabs: pinned_tabs.clone(),
            time_format_24h,
            date_style,
            sort_column,
            sort_ascending,
            language,
            item_viewer_file_column_order,
            item_viewer_drive_column_order,
            recycle_bin_column_order,
            item_viewer_file_column_sizes,
            item_viewer_drive_column_sizes,
            recycle_bin_column_sizes,
            directory_settings,
            vertical_tabs: layout_settings.vertical_tabs,
            vertical_tabs_width: layout_settings.vertical_tabs_width,
        };

        let system_locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string());

        let default_locale = match system_locale.as_str() {
            l if l.starts_with("ja") => "ja-JP",
            l if l.starts_with("id") => "id-ID",
            l if l.starts_with("zh") => "zh-CN",
            l if l.starts_with("zh-HK") => "zh-HK",
            l if l.starts_with("zh-TW") => "zh-TW",
            _ => "en-US",
        };

        let pinned_tabs = pinned_tabs;
        let mut tabs = Vec::new();
        let mut next_tab_id = 1;

        if pinned_tabs.is_empty() {
            tabs.push(TabState::new(
                next_tab_id,
                Navigation::new(start_path),
                loaded_settings.sort_column,
                loaded_settings.sort_ascending,
            ));
            tabs.last_mut().unwrap().primary_view.column_state = ItemViewerColumnState::from_orders(
                loaded_settings.item_viewer_file_column_order.clone(),
                loaded_settings.item_viewer_drive_column_order.clone(),
                loaded_settings.recycle_bin_column_order.clone(),
                loaded_settings.item_viewer_file_column_sizes.clone(),
                loaded_settings.item_viewer_drive_column_sizes.clone(),
                loaded_settings.recycle_bin_column_sizes.clone(),
            );
            next_tab_id += 1;
        } else {
            for path in &pinned_tabs {
                tabs.push(TabState::new(
                    next_tab_id,
                    Navigation::new(path.clone()),
                    loaded_settings.sort_column,
                    loaded_settings.sort_ascending,
                ));
                tabs.last_mut().unwrap().primary_view.column_state =
                    ItemViewerColumnState::from_orders(
                        loaded_settings.item_viewer_file_column_order.clone(),
                        loaded_settings.item_viewer_drive_column_order.clone(),
                        loaded_settings.recycle_bin_column_order.clone(),
                        loaded_settings.item_viewer_file_column_sizes.clone(),
                        loaded_settings.item_viewer_drive_column_sizes.clone(),
                        loaded_settings.recycle_bin_column_sizes.clone(),
                    );
                next_tab_id += 1;
            }
        }

        let mut app = Self {
            tabs,
            active_tab: 0,
            tab_infos_cache: Vec::new(),
            tab_infos_dirty: true,

            tab_scroll_offset: 0.0,
            pending_tab_scroll_id: None,
            focused_split: SplitSide::Primary,
            nav_strip_active: true,
            next_tab_id,
            folder_sizes: HashMap::new(),

            sidebar_state: SidebarState::default(),

            rename_state: None,
            theme: match saved_theme.as_deref() {
                Some("light") => ThemeMode::Light,
                Some("dark") | _ => ThemeMode::Dark,
            },
            sidebar_collapsed: false,
            tags_dock_open: false,
            tags_dock_height: crate::gui::windows::containers::tags::TAGS_DOCK_DEFAULT_HEIGHT,
            tags_dock_focused: false,
            tags_dock_rect: None,
            theme_dirty: true,
            window_override_set: false,
            dragdrop: None,
            icon_cache: None,

            file_type_cache: HashMap::new(),
            tags_state: load_tags()
                .map(TagsState::from_snapshot)
                .unwrap_or_default(),
            file_op: None,
            theme_customizer: Default::default(),
            settings_window: Default::default(),
            about_window: Default::default(),
            dropped_files: Vec::new(), // Files dropped from external drag and drop
            external_drag_to_internal_hover: false, // Whether external drag is hovering over the item viewer
            dropped_files_pending_ui_refresh: false,
            shutdown: Arc::new(AtomicBool::new(false)),

            hwnd: None,
            clipboard_paths: Vec::new(),
            clipboard_set: HashSet::new(),
            clipboard_is_cut: false,
            clipboard_has_files: false,
            file_size_text_cache: HashMap::new(),
            folder_size_text_cache: HashMap::new(),
            drive_size_text_cache: HashMap::new(),
            column_settings_save_due: None,

            i18n: I18n::new(default_locale),
        };

        let lang = if loaded_settings.language.is_empty() {
            default_locale
        } else {
            loaded_settings.language.as_str()
        };

        app.i18n.set_locale(lang);
        app.settings_window.current_settings = loaded_settings;

        let current_settings = app.settings_window.current_settings.clone();
        for tab in &mut app.tabs {
            apply_directory_settings_to_view(&mut tab.primary_view, &current_settings);
            if let Some(split) = tab.split_view.as_mut() {
                apply_directory_settings_to_view(split, &current_settings);
            }
        }

        match load_theme_settings() {
            Some((light, dark)) => {
                set_palette(ThemeMode::Light, light);
                set_palette(ThemeMode::Dark, dark);
            }
            None => {
                let light = get_default_palette(ThemeMode::Light);
                let dark = get_default_palette(ThemeMode::Dark);

                set_palette(ThemeMode::Light, light);
                set_palette(ThemeMode::Dark, dark);
            }
        }

        app.theme_customizer.light_palette = get_palette(ThemeMode::Light);
        app.theme_customizer.dark_palette = get_palette(ThemeMode::Dark);
        app.sidebar_state.sidebar_default_width = layout_settings.sidebar_width;
        app.sidebar_collapsed = layout_settings.sidebar_collapsed;
        app.tags_dock_open = layout_settings.tags_dock_open;
        app.tags_dock_height = layout_settings.tags_dock_height;
        app.sidebar_state.collapsed_sections =
            SidebarCollapsedSections::from_keys(&load_sidebar_settings().collapsed);
        let stored = load_favorites('C');
        if stored.is_empty() {
            app.sidebar_state.favorites = app.default_favorites();
            app.persist_favorites();
        } else {
            app.sidebar_state.favorites = stored
                .into_iter()
                .map(|path| {
                    let path = PathBuf::from(path);
                    let label = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    FavoriteItem { path, label }
                })
                .collect();
        }
        app.load_path();
        app
    }
}

impl MainWindow {
    pub fn new_with_paths(paths: Vec<PathBuf>) -> Self {
        let mut app = Self::default();
        if !paths.is_empty() {
            app.open_startup_paths(&paths);
        }
        app
    }

    pub fn mark_tab_infos_dirty(&mut self) {
        self.tab_infos_dirty = true;
    }

    pub fn active_tab(&self) -> &TabState {
        &self.tabs[self.active_tab]
    }

    pub fn active_tab_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab]
    }
}

impl eframe::App for MainWindow {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.sync_manual_window_size();
        {
            let forwarded_paths = take_forwarded_paths();
            for path in &forwarded_paths {
                self.open_new_tab(path.clone());
            }
            if !forwarded_paths.is_empty() {
                self.load_path();
            }
        }
        let palette = get_palette(self.theme);

        if self.hwnd.is_none() {
            if let Some(hwnd) = crate::gui::windows::windowsoverrides::get_hwnd_from_frame(frame) {
                self.hwnd = Some(hwnd);

                unsafe {
                    if let Err(e) = install_wndproc(hwnd) {
                        eprintln!("Failed to install wndproc: {}", e);
                    }
                }

                self.dragdrop = Some(Box::new(
                    crate::gui::windows::dragdrop::WindowsDragDropBackend::new(Some(hwnd)),
                ));

                // The first item menu would otherwise load the shell
                // extensions while the user waits.
                crate::gui::windows::shell_context_menu::warm_up_in_background();
            } else {
                eprintln!("Failed to get HWND on first frame");
            }
        }

        if !self.window_override_set {
            if let Some(hwnd) = self.hwnd {
                apply_window_override(hwnd, &palette);

                // The UI probe keeps its scripted size and off-screen position
                // (both calls would also show/activate the window).
                if !crate::core::probe_active() {
                    crate::gui::windows::windowsoverrides::set_window_mode(
                        hwnd,
                        &self.settings_window.current_settings.window_size_mode,
                    );

                    crate::gui::windows::windowsoverrides::clamp_window_to_monitor_work_area(hwnd);
                }

                self.window_override_set = true;
            }
        }

        // Main layout: sidebar + tabs column
        let mut pending_action: Option<ItemViewerAction> = None;
        let mut drop_targets = DropTargets::default();
        let dragdrop = self.dragdrop.as_deref();
        let native_drag_active = dragdrop
            .map(|backend| backend.is_drag_active())
            .unwrap_or(false);
        let native_inbound_drag_active = dragdrop
            .map(|backend| backend.is_inbound_drag_active())
            .unwrap_or(false);
        let drag_hover_target = dragdrop.and_then(|backend| backend.hovered_drop_target());
        let drag_active = {
            let tab = self.active_tab();
            tab.primary_view.drag_state.active
                || tab
                    .split_view
                    .as_ref()
                    .map(|v| v.drag_state.active)
                    .unwrap_or(false)
        } || native_drag_active;
        if let Some(backend) = dragdrop {
            backend.set_scale_factor(ui.ctx().pixels_per_point());
        }

        // Tags dock keyboard focus: a press inside the dock (last frame's rect)
        // gives it focus, a press anywhere else on the main layer gives it back
        // to the explorer. Presses in popups/windows (e.g. a color picker opened
        // from the dock) keep the current owner.
        if self.tags_dock_open {
            let press = ui.ctx().input(|i| {
                if i.pointer.any_pressed() {
                    i.pointer.press_origin()
                } else {
                    None
                }
            });
            if let Some(pos) = press {
                let on_main_layer = ui
                    .ctx()
                    .layer_id_at(pos)
                    .is_none_or(|layer| layer.order == egui::Order::Background);
                if on_main_layer {
                    self.tags_dock_focused =
                        self.tags_dock_rect.is_some_and(|rect| rect.contains(pos));
                }
            }
        } else {
            self.tags_dock_focused = false;
            self.tags_dock_rect = None;
        }
        self.tags_state.keyboard_captured = self.tags_dock_open
            && (self.tags_dock_focused || self.tags_state.rename_state.is_some());
        let tags_dock_focused = self.tags_state.keyboard_captured;
        let mut tags_dock_close_requested = false;

        if self.theme_dirty {
            apply_theme(ui.ctx(), self.theme);
            apply_font_to_context(ui.ctx(), &palette);
            crate::gui::windows::dark_menu::sync_with_palette(&palette);
            if let Some(hwnd) = self.hwnd {
                apply_window_theme(hwnd, &palette);
            }
            self.theme_dirty = false;
        }

        if consume_clipboard_dirty() {
            match crate::gui::utils::read_clipboard_files() {
                crate::gui::utils::ClipboardFileRead::Files(paths) => {
                    self.clipboard_is_cut = crate::gui::utils::is_clipboard_cut();
                    self.clipboard_has_files = !paths.is_empty();
                    self.clipboard_set = paths.iter().cloned().collect();
                    self.clipboard_paths = paths;
                }
                crate::gui::utils::ClipboardFileRead::Empty => {
                    self.clipboard_paths.clear();
                    self.clipboard_set.clear();
                    self.clipboard_is_cut = false;
                    self.clipboard_has_files = false;
                }
                // Preserve the last known clipboard state when another process is temporarily
                // using the clipboard. The next clipboard update will refresh it.
                crate::gui::utils::ClipboardFileRead::Unavailable => {}
            }
        }

        // Increase scroll speed for the explorer view.
        ui.ctx().input_mut(|i| {
            i.smooth_scroll_delta *= 6.0;
        });

        if self.icon_cache.is_none() {
            self.icon_cache = Some(IconCache::new(ui.ctx().clone()));
        }

        let icon_cache = self.icon_cache.take().unwrap();

        // Main layout: sidebar + tabs column
        let mut topbar_action = None;
        let mut sidebar_action: Option<SidebarAction> = None;
        let mut tabs_action: Option<TabsAction> = None;
        let mut tabbar_action = None;
        let mut secondary_tabbar_action: Option<ItemViewerNavBarAction> = None;
        let mut secondary_pending_action: Option<ItemViewerAction> = None;
        let mut tags_changed = false;
        let has_split = self.tabs[self.active_tab].split_view.is_some();
        let vertical_tabs = self.settings_window.current_settings.vertical_tabs;
        let mut save_layout_settings = false;

        let offset = egui::vec2(8.0, 8.0);

        egui::CentralPanel::default().show(ui, |ui| {
            // CentralPanel available rect
            let rect = ui.min_rect();

            // Shift it to compensate for Windows inset
            let rect = rect.translate(-offset);

            ui.scope_builder(
                egui::UiBuilder::new().max_rect(rect),
                |ui| {
                let avail = ui.available_size();

                ui.allocate_ui_with_layout(
                    avail,
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        // --- Sidebar column ---
                        let collapsed_width = 38.0;
                        let sidebar_width_min = SIDEBAR_MIN_WIDTH;
                        let explorer_min_width = 200.0;
                        let sidebar_width_max =
                            (avail.x - explorer_min_width).max(sidebar_width_min);
                        let sidebar_width = if self.sidebar_collapsed {
                            collapsed_width
                        } else {
                            self.sidebar_state
                                .sidebar_default_width
                                .max(sidebar_width_min)
                                .min(sidebar_width_max)
                        };

                        let sidebar_frame = egui::Frame::NONE
                            .stroke(egui::Stroke::new(1.0, palette.borders_default));
                        // Outer rect of the bordered sidebar frame (None when collapsed); the
                        // vertical tab rail attaches to its right border.
                        let mut sidebar_frame_rect: Option<egui::Rect> = None;

                        let sidebar_column_rect = ui.allocate_ui_with_layout(
                            egui::vec2(sidebar_width, ui.available_height() + 15.5),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::Frame::NONE.show(ui, |ui| {
                                    ui.add_space(8.0);
                                    topbar_action = Some(draw_topbar(
                                        ui,
                                        &self.i18n,
                                        self.theme == ThemeMode::Dark,
                                        self.tags_dock_open,
                                        self.sidebar_collapsed,
                                        self.hwnd,
                                        &palette,
                                        has_split,
                                        vertical_tabs,
                                    ));
                                });
                                if !self.sidebar_collapsed {
                                    let frame_response = sidebar_frame.show(ui, |ui| {
                                        let sidebar_tags = SidebarTags {
                                            groups: &self.tags_state.groups,
                                            dock_open: self.tags_dock_open,
                                            focused: self.tags_state.focused_group,
                                            highlight: self.tags_state.highlight,
                                        };
                                        sidebar_action = Some(draw_sidebar(
                                            ui,
                                            &self.i18n,
                                            &icon_cache,
                                            &mut self.sidebar_state,
                                            &sidebar_tags,
                                            &palette,
                                            drag_active,
                                            drag_hover_target.clone(),
                                        ));
                                    });
                                    sidebar_frame_rect = Some(frame_response.response.rect);
                                }
                            },
                        )
                        .response
                        .rect;

                        // --- Sidebar separator, only when expanded ---
                        // Reserve the same layout space as before so the explorer column keeps
                        // its position; the interactive handle itself is registered after the
                        // explorer column (so it wins hit-testing) and is aligned with the
                        // sidebar frame's visible right border rather than `sidebar_width`,
                        // which is relative to the column and not to the window.
                        //
                        // The explorer column normally starts at
                        // `sidebar_width + SIDEBAR_SEPARATOR_WIDTH / 2 + item_spacing`, which is
                        // 1px right of the sidebar frame (the frame is always slightly wider
                        // than `sidebar_width`). If sidebar content ever makes the column wider
                        // than that, start after the column instead so the two never overlap.
                        // (`sidebar_width` is used as an absolute x here, as before: the column
                        // starts at x = 0; the `max` keeps this safe if that ever changes.)
                        const SIDEBAR_SEPARATOR_WIDTH: f32 = 6.0;
                        if !self.sidebar_collapsed {
                            let item_spacing_x = ui.spacing().item_spacing.x;
                            let explorer_left = (sidebar_width
                                + SIDEBAR_SEPARATOR_WIDTH / 2.0
                                + item_spacing_x)
                                .max(sidebar_column_rect.right() + 1.0);
                            ui.advance_cursor_after_rect(egui::Rect::from_min_max(
                                egui::pos2(
                                    explorer_left - item_spacing_x - SIDEBAR_SEPARATOR_WIDTH,
                                    0.0,
                                ),
                                egui::pos2(
                                    explorer_left - item_spacing_x,
                                    ui.available_height(),
                                ),
                            ));
                        }

                        // --- Explorer column ---
                        {
                            update_tab_infos_cache(
                                &self.tabs,
                                &mut self.tab_infos_cache,
                                &mut self.tab_infos_dirty,
                                &self.settings_window,
                                &self.i18n,
                            );

                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), ui.available_height()),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    let active_id = self.tabs[self.active_tab].id;
                                    // Vertical tabs mode: title-bar strip area left of the
                                    // window controls that hosts the address bar(s).
                                    let mut nav_strip_rect: Option<egui::Rect> = None;

                                    egui::Frame::NONE.show(ui, |ui| {
                                        let topbar_rect = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 32.0),
                                            egui::Sense::hover(),
                                        ).0;

                                        // -------------------------
                                        // Tabs
                                        // -------------------------

                                        let controls_width = 45.0 * 3.0;

                                        if vertical_tabs {
                                            // Tabs live in the side panel below; the strip
                                            // left of the window controls holds the views'
                                            // address bars (drawn with the content) and the
                                            // title-bar drag areas around them.
                                            let controls_left =
                                                ui.ctx().viewport_rect().right() - controls_width;
                                            nav_strip_rect = Some(egui::Rect::from_min_max(
                                                topbar_rect.min,
                                                egui::pos2(
                                                    controls_left.min(topbar_rect.right()),
                                                    topbar_rect.bottom(),
                                                ),
                                            ));
                                        } else {
                                            let tabs_rect = egui::Rect::from_min_size(
                                                topbar_rect.min + egui::vec2(0.0, 4.0),
                                                egui::vec2(topbar_rect.width(), 24.0),
                                            );

                                            let scroll_to_id = self.pending_tab_scroll_id;

                                            ui.scope_builder(
                                                egui::UiBuilder::new().max_rect(tabs_rect),
                                                |ui| {
                                                    tabs_action = Some(draw_tabs(
                                                        ui,
                                                        &self.i18n,
                                                        &self.tab_infos_cache,
                                                        self.tab_scroll_offset,
                                                        active_id,
                                                        &palette,
                                                        self.hwnd,
                                                        scroll_to_id,
                                                        drag_active,
                                                        drag_hover_target.clone(),
                                                    ));
                                                },
                                            );

                                            if scroll_to_id.is_some() {
                                                self.pending_tab_scroll_id = None;
                                            }
                                        }

                                        // -------------------------
                                        // Window controls
                                        // -------------------------

                                        let viewport_rect = ui.ctx().viewport_rect();

                                        let controls_rect = egui::Rect::from_min_size(
                                            egui::pos2(
                                                viewport_rect.right() - controls_width,
                                                topbar_rect.top(),
                                            ),
                                            egui::vec2(controls_width, 32.0),
                                        );

                                        ui.scope_builder(
                                            egui::UiBuilder::new().max_rect(controls_rect),
                                            |ui| {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Min),
                                                    |ui| {
                                                        handle_draw_windows_buttons(&self.i18n, ui, self.hwnd, &palette);
                                                    },
                                                );
                                            },
                                        );
                                    });
                                    let container = egui::Frame::NONE
                                        .stroke(egui::Stroke::NONE)
                                        .fill(egui::Color32::TRANSPARENT)
                                        .inner_margin(egui::Margin {
                                            left: 3,
                                            right: 0,
                                            top: 0,
                                            bottom: 0,
                                        });

                                    // -------------------------
                                    // Vertical tabs panel + resize separator
                                    // -------------------------
                                    let mut content_rect: Option<egui::Rect> = None;
                                    if vertical_tabs {
                                        const CONTENT_MIN_WIDTH: f32 = 200.0;
                                        let area = ui.available_rect_before_wrap();
                                        // With the sidebar expanded the rail starts right at the
                                        // sidebar's right border (sharing that single line) and its
                                        // top lines up with the sidebar's top border. Collapsed, it
                                        // starts at the column and draws its own left edge.
                                        // Edges are snapped to physical pixels like egui does for
                                        // the sidebar frame, so the shared line stays crisp at any DPI.
                                        let ppp = ui.ctx().pixels_per_point();
                                        let (panel_left, panel_top) = match sidebar_frame_rect {
                                            Some(sidebar) => (
                                                sidebar.right().round_to_pixels(ppp).min(area.left()),
                                                sidebar.top().round_to_pixels(ppp).min(area.top()),
                                            ),
                                            None => (area.left(), area.top()),
                                        };
                                        let max_width = (area.right() - panel_left - CONTENT_MIN_WIDTH)
                                            .min(VERTICAL_TABS_MAX_WIDTH)
                                            .max(VERTICAL_TABS_MIN_WIDTH);
                                        let panel_width = self
                                            .settings_window
                                            .current_settings
                                            .vertical_tabs_width
                                            .clamp(VERTICAL_TABS_MIN_WIDTH, max_width);
                                        let panel_rect = egui::Rect::from_min_max(
                                            egui::pos2(panel_left, panel_top),
                                            egui::pos2(
                                                (panel_left + panel_width).round_to_pixels(ppp),
                                                area.bottom(),
                                            ),
                                        );

                                        let scroll_to_id = self.pending_tab_scroll_id;
                                        ui.scope_builder(
                                            egui::UiBuilder::new().max_rect(panel_rect),
                                            |ui| {
                                                tabs_action = Some(draw_vertical_tabs(
                                                    ui,
                                                    &self.i18n,
                                                    &self.tab_infos_cache,
                                                    active_id,
                                                    &palette,
                                                    scroll_to_id,
                                                    drag_active,
                                                    drag_hover_target.clone(),
                                                    sidebar_frame_rect.is_none(),
                                                ));
                                            },
                                        );
                                        if scroll_to_id.is_some() {
                                            self.pending_tab_scroll_id = None;
                                        }

                                        // Centered on the rail's 1px right divider.
                                        let separator_x = panel_rect.right() - 0.5;
                                        let separator_rect = egui::Rect::from_center_size(
                                            egui::pos2(separator_x, panel_rect.center().y),
                                            egui::vec2(6.0, panel_rect.height()),
                                        );
                                        let separator_response = ui.interact(
                                            separator_rect,
                                            ui.id().with("vertical_tabs_separator"),
                                            egui::Sense::click_and_drag(),
                                        );

                                        if separator_response.hovered() || separator_response.dragged() {
                                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);

                                            let handle_height = 25.0;
                                            let handle_width = 6.0;
                                            let handle_rect = egui::Rect::from_center_size(
                                                egui::pos2(separator_x, panel_rect.center().y),
                                                egui::vec2(handle_width, handle_height),
                                            );

                                            ui.painter().rect_filled(
                                                handle_rect,
                                                handle_width / 2.0,
                                                palette.button_seperator_handle_fill,
                                            );
                                        }

                                        if separator_response.dragged() {
                                            self.settings_window.current_settings.vertical_tabs_width =
                                                (panel_width + separator_response.drag_delta().x)
                                                    .clamp(VERTICAL_TABS_MIN_WIDTH, max_width);
                                        }

                                        if separator_response.drag_stopped() {
                                            save_layout_settings = true;
                                        }

                                        // The content container adds its own 3px left margin.
                                        content_rect = Some(egui::Rect::from_min_max(
                                            egui::pos2(panel_rect.right(), area.top()),
                                            area.max,
                                        ));
                                    }

                                    let draw_content = |ui: &mut egui::Ui| {
                                        if has_split {
                                            let split_rect = ui.available_rect_before_wrap();
                                            let (split_rect, _) = ui.allocate_exact_size(
                                                split_rect.size(),
                                                egui::Sense::hover(),
                                            );
                                            let split_width = split_rect.width();
                                            let split_height = split_rect.height();
                                            const SPLIT_GAP: f32 = 8.0;
                                            let primary_width = ((split_width - SPLIT_GAP) * 0.5).floor();
                                            let secondary_width =
                                                (split_width - primary_width - SPLIT_GAP).max(0.0);
                                            let primary_rect = egui::Rect::from_min_size(
                                                split_rect.min,
                                                egui::vec2(primary_width, split_height),
                                            );

                                            let secondary_rect = egui::Rect::from_min_size(
                                                egui::pos2(
                                                    primary_rect.right() + SPLIT_GAP,
                                                    split_rect.top(),
                                                ),
                                                egui::vec2(secondary_width, split_height),
                                            );

                                            // Vertical tabs mode: each pane's address bar sits in
                                            // the title-bar strip above that pane.
                                            let tab = &self.tabs[self.active_tab];
                                            let editing = tab.primary_view.breadcrumb_path_editing
                                                || tab
                                                    .split_view
                                                    .as_ref()
                                                    .is_some_and(|v| v.breadcrumb_path_editing);
                                            let nav_rects = layout_nav_strip(
                                                ui,
                                                nav_strip_rect,
                                                [primary_rect, secondary_rect],
                                                &mut self.nav_strip_active,
                                                editing,
                                                self.hwnd,
                                            );
                                            let primary_nav_rect = nav_rects.map(|[a, _]| a);
                                            let secondary_nav_rect = nav_rects.map(|[_, b]| b);

                                            let (pointer_pos, primary_clicked) = ui.ctx().input(|i| {
                                                (
                                                    i.pointer.interact_pos(),
                                                    i.pointer.primary_clicked(),
                                                )
                                            });

                                            if primary_clicked {
                                                if let Some(pos) = pointer_pos {
                                                    let in_primary = primary_rect.contains(pos)
                                                        || primary_nav_rect
                                                            .is_some_and(|r| r.contains(pos));
                                                    let in_secondary = secondary_rect.contains(pos)
                                                        || secondary_nav_rect
                                                            .is_some_and(|r| r.contains(pos));

                                                    if in_primary {
                                                        self.focused_split = SplitSide::Primary;
                                                    } else if in_secondary {
                                                        self.focused_split = SplitSide::Secondary;
                                                    }
                                                }
                                            }

                                            let primary_focused = self.focused_split == SplitSide::Primary;
                                            let secondary_focused = !primary_focused;

                                            ui.scope_builder(
                                                egui::UiBuilder::new().max_rect(primary_rect),
                                                |ui| {
                                                ui.set_clip_rect(primary_rect);
                                                ui.push_id("pane_0", |ui| {
                                                    ui.allocate_ui_with_layout(
                                                        ui.available_size(),
                                                        egui::Layout::top_down(egui::Align::Min),
                                                        |ui| {
                                                            if primary_focused && !tags_dock_focused {
                                                                let accent_rect =
                                                                    ui.available_rect_before_wrap();
                                                                ui.painter().hline(
                                                                    accent_rect.x_range(),
                                                                    accent_rect.top(),
                                                                    egui::Stroke::new(
                                                                        2.0,
                                                                        palette.borders_active,
                                                                    ),
                                                                );
                                                            }
                                                            ui.add_space(2.0);

                                                            let tab_id = self.tabs[self.active_tab].id;
                                                            let is_favorited = self
                                                                .sidebar_state
                                                                .favorites
                                                                .iter()
                                                                .any(|fav| {
                                                                    fav.path
                                                                        == self.tabs
                                                                            [self.active_tab]
                                                                            .primary_view
                                                                            .nav
                                                                            .current
                                                                });
                                                            let (a, b) = draw_tab_content(
                                                                ui,
                                                                &self.i18n,
                                                                &icon_cache,
                                                                &palette,
                                                                self.hwnd,
                                                                &mut self.tabs[self.active_tab]
                                                                    .primary_view,
                                                                tab_id,
                                                                is_favorited,
                                                                &self.folder_sizes,
                                                                self.clipboard_has_files,
                                                                &self.clipboard_set,
                                                                self.clipboard_is_cut,
                                                                self.settings_window
                                                                    .current_settings
                                                                    .show_hidden_files_folders,
                                                                self.settings_window
                                                                    .current_settings
                                                                    .show_item_viewer_icons,
                                                                &mut self.rename_state,
                                                                &mut self.file_type_cache,
                                                                &mut self.file_size_text_cache,
                                                                &mut self.folder_size_text_cache,
                                                                &mut self.drive_size_text_cache,
                                                                &mut self.external_drag_to_internal_hover,
                                                                drag_active,
                                                                native_inbound_drag_active,
                                                                drag_hover_target.clone(),
                                                                &mut self.tags_state,
                                                                &mut self.theme_customizer,
                                                                &mut self.settings_window,
                                                                &mut drop_targets,
                                                                primary_focused,
                                                                primary_nav_rect,
                                                            );
                                                            tabbar_action = a;
                                                            pending_action = b;
                                                            let snapshot =
                                                                directory_settings_snapshot_for_view(
                                                                    &self.tabs[self.active_tab]
                                                                        .primary_view,
                                                                );
                                                            let _ =
                                                                persist_directory_settings_snapshot(
                                                                    &mut self
                                                                        .settings_window
                                                                        .current_settings
                                                                        .directory_settings,
                                                                    snapshot,
                                                                );
                                                        },
                                                    );
                                                });
                                            });

                                            ui.scope_builder(
                                                egui::UiBuilder::new().max_rect(secondary_rect),
                                                |ui| {
                                                ui.set_clip_rect(secondary_rect);
                                                ui.push_id("pane_1", |ui| {
                                                    ui.allocate_ui_with_layout(
                                                        ui.available_size(),
                                                        egui::Layout::top_down(egui::Align::Min),
                                                        |ui| {
                                                            if secondary_focused && !tags_dock_focused {
                                                                let accent_rect =
                                                                    ui.available_rect_before_wrap();
                                                                ui.painter().hline(
                                                                    accent_rect.x_range(),
                                                                    accent_rect.top(),
                                                                    egui::Stroke::new(
                                                                        2.0,
                                                                        palette.borders_active,
                                                                    ),
                                                                );
                                                            }
                                                            ui.add_space(2.0);

                                                            let tab_id = self.tabs[self.active_tab].id;
                                                            let is_favorited = self.tabs
                                                                [self.active_tab]
                                                                .split_view
                                                                .as_ref()
                                                                .map(|v| {
                                                                    self.sidebar_state
                                                                        .favorites
                                                                        .iter()
                                                                        .any(|fav| {
                                                                            fav.path
                                                                                == v.nav.current
                                                                        })
                                                                })
                                                                .unwrap_or(false);
                                                            let (a, b) = draw_tab_content(
                                                                ui,
                                                                &self.i18n,
                                                                &icon_cache,
                                                                &palette,
                                                                self.hwnd,
                                                                self.tabs[self.active_tab]
                                                                    .split_view
                                                                    .as_mut()
                                                                    .unwrap(),
                                                                tab_id,
                                                                is_favorited,
                                                                &self.folder_sizes,
                                                                self.clipboard_has_files,
                                                                &self.clipboard_set,
                                                                self.clipboard_is_cut,
                                                                self.settings_window
                                                                    .current_settings
                                                                    .show_hidden_files_folders,
                                                                self.settings_window
                                                                    .current_settings
                                                                    .show_item_viewer_icons,
                                                                &mut self.rename_state,
                                                                &mut self.file_type_cache,
                                                                &mut self.file_size_text_cache,
                                                                &mut self.folder_size_text_cache,
                                                                &mut self.drive_size_text_cache,
                                                                &mut self.external_drag_to_internal_hover,
                                                                drag_active,
                                                                native_inbound_drag_active,
                                                                drag_hover_target.clone(),
                                                                &mut self.tags_state,
                                                                &mut self.theme_customizer,
                                                                &mut self.settings_window,
                                                                &mut drop_targets,
                                                                secondary_focused,
                                                                secondary_nav_rect,
                                                            );
                                                            secondary_tabbar_action = a;
                                                            secondary_pending_action = b;
                                                            let snapshot =
                                                                directory_settings_snapshot_for_view(
                                                                    self.tabs[self.active_tab]
                                                                        .split_view
                                                                        .as_ref()
                                                                        .unwrap(),
                                                                );
                                                            let _ =
                                                                persist_directory_settings_snapshot(
                                                                    &mut self
                                                                        .settings_window
                                                                        .current_settings
                                                                        .directory_settings,
                                                                    snapshot,
                                                                );
                                                        },
                                                    );
                                                });
                                            });
                                        } else {
                                            let pane_rect = ui.available_rect_before_wrap();
                                            let nav_rect = layout_nav_strip(
                                                ui,
                                                nav_strip_rect,
                                                [pane_rect],
                                                &mut self.nav_strip_active,
                                                self.tabs[self.active_tab]
                                                    .primary_view
                                                    .breadcrumb_path_editing,
                                                self.hwnd,
                                            )
                                            .map(|[nav]| nav);
                                            let tab_id = self.tabs[self.active_tab].id;
                                            let is_favorited =
                                                self.sidebar_state.favorites.iter().any(|fav| {
                                                    fav.path
                                                        == self.tabs[self.active_tab]
                                                            .primary_view
                                                            .nav
                                                            .current
                                                });
                                            let (a, b) = draw_tab_content(
                                                ui,
                                                &self.i18n,
                                                &icon_cache,
                                                &palette,
                                                self.hwnd,
                                                &mut self.tabs[self.active_tab].primary_view,
                                                tab_id,
                                                is_favorited,
                                                &self.folder_sizes,
                                                self.clipboard_has_files,
                                                &self.clipboard_set,
                                                self.clipboard_is_cut,
                                                self.settings_window
                                                    .current_settings
                                                    .show_hidden_files_folders,
                                                self.settings_window
                                                    .current_settings
                                                    .show_item_viewer_icons,
                                                &mut self.rename_state,
                                                &mut self.file_type_cache,
                                                &mut self.file_size_text_cache,
                                                &mut self.folder_size_text_cache,
                                                &mut self.drive_size_text_cache,
                                                &mut self.external_drag_to_internal_hover,
                                                drag_active,
                                                native_inbound_drag_active,
                                                drag_hover_target.clone(),
                                                &mut self.tags_state,
                                                &mut self.theme_customizer,
                                                &mut self.settings_window,
                                                &mut drop_targets,
                                                true,
                                                nav_rect,
                                            );
                                            tabbar_action = a;
                                            pending_action = b;
                                            let snapshot =
                                                directory_settings_snapshot_for_view(
                                                    &self.tabs[self.active_tab].primary_view,
                                                );
                                            let _ = persist_directory_settings_snapshot(
                                                &mut self
                                                    .settings_window
                                                    .current_settings
                                                    .directory_settings,
                                                snapshot,
                                            );
                                        }
                                    };

                                    // -------------------------
                                    // Tags dock: horizontal split below the content
                                    // (under both panes / right of the tab rail)
                                    // -------------------------
                                    let dock_layout = if self.tags_dock_open {
                                        layout_tags_dock(
                                            content_rect
                                                .unwrap_or_else(|| ui.available_rect_before_wrap()),
                                            self.tags_dock_height,
                                            ui.ctx().pixels_per_point(),
                                        )
                                    } else {
                                        None
                                    };
                                    let content_rect =
                                        dock_layout.as_ref().map(|dock| dock.content).or(content_rect);

                                    if let Some(content_rect) = content_rect {
                                        ui.scope_builder(
                                            egui::UiBuilder::new().max_rect(content_rect),
                                            |ui| {
                                                container.show(ui, draw_content);
                                            },
                                        );
                                    } else {
                                        container.show(ui, draw_content);
                                    }

                                    self.tags_dock_rect = dock_layout.as_ref().map(|dock| dock.panel);
                                    if let Some(dock) = dock_layout {
                                        let line_y = dock.separator.center().y;
                                        ui.painter().hline(
                                            dock.separator.x_range(),
                                            line_y,
                                            egui::Stroke::new(1.0, palette.borders_default),
                                        );
                                        let separator_response = ui.interact(
                                            dock.separator,
                                            ui.id().with("tags_dock_separator"),
                                            egui::Sense::click_and_drag(),
                                        );
                                        if separator_response.hovered()
                                            || separator_response.dragged()
                                        {
                                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                                            let handle_width = 25.0;
                                            let handle_height = 6.0;
                                            let handle_rect = egui::Rect::from_center_size(
                                                egui::pos2(dock.separator.center().x, line_y),
                                                egui::vec2(handle_width, handle_height),
                                            );
                                            ui.painter().rect_filled(
                                                handle_rect,
                                                handle_height / 2.0,
                                                palette.button_seperator_handle_fill,
                                            );
                                        }
                                        if separator_response.dragged() {
                                            // Start from the height actually drawn: the
                                            // stored preference may exceed the window.
                                            self.tags_dock_height = (dock.height
                                                - separator_response.drag_delta().y)
                                                .clamp(dock.min_height, dock.max_height);
                                        }
                                        if separator_response.drag_stopped() {
                                            save_layout_settings = true;
                                        }

                                        ui.scope_builder(
                                            egui::UiBuilder::new()
                                                .id_salt("tags_dock")
                                                .max_rect(dock.panel),
                                            |ui| {
                                                ui.set_clip_rect(
                                                    dock.panel.intersect(ui.clip_rect()),
                                                );
                                                let response = draw_tags(
                                                    ui,
                                                    &self.i18n,
                                                    &icon_cache,
                                                    &palette,
                                                    &mut self.tags_state,
                                                    tags_dock_focused,
                                                );
                                                tags_changed |= response.changed;
                                                tags_dock_close_requested |=
                                                    response.close_requested;
                                            },
                                        );
                                    }
                                },
                            );
                        }

                        if let Some(frame_rect) = sidebar_frame_rect {
                            // The frame stroke is painted inside its rect: the visible line
                            // is the last pixel column of the frame.
                            let line_x = frame_rect.right() - 0.5;
                            let separator_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    line_x - SIDEBAR_SEPARATOR_WIDTH / 2.0,
                                    frame_rect.top(),
                                ),
                                egui::pos2(
                                    line_x + SIDEBAR_SEPARATOR_WIDTH / 2.0,
                                    frame_rect.bottom(),
                                ),
                            );

                            let separator_response = ui.interact(
                                separator_rect,
                                ui.id().with("sidebar_separator"),
                                egui::Sense::click_and_drag(),
                            );

                            if separator_response.hovered() || separator_response.dragged() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);

                                let handle_height = 25.0;
                                let handle_width = 6.0;

                                let handle_rect = egui::Rect::from_center_size(
                                    egui::pos2(line_x, frame_rect.center().y),
                                    egui::vec2(handle_width, handle_height),
                                );

                                ui.painter().rect_filled(
                                    handle_rect,
                                    handle_width / 2.0,
                                    palette.button_seperator_handle_fill,
                                );
                            }

                            if separator_response.dragged() {
                                // Start from the width actually drawn: the stored preference
                                // may exceed what the current window allows.
                                self.sidebar_state.sidebar_default_width = (sidebar_width
                                    + separator_response.drag_delta().x)
                                    .max(sidebar_width_min)
                                    .min(sidebar_width_max);
                            }

                            if separator_response.drag_stopped() {
                                save_layout_settings = true;
                            }
                        }


                        if draw_delete_confirmation_popup(
                            ui.ctx(),
                            &self.i18n,
                            &palette,
                            &mut self.tags_state,
                        ) {
                            tags_changed = true;
                        }
                    },
                );
            });
        });

        if let Some(backend) = self.dragdrop.as_ref() {
            backend.update_drop_targets(drop_targets.clone());
        }

        // ItemViewer emits the drop action itself when the source and target
        // are the same view. For split views, the target pane has a different
        // DragState, so handle the cross-pane release here using the shared
        // drop target collected from both panes.
        // Sources of an internal drag released this frame. The block below clears
        // the views' drag states, so the sidebar (favorite/tag drop) handler
        // further down gets them from here.
        let mut released_drag_sources: Option<Vec<PathBuf>> = None;
        // Items dropped on a sidebar tag are only tagged, never moved/copied.
        let tag_drop = sidebar_action
            .as_ref()
            .is_some_and(|action| action.add_to_tag.is_some());
        if !native_drag_active && ui.ctx().input(|input| input.pointer.primary_released()) {
            let source_items = if self.tabs[self.active_tab].primary_view.drag_state.active {
                self.tabs[self.active_tab]
                    .primary_view
                    .drag_state
                    .source_items
                    .clone()
            } else if self.tabs[self.active_tab]
                .split_view
                .as_ref()
                .is_some_and(|view| view.drag_state.active)
            {
                self.tabs[self.active_tab]
                    .split_view
                    .as_ref()
                    .map(|view| view.drag_state.source_items.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            if !source_items.is_empty() {
                // Only a folder row is a move target for an internal drag:
                // releasing it over the empty area of either pane does nothing
                // (`item_background_targets` are only for inbound OLE drops).
                let target_dir = drop_targets.item_target.target.clone();

                released_drag_sources = Some(source_items.clone());
                if let Some(target_dir) = target_dir
                    && !tag_drop
                {
                    pending_action = Some(ItemViewerAction::MoveItems {
                        sources: source_items,
                        target_dir,
                    });
                }

                // The release belongs to the internal drag even when target
                // hit-testing missed the second pane. Always clear both states
                // so a failed drop cannot leave the drag ghost stuck forever.
                self.tabs[self.active_tab].primary_view.drag_state.active = false;
                self.tabs[self.active_tab].primary_view.drag_state.start_pos = None;
                self.tabs[self.active_tab]
                    .primary_view
                    .drag_state
                    .source_items
                    .clear();
                if let Some(split) = self.tabs[self.active_tab].split_view.as_mut() {
                    split.drag_state.active = false;
                    split.drag_state.start_pos = None;
                    split.drag_state.source_items.clear();
                }
            }
        }

        if pending_action.is_none() {
            let dropped_paths: Vec<PathBuf> = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect()
            });
            if !dropped_paths.is_empty() {
                pending_action = Some(ItemViewerAction::FilesDropped(dropped_paths));
            }
        }

        let primary_drag_active = self.tabs[self.active_tab].primary_view.drag_state.active;
        if pending_action.is_none() && primary_drag_active && !native_drag_active {
            // The UI probe drives a synthetic pointer: the real cursor is never
            // "inside", so never hand the drag over to native OLE drag-out.
            let pointer_inside = if crate::core::probe_active() {
                true
            } else if let Some(hwnd) = self.hwnd {
                unsafe {
                    let mut screen_pt = POINT::default();
                    if GetCursorPos(&mut screen_pt).is_err() {
                        false
                    } else {
                        let mut client_rect = RECT::default();
                        if GetClientRect(hwnd, &mut client_rect).is_err() {
                            false
                        } else {
                            let mut top_left = POINT {
                                x: client_rect.left,
                                y: client_rect.top,
                            };
                            let mut bottom_right = POINT {
                                x: client_rect.right,
                                y: client_rect.bottom,
                            };
                            let _ = ClientToScreen(hwnd, &mut top_left);
                            let _ = ClientToScreen(hwnd, &mut bottom_right);
                            screen_pt.x >= top_left.x
                                && screen_pt.x < bottom_right.x
                                && screen_pt.y >= top_left.y
                                && screen_pt.y < bottom_right.y
                        }
                    }
                }
            } else {
                true
            };

            if !pointer_inside {
                if let Some(backend) = self.dragdrop.as_ref() {
                    if backend.begin_file_drag(
                        &self.tabs[self.active_tab]
                            .primary_view
                            .drag_state
                            .source_items,
                    ) {
                        let view = &mut self.tabs[self.active_tab].primary_view;
                        view.drag_state.active = false;
                        view.drag_state.start_pos = None;
                        view.drag_state.source_items.clear();
                        self.load_path();
                    }
                }
            }
        }

        let primary_drag_sources = if self.tabs[self.active_tab]
            .primary_view
            .drag_state
            .source_items
            .is_empty()
        {
            None
        } else {
            Some(
                self.tabs[self.active_tab]
                    .primary_view
                    .drag_state
                    .source_items
                    .clone(),
            )
        };
        let secondary_drag_sources = self.tabs[self.active_tab]
            .split_view
            .as_ref()
            .and_then(|v| {
                if v.drag_state.source_items.is_empty() {
                    None
                } else {
                    Some(v.drag_state.source_items.clone())
                }
            });
        // Tabs keep their previous behavior (no fallback to the released
        // sources); only the sidebar (favorite / tag drop) uses it.
        let tabs_drag_sources = primary_drag_sources
            .clone()
            .or_else(|| secondary_drag_sources.clone());
        let sidebar_drag_sources = primary_drag_sources
            .clone()
            .or_else(|| secondary_drag_sources.clone())
            .or_else(|| released_drag_sources.clone());
        let primary_move_target = tabbar_action
            .as_ref()
            .and_then(|a| a.move_files_to_breadcrumb_dir.as_ref())
            .is_some()
            || tabs_action
                .as_ref()
                .and_then(|a| a.move_files_to_tab_dir.as_ref())
                .is_some()
            || sidebar_action
                .as_ref()
                .and_then(|a| a.move_files_to_sidebar_dir.as_ref())
                .is_some()
            || tag_drop;
        let secondary_move_target = secondary_tabbar_action
            .as_ref()
            .and_then(|a| a.move_files_to_breadcrumb_dir.as_ref())
            .is_some();

        self.handle_directory_batch_recieve(ui.ctx());
        self.handle_file_op_progress(ui.ctx());
        self.handle_directory_size_updates(ui.ctx());
        self.handle_throttle_size_requests(ui.ctx());
        if save_layout_settings {
            self.save_layout_settings_to_disk();
        }
        self.handle_topbar_action(topbar_action);
        if tags_dock_close_requested {
            self.set_tags_dock_open(false);
        }
        tags_changed |= self.handle_sidebar_action(sidebar_action, sidebar_drag_sources.as_deref());
        self.handle_tabs_action(tabs_action, tabs_drag_sources.as_deref());

        // Route each view's breadcrumb actions with focus pinned to that view for the
        // duration of the call, since the shared handlers resolve via self.focused_split.
        let restore_focus = self.focused_split;
        if tabbar_action.is_some() {
            self.focused_split = SplitSide::Primary;
            self.handle_tabbar_action(tabbar_action, primary_drag_sources.as_deref());
        }
        if secondary_tabbar_action.is_some() {
            self.focused_split = SplitSide::Secondary;
            self.handle_tabbar_action(secondary_tabbar_action, secondary_drag_sources.as_deref());
        }
        self.focused_split = restore_focus;

        if primary_move_target {
            let view = &mut self.tabs[self.active_tab].primary_view;
            view.drag_state.active = false;
            view.drag_state.start_pos = None;
            view.drag_state.source_items.clear();
        }
        if secondary_move_target {
            if let Some(split) = self.tabs[self.active_tab].split_view.as_mut() {
                split.drag_state.active = false;
                split.drag_state.start_pos = None;
                split.drag_state.source_items.clear();
            }
        }

        if pending_action.is_none() {
            if let Some(command) = self
                .dragdrop
                .as_ref()
                .and_then(|backend| backend.poll_command())
            {
                pending_action = Some(match command {
                    crate::gui::dragdrop::NativeDropCommand::ImportFiles(paths) => {
                        ItemViewerAction::FilesDropped(paths)
                    }
                    crate::gui::dragdrop::NativeDropCommand::MoveFiles {
                        sources,
                        target_dir,
                    } => ItemViewerAction::MoveItems {
                        sources,
                        target_dir,
                    },
                });
            }
        }

        let restore_focus = self.focused_split;
        if pending_action.is_some() {
            self.focused_split = SplitSide::Primary;
            handle_pending_actions(pending_action, self);
        }
        if secondary_pending_action.is_some() {
            self.focused_split = SplitSide::Secondary;
            handle_pending_actions(secondary_pending_action, self);
        }
        self.focused_split = restore_focus;
        // Tags dock item actions (open folder / file) apply to the focused pane.
        if let Some(tags_action) = self.tags_state.pending_action.take() {
            handle_pending_actions(Some(tags_action), self);
            self.focused_split = restore_focus;
        }
        if draw_tag_picker_popup(ui.ctx(), &self.i18n, &palette, &mut self.tags_state) {
            tags_changed = true;
        }
        if let Some(state) = &self.file_op {
            if state.started {
                draw_file_op_window(ui.ctx(), &self.i18n, &palette, state);
            }
        }
        handle_draw_customizetheme_window(
            &mut self.i18n,
            ui.ctx(),
            &mut self.theme_customizer,
            &palette,
            self.theme,
            &mut self.theme_dirty,
        );
        self.handle_draw_settings_window(ui.ctx(), &palette);
        self.handle_draw_about_window(ui.ctx(), &palette);
        self.handle_global_shortcuts(ui.ctx());

        if tags_changed {
            self.persist_tags();

            // Re-sort views if they're sorted by Tags
            for side in [SplitSide::Primary, SplitSide::Secondary] {
                if side == SplitSide::Secondary && self.active_tab().split_view.is_none() {
                    continue;
                }

                let sort_keys = self.active_tab().view(side).sort_keys.clone();
                if sort_keys.iter().any(|key| key.column == SortColumn::Tags) {
                    let tag_counts = self.tag_counts_for_sort_keys(&sort_keys);
                    sort_files_by_keys(
                        &mut self.active_tab_mut().view_mut(side).files,
                        &sort_keys,
                        &tag_counts,
                    );
                }
            }
        }

        // ✅ Step 5: Apply Deferred Refresh (IMPORTANT)
        if self.dropped_files_pending_ui_refresh {
            self.load_path();
            self.dropped_files_pending_ui_refresh = false;
        }

        self.flush_pending_column_settings_save(ui.ctx());

        self.icon_cache = Some(icon_cache);
    }

    #[cfg(feature = "ui-probe")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::core::probe::raw_input_hook(ctx, raw_input);
    }
}

/// Vertical tabs mode: width of the title-bar drag area kept free directly
/// left of the window controls, next to the address bar(s).
const NAV_STRIP_DRAG_GAP: f32 = 48.0;
/// Offset of the address bar from the top of the 32px strip; lines its icons
/// up with the sidebar top bar icons in the same row. The bar's row is ~20px
/// tall and fits the remaining 24px.
const NAV_STRIP_NAVBAR_TOP: f32 = 8.0;
/// Extra width required to move the address bars back into the strip after
/// they fell back into the panes, so resizing near the limit does not flicker.
const NAV_STRIP_HYSTERESIS: f32 = 16.0;

/// Address bar rect in the vertical-tabs title-bar strip for a view drawn in
/// `pane`: aligned to the pane's x range and ending before the drag gap.
fn strip_navbar_rect(strip: egui::Rect, pane: egui::Rect) -> egui::Rect {
    let right = pane
        .right()
        .min(strip.right() - NAV_STRIP_DRAG_GAP)
        .max(pane.left());
    egui::Rect::from_min_max(
        egui::pos2(pane.left(), strip.top() + NAV_STRIP_NAVBAR_TOP),
        egui::pos2(right, strip.bottom()),
    )
}

/// Vertical tabs mode (`strip` is `Some`): places the address bars of the
/// views drawn in `panes` into the title-bar strip and registers title-bar
/// drag areas on the rest of the strip. Returns `None` (address bars stay in
/// the panes, the whole strip is a drag area) when a bar would be narrower
/// than `NAVBAR_MIN_STRIP_WIDTH`. `active` keeps that decision between frames
/// for hysteresis; it is not changed while an address bar is being edited, so
/// the edit field does not move (and lose focus) mid-edit.
fn layout_nav_strip<const N: usize>(
    ui: &mut egui::Ui,
    strip: Option<egui::Rect>,
    panes: [egui::Rect; N],
    active: &mut bool,
    editing: bool,
    hwnd: Option<HWND>,
) -> Option<[egui::Rect; N]> {
    let strip = strip?;
    let navbars = panes.map(|pane| strip_navbar_rect(strip, pane));
    if !editing {
        let min_width = navbars
            .iter()
            .map(|rect| rect.width())
            .fold(f32::INFINITY, f32::min);
        let required = if *active {
            NAVBAR_MIN_STRIP_WIDTH
        } else {
            NAVBAR_MIN_STRIP_WIDTH + NAV_STRIP_HYSTERESIS
        };
        *active = min_width >= required;
    }
    let navbars = active.then_some(navbars);
    if let Some(navbars) = &navbars {
        // Split view: a separator like the one inside the bar keeps the
        // primary breadcrumb from running into the secondary bar's buttons.
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let height = ui.spacing().interact_size.y;
        let painter = ui.painter().with_clip_rect(strip);
        for pair in navbars.windows(2) {
            let x = ((pair[0].right() + pair[1].left()) * 0.5).round() + 0.5;
            painter.vline(x, pair[1].top()..=pair[1].top() + height, stroke);
        }
    }
    draw_nav_strip_drag_areas(ui, strip, navbars.as_ref().map_or(&[], |n| n), hwnd);
    navbars
}

/// Registers title-bar drag areas on the parts of `strip` not covered by the
/// address bars in `navbars` (ordered left to right): the spans between and
/// beside them, and the band above each bar.
fn draw_nav_strip_drag_areas(
    ui: &mut egui::Ui,
    strip: egui::Rect,
    navbars: &[egui::Rect],
    hwnd: Option<HWND>,
) {
    let mut strip_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("vertical_tabs_window_drag_area")
            .max_rect(strip),
    );
    strip_ui.set_clip_rect(strip);

    let mut left = strip.left();
    let span_ends = navbars
        .iter()
        .map(|nav| nav.left())
        .chain(std::iter::once(strip.right()));
    for (index, right) in span_ends.enumerate() {
        let right = right.min(strip.right());
        if right - left > 1.0 {
            let id = strip_ui.id().with(index);
            handle_window_drag_area(
                &mut strip_ui,
                egui::Rect::from_min_max(
                    egui::pos2(left, strip.top()),
                    egui::pos2(right, strip.bottom()),
                ),
                id,
                hwnd,
            );
        }
        if let Some(nav) = navbars.get(index) {
            left = left.max(nav.right());
        }
    }

    for (index, nav) in navbars.iter().enumerate() {
        let band = egui::Rect::from_min_max(
            egui::pos2(nav.left(), strip.top()),
            egui::pos2(nav.right(), nav.top()),
        );
        if band.width() > 1.0 && band.height() > 1.0 {
            let id = strip_ui.id().with(("above_navbar", index));
            handle_window_drag_area(&mut strip_ui, band, id, hwnd);
        }
    }
}
