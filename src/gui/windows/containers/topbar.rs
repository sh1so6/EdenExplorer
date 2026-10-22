use crate::gui::i18n::I18n;
use crate::gui::utils::{clickable_active_icon, clickable_icon};
use crate::gui::windows::containers::structs::TopbarAction;
use crate::gui::windows::windowsoverrides::toggle_window_fullscreen;
use eframe::egui;
use egui::Pos2;
use egui_phosphor::regular;
use windows::Win32::Foundation::HWND;

/// Leading space before the hamburger icon in the expanded topbar row.
const TOPBAR_LEADING_SPACE: f32 = 12.0;
/// Width of the vertical separator between the view icons and the mode icons
/// (egui's default separator spacing, set explicitly so the row width can be
/// computed up front).
const TOPBAR_SEPARATOR_WIDTH: f32 = 6.0;
/// Empty space kept at the end of the expanded row so the window can still be
/// dragged from the topbar. Mode icons that would eat into it move into the
/// hamburger menu instead.
const TOPBAR_MIN_DRAG_WIDTH: f32 = 8.0;

/// One of the toggle icons next to the hamburger menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModeIconKind {
    Sidebar,
    Theme,
    FileExplorer,
    Tags,
    Split,
    VerticalTabs,
}

#[derive(Clone, Copy)]
struct ModeIcon {
    kind: ModeIconKind,
    glyph: &'static str,
    tooltip_key: &'static str,
    /// `Some(active)` for state icons drawn with `clickable_active_icon`,
    /// `None` for plain action icons drawn with `clickable_icon`.
    active: Option<bool>,
}

impl ModeIcon {
    fn apply(self, action: &mut TopbarAction) {
        match self.kind {
            ModeIconKind::Sidebar => action.toggle_sidebar = true,
            ModeIconKind::Theme => action.toggle_theme = true,
            ModeIconKind::FileExplorer | ModeIconKind::Tags => action.toggle_file_explorer = true,
            ModeIconKind::Split => action.toggle_active_tab_split = true,
            ModeIconKind::VerticalTabs => action.toggle_vertical_tabs = true,
        }
    }

    /// The separator is drawn right before the file explorer icon.
    fn separator_before(self) -> bool {
        self.kind == ModeIconKind::FileExplorer
    }
}

/// Mode icons in display order (left to right / top to bottom). When the
/// expanded row is too narrow, icons move into the hamburger menu starting
/// from the end of this list.
fn mode_icons(
    is_dark: bool,
    is_file_explorer: bool,
    sidebar_collapsed: bool,
    has_split: bool,
    vertical_tabs: bool,
) -> Vec<ModeIcon> {
    let mut icons = vec![
        ModeIcon {
            kind: ModeIconKind::Sidebar,
            glyph: regular::SIDEBAR_SIMPLE,
            tooltip_key: if sidebar_collapsed {
                "tooltip_show_sidebar"
            } else {
                "tooltip_hide_sidebar"
            },
            active: Some(!sidebar_collapsed),
        },
        ModeIcon {
            kind: ModeIconKind::Theme,
            glyph: if is_dark { regular::SUN } else { regular::MOON },
            tooltip_key: "tooltip_theme",
            active: None,
        },
        ModeIcon {
            kind: ModeIconKind::FileExplorer,
            glyph: regular::TABS,
            tooltip_key: "tooltip_show_file_explorer",
            active: Some(is_file_explorer),
        },
        ModeIcon {
            kind: ModeIconKind::Tags,
            glyph: regular::TAG,
            tooltip_key: "tooltip_show_tags",
            active: Some(!is_file_explorer),
        },
    ];

    if is_file_explorer {
        let (glyph, tooltip_key) = if has_split {
            (regular::ARROWS_IN_LINE_HORIZONTAL, "tooltip_close_split")
        } else {
            (regular::SPLIT_HORIZONTAL, "tooltip_open_split")
        };
        icons.push(ModeIcon {
            kind: ModeIconKind::Split,
            glyph,
            tooltip_key,
            active: None,
        });
        icons.push(ModeIcon {
            kind: ModeIconKind::VerticalTabs,
            glyph: regular::ROWS,
            tooltip_key: if vertical_tabs {
                "tooltip_vertical_tabs_off"
            } else {
                "tooltip_vertical_tabs_on"
            },
            active: Some(vertical_tabs),
        });
    }

    icons
}

/// Width of an icon button as allocated by `clickable_icon` / `clickable_active_icon`.
fn icon_width(ui: &egui::Ui, glyph: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            glyph.to_owned(),
            egui::FontId::default(),
            ui.visuals().text_color(),
        )
        .size()
        .x
}

/// How many of `icons` (from the start) fit in an expanded topbar row of
/// `row_width`, leaving at least `TOPBAR_MIN_DRAG_WIDTH` for the drag region.
///
/// Computed from the glyph font metrics and the layout spacing (not from rects
/// measured in a previous frame), so the result only depends on the current
/// row width and cannot oscillate between frames. Mirrors the layout of the
/// expanded row in `draw_topbar`: leading space, hamburger, then each icon
/// followed by the item spacing (the separator adds its width + spacing).
fn visible_mode_icon_count(ui: &egui::Ui, row_width: f32, icons: &[ModeIcon]) -> usize {
    let spacing = ui.spacing().item_spacing.x;
    let mut used = TOPBAR_LEADING_SPACE + icon_width(ui, regular::LIST) + spacing;
    let budget = row_width - TOPBAR_MIN_DRAG_WIDTH;

    let mut count = 0;
    for icon in icons {
        let mut width = icon_width(ui, icon.glyph) + spacing;
        if icon.separator_before() {
            width += TOPBAR_SEPARATOR_WIDTH + spacing;
        }
        if used + width > budget {
            break;
        }
        used += width;
        count += 1;
    }
    count
}

pub fn draw_topbar(
    ui: &mut egui::Ui,
    i18n: &I18n,
    is_dark: bool,
    is_file_explorer: bool,
    sidebar_collapsed: bool,
    hwnd: Option<HWND>,
    palette: &crate::gui::theme::ThemePalette,
    has_split: bool,
    vertical_tabs: bool,
) -> TopbarAction {
    let mut action = TopbarAction::default();
    let icons = mode_icons(
        is_dark,
        is_file_explorer,
        sidebar_collapsed,
        has_split,
        vertical_tabs,
    );

    if sidebar_collapsed {
        // Collapsed: icons stack in a narrow vertical rail, centered.
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            draw_hamburger_menu(ui, i18n, palette, &mut action, &[]);
            draw_mode_icons(ui, i18n, palette, &mut action, &icons);
            draw_drag_region(ui, hwnd);
        });
    } else {
        // Expanded: horizontal topbar row. Icons that don't fit the sidebar
        // width are listed in the hamburger menu instead, so the row never
        // makes the sidebar column wider than the sidebar.
        ui.horizontal(|ui| {
            let visible = visible_mode_icon_count(ui, ui.available_width(), &icons);
            let (shown, overflow) = icons.split_at(visible);

            ui.add_space(TOPBAR_LEADING_SPACE);
            draw_hamburger_menu(ui, i18n, palette, &mut action, overflow);

            if !shown.is_empty() {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    draw_mode_icons(ui, i18n, palette, &mut action, shown);
                });
            }

            draw_drag_region(ui, hwnd);
        });
    }

    action
}

fn draw_hamburger_menu(
    ui: &mut egui::Ui,
    i18n: &I18n,
    palette: &crate::gui::theme::ThemePalette,
    action: &mut TopbarAction,
    overflow: &[ModeIcon],
) {
    let menu_id = egui::Id::new("topbar_hamburger_menu");

    let menu_open = ui.memory(|mem| mem.data.get_temp::<bool>(menu_id).unwrap_or(false));

    if clickable_icon(ui, regular::LIST, palette)
        .on_hover_text(
            egui::RichText::new(&i18n.tr("tooltip_mainmenu"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        ui.memory_mut(|mem| mem.data.insert_temp(menu_id, !menu_open));
    }

    if menu_open {
        let icon_rect = ui.min_rect();
        let popup_pos = Pos2::new(icon_rect.min.x, icon_rect.max.y);

        let area_response = egui::Area::new(egui::Id::new("topbar_hamburger_menu_area"))
            .fixed_pos(popup_pos)
            .show(ui.ctx(), |ui| {
                egui::containers::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(120.0);

                    // Mode icons that didn't fit in the topbar row, in row order.
                    // Toggles in their "on" state get the same accent icon color
                    // as in the row.
                    for icon in overflow {
                        if menu_item(
                            ui,
                            icon.glyph,
                            &i18n.tr(icon.tooltip_key),
                            icon.active.unwrap_or(false),
                            palette,
                        )
                        .clicked()
                        {
                            icon.apply(action);
                            ui.memory_mut(|mem| mem.data.insert_temp(menu_id, false));
                        }
                    }
                    if !overflow.is_empty() {
                        ui.separator();
                    }

                    if menu_item(ui, regular::PALETTE, &i18n.tr("theme"), false, palette).clicked()
                    {
                        action.customize_theme = true;
                        ui.memory_mut(|mem| mem.data.insert_temp(menu_id, false));
                    }

                    if menu_item(ui, regular::SLIDERS, &i18n.tr("settings"), false, palette)
                        .clicked()
                    {
                        action.open_settings = true;
                        ui.memory_mut(|mem| mem.data.insert_temp(menu_id, false));
                    }

                    if menu_item(ui, regular::QUESTION, &i18n.tr("about"), false, palette).clicked()
                    {
                        action.about = true;
                        ui.memory_mut(|mem| mem.data.insert_temp(menu_id, false));
                    }

                    if menu_item(ui, regular::X, &i18n.tr("exit"), false, palette).clicked() {
                        action.exit = true;
                        ui.memory_mut(|mem| mem.data.insert_temp(menu_id, false));
                    }
                });
            })
            .response;

        if area_response.clicked_elsewhere() {
            ui.memory_mut(|mem| mem.data.insert_temp(menu_id, false));
        }
    }
}

fn draw_mode_icons(
    ui: &mut egui::Ui,
    i18n: &I18n,
    palette: &crate::gui::theme::ThemePalette,
    action: &mut TopbarAction,
    icons: &[ModeIcon],
) {
    for icon in icons {
        if icon.separator_before() {
            ui.add(egui::Separator::default().spacing(TOPBAR_SEPARATOR_WIDTH));
        }

        let response = match icon.active {
            Some(active) => {
                clickable_active_icon(ui, icon.glyph, ui.visuals().text_color(), active, palette)
            }
            None => clickable_icon(ui, icon.glyph, palette),
        };

        if response
            .on_hover_text(
                egui::RichText::new(i18n.tr(icon.tooltip_key))
                    .size(palette.tooltip_text_size)
                    .color(palette.tooltip_text_color),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            icon.apply(action);
        }
    }
}

fn draw_drag_region(ui: &mut egui::Ui, hwnd: Option<HWND>) {
    // Remaining empty space: lets the user drag the window from the topbar.
    let drag_rect = ui.available_rect_before_wrap();
    if drag_rect.width() > 0.0 && drag_rect.height() > 0.0 {
        let resp = ui.allocate_rect(drag_rect, egui::Sense::click_and_drag());
        if resp.double_clicked() {
            if let Some(hwnd) = hwnd {
                toggle_window_fullscreen(hwnd);
            }
        }
        if (resp.drag_started() || resp.dragged()) && !crate::core::probe_active() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
}

/// A popup menu row: icon + label. `icon_active` paints the icon in the accent
/// color (used for toggles that are currently on).
fn menu_item(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    icon_active: bool,
    palette: &crate::gui::theme::ThemePalette,
) -> egui::Response {
    let text_galley = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(palette.text_size),
            ui.visuals().text_color(),
        )
    });
    let text_width = text_galley.size().x;
    let row_height = 18.0;
    let icon_width = 18.0;
    let spacing = 4.0;

    let total_size = egui::vec2(icon_width + spacing + text_width, row_height);

    let (rect, response) = ui.allocate_exact_size(total_size, egui::Sense::click());
    let mut x = rect.min.x;
    let center_y = rect.center().y;

    let icon_color = if response.hovered() || icon_active {
        palette.primary
    } else {
        ui.visuals().text_color()
    };
    ui.painter().text(
        egui::pos2(x, center_y),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::default(),
        icon_color,
    );
    x += icon_width + spacing;

    let text_color = if response.hovered() {
        palette.primary
    } else {
        ui.visuals().text_color()
    };
    ui.painter().text(
        egui::pos2(x, center_y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(palette.text_size),
        text_color,
    );

    response
}
