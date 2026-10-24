use crate::core::drives::{
    DriveInfo, consume_drive_list_dirty, get_drive_infos, is_raw_physical_drive_path,
};
use crate::core::fs::{MY_PC_PATH, MY_RECYCLE_BIN_PATH};
use crate::core::utils::text::apply_eden_text_overrides;
use crate::gui::i18n::I18n;
use crate::gui::icons::IconCache;
use crate::gui::theme::ThemePalette;
use crate::gui::utils::files::format_size;
use crate::gui::utils::{draw_object_drag_ghost, drive_usage_color, truncate_item_text};
use crate::gui::windows::containers::structs::{SidebarAction, TagGroup};
use crate::gui::windows::containers::tags::TAG_FOCUS_HIGHLIGHT;
use crate::gui::windows::structs::{SidebarSection, SidebarState};
use eframe::egui;
use egui::containers::{Popup, PopupCloseBehavior};
use egui::{FontFamily, FontId, ScrollArea};
use egui_phosphor::regular;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Narrowest width the sidebar separator can be dragged to.
pub const SIDEBAR_MIN_WIDTH: f32 = 140.0;
/// Upper bound for a stored sidebar width. At draw time the width is also
/// limited by the window, so this only rejects nonsensical stored values.
pub const SIDEBAR_MAX_WIDTH: f32 = 4000.0;
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 250.0;

/// Read-only view of the tags shown in the sidebar's Tags section.
pub struct SidebarTags<'a> {
    pub groups: &'a [TagGroup],
    /// The tags dock is open (the focused tag's row is highlighted).
    pub dock_open: bool,
    /// Tag last focused from the sidebar.
    pub focused: Option<u64>,
    /// Tag to flash briefly (e.g. after items were dropped on it).
    pub highlight: Option<(u64, Instant)>,
}

/// Draw the sidebar, supporting favorites reordering
#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar(
    ui: &mut egui::Ui,
    i18n: &I18n,
    icon_cache: &IconCache,
    sidebar_state: &mut SidebarState,
    tags: &SidebarTags,
    palette: &ThemePalette,
    drag_active: bool,
    drag_hover_target: Option<PathBuf>,
) -> SidebarAction {
    const DRIVE_CACHE_DURATION: Duration = Duration::from_secs(30);
    let mut action = SidebarAction::default();
    let mut drop_index: Option<usize> = None;
    let pointer_pos = ui.ctx().input(|i| i.pointer.hover_pos());
    let pointer_released = ui.ctx().input(|i| i.pointer.primary_released());
    let mut tab_drop_target: Option<PathBuf> = None;
    let hovered_target_ref = drag_hover_target.as_ref();
    if !drag_active {
        sidebar_state.spring_hover = None;
    }

    ScrollArea::vertical()
        .id_salt("sidebar_scroll")
        .auto_shrink([false; 2]) // don't shrink horizontally or vertically
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    // Headers are `SECTION_HEADER_PAD_Y` taller than the old plain
                    // labels on each side; the surrounding spaces shrink to match.
                    ui.add_space(8.0 - SECTION_HEADER_PAD_Y);
                    ui.spacing_mut().item_spacing.y *= palette.sidebar_item_spacing_y;

                    if section_header(
                        ui,
                        i18n,
                        sidebar_state,
                        SidebarSection::Places,
                        palette,
                        false,
                        drag_active,
                        pointer_pos,
                        &mut action,
                    ) {
                        ui.add_space(8.0 - SECTION_HEADER_PAD_Y);

                        let pc_icon_path = PathBuf::from("C:\\");
                        let resp = draw_sidebar_item(
                            ui,
                            icon_cache,
                            &pc_icon_path,
                            &i18n.tr("thispc"),
                            true,
                            false,
                            palette,
                            false,
                            None,
                        );
                        if resp.clicked() {
                            action.nav_to = Some(PathBuf::from(MY_PC_PATH));
                        }
                        if resp.middle_clicked() {
                            action.open_new_tab = Some(PathBuf::from(MY_PC_PATH));
                        }

                        Popup::context_menu(&resp)
                            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                apply_eden_text_overrides(ui, palette);
                                if ui.button(&i18n.tr("inputs_newtab")).clicked() {
                                    action.open_new_tab = Some(PathBuf::from(MY_PC_PATH));
                                    ui.close();
                                }
                            });

                        if let Some(home) = dirs::home_dir() {
                            let resp = draw_sidebar_item(
                                ui,
                                icon_cache,
                                &home,
                                &i18n.tr("my_user_home"),
                                true,
                                false,
                                palette,
                                false,
                                None,
                            );

                            if resp.clicked() {
                                action.nav_to = Some(home.clone());
                            }
                            if resp.middle_clicked() {
                                action.open_new_tab = Some(home.clone());
                            }
                            Popup::context_menu(&resp)
                                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                                .show(|ui| {
                                    apply_eden_text_overrides(ui, palette);
                                    if ui.button(&i18n.tr("inputs_newtab")).clicked() {
                                        action.open_new_tab = Some(home.clone());
                                        ui.close();
                                    }
                                });
                        }

                        let recycle_bin_path = PathBuf::from("C:\\$Recycle.Bin");
                        let resp = draw_sidebar_item(
                            ui,
                            icon_cache,
                            &recycle_bin_path,
                            &i18n.tr("recycle_bin"),
                            true,
                            true,
                            palette,
                            false,
                            None,
                        );
                        if resp.clicked() {
                            action.nav_to = Some(PathBuf::from(MY_RECYCLE_BIN_PATH));
                        }
                        if resp.middle_clicked() {
                            action.open_new_tab = Some(PathBuf::from(MY_RECYCLE_BIN_PATH));
                        }

                        Popup::context_menu(&resp)
                            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                apply_eden_text_overrides(ui, palette);
                                if ui.button(&i18n.tr("inputs_newtab")).clicked() {
                                    action.open_new_tab = Some(PathBuf::from(MY_RECYCLE_BIN_PATH));
                                    ui.close();
                                }
                            });
                    }

                    ui.add_space(6.0 - SECTION_HEADER_PAD_Y);
                    let favorites_expanded = section_header(
                        ui,
                        i18n,
                        sidebar_state,
                        SidebarSection::Favorites,
                        palette,
                        true,
                        drag_active,
                        pointer_pos,
                        &mut action,
                    );
                    if !favorites_expanded {
                        // A reorder drag cannot finish while the items are hidden.
                        sidebar_state.dragging_favorite = None;
                    }
                    if favorites_expanded {
                        ui.add_space(4.0 - SECTION_HEADER_PAD_Y);

                        let is_dragging = sidebar_state.dragging_favorite.is_some();
                        let mut item_layouts = Vec::with_capacity(sidebar_state.favorites.len());

                        for (i, _favorite) in sidebar_state.favorites.iter().enumerate() {
                            let (rect, resp) = favorites_item_layout(ui, palette);

                            if !is_dragging && resp.drag_started() {
                                sidebar_state.dragging_favorite = Some(i);
                            }

                            item_layouts.push((rect, resp));
                        }

                        if is_dragging {
                            if let (Some(pos), Some(drag_idx)) =
                                (pointer_pos, sidebar_state.dragging_favorite)
                            {
                                drop_index = None;

                                for (i, (rect, _)) in item_layouts.iter().enumerate() {
                                    let mid_y = rect.center().y;

                                    let new_index = if pos.y < mid_y { i } else { i + 1 };

                                    if new_index != drag_idx && new_index != drag_idx + 1 {
                                        drop_index = Some(new_index);
                                    }

                                    if pos.y < rect.bottom() {
                                        break;
                                    }
                                }

                                if let Some(last) = item_layouts.last() {
                                    if pos.y > last.0.bottom() {
                                        drop_index = Some(item_layouts.len());
                                    }
                                }
                            }
                        }

                        for (i, favorite) in sidebar_state.favorites.iter().enumerate() {
                            let (rect, resp) = &item_layouts[i];
                            draw_sidebar_item(
                                ui,
                                icon_cache,
                                &favorite.path,
                                &favorite.label,
                                true,
                                false,
                                palette,
                                true,
                                Some((*rect, resp.clone())),
                            );

                            if drag_active {
                                let hovered = hovered_target_ref
                                    .map(|target| target == &favorite.path)
                                    .unwrap_or_else(|| {
                                        pointer_pos
                                            .map(|pointer| rect.contains(pointer))
                                            .unwrap_or(false)
                                    });
                                if hovered {
                                    let painter = ui
                                        .ctx()
                                        .layer_painter(egui::LayerId::new(
                                            egui::Order::Background,
                                            ui.id().with("tab_drop_bg").with(favorite.path.clone()),
                                        ))
                                        .with_clip_rect(ui.clip_rect());
                                    painter.rect_filled(
                                        *rect,
                                        egui::CornerRadius::same(palette.medium_radius),
                                        palette.primary_hover,
                                    );

                                    if pointer_released {
                                        tab_drop_target = Some(favorite.path.clone());
                                    }
                                }
                            }

                            if !is_dragging && resp.drag_started() {
                                sidebar_state.dragging_favorite = Some(i);
                            }

                            if resp.clicked() {
                                action.nav_to = Some(favorite.path.clone());
                            }
                            if resp.secondary_clicked() {
                                action.select_favorite = Some(favorite.path.clone());
                            }
                            if resp.middle_clicked() {
                                action.open_new_tab = Some(favorite.path.clone());
                            }

                            Popup::context_menu(&resp)
                                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                                .show(|ui| {
                                    apply_eden_text_overrides(ui, palette);
                                    if ui.button(&i18n.tr("inputs_newtab")).clicked() {
                                        action.open_new_tab = Some(favorite.path.clone());
                                        ui.close();
                                    }
                                    if ui.button(&i18n.tr("remove_favorite")).clicked() {
                                        action.remove_favorite = Some(favorite.path.clone());
                                        ui.close();
                                    }
                                });

                            if let Some(drop) = drop_index {
                                if drop == i {
                                    let painter = ui.ctx().layer_painter(egui::LayerId::new(
                                        egui::Order::Background,
                                        egui::Id::new(format!("insert_line_{}", i)),
                                    ));

                                    let y = resp.rect.top();
                                    let left = resp.rect.left() + 6.0;
                                    let right = resp.rect.right() - 6.0;

                                    painter.line_segment(
                                        [egui::pos2(left, y), egui::pos2(right, y)],
                                        egui::Stroke::new(2.0, palette.primary_active),
                                    );
                                }
                            }
                        }

                        if is_dragging {
                            if let Some(drop) = drop_index {
                                if drop == sidebar_state.favorites.len() {
                                    if let Some(rect) = item_layouts.last().map(|(r, _)| r) {
                                        let painter = ui.ctx().layer_painter(egui::LayerId::new(
                                            egui::Order::Background,
                                            egui::Id::new("insert_line_end"),
                                        ));

                                        let y = rect.bottom();
                                        let left = rect.left() + 6.0;
                                        let right = rect.right() - 6.0;

                                        painter.line_segment(
                                            [egui::pos2(left, y), egui::pos2(right, y)],
                                            egui::Stroke::new(2.0, palette.primary_active),
                                        );
                                    }
                                }
                            }

                            if let Some(drag_idx) = sidebar_state.dragging_favorite {
                                draw_object_drag_ghost(
                                    ui,
                                    palette,
                                    &sidebar_state.favorites[drag_idx].label,
                                    true,
                                );
                            }

                            if let Some(from) = sidebar_state.dragging_favorite {
                                if pointer_released {
                                    if let Some(to) = drop_index {
                                        action.reorder = Some((from, to));
                                    }

                                    sidebar_state.dragging_favorite = None;
                                    drop_index = None;
                                }
                            }
                        }
                    }

                    ui.add_space(6.0 - SECTION_HEADER_PAD_Y);
                    if section_header(
                        ui,
                        i18n,
                        sidebar_state,
                        SidebarSection::Tags,
                        palette,
                        true,
                        drag_active,
                        pointer_pos,
                        &mut action,
                    ) {
                        ui.add_space(4.0 - SECTION_HEADER_PAD_Y);
                        // Internal (egui pointer) drags only: a native OLE drag
                        // hovering the window cannot be dropped on a tag.
                        let internal_drag = drag_active && drag_hover_target.is_none();
                        draw_sidebar_tags(
                            ui,
                            i18n,
                            palette,
                            tags,
                            internal_drag,
                            pointer_pos,
                            pointer_released,
                            &mut action,
                        );
                    }

                    ui.add_space(6.0 - SECTION_HEADER_PAD_Y);
                    // While collapsed the drive list is neither drawn nor refreshed
                    // (no `get_drive_infos` calls); expanding refreshes it if stale.
                    if section_header(
                        ui,
                        i18n,
                        sidebar_state,
                        SidebarSection::Storage,
                        palette,
                        false,
                        drag_active,
                        pointer_pos,
                        &mut action,
                    ) {
                        ui.add_space(4.0 - SECTION_HEADER_PAD_Y);

                        if sidebar_state.cached_drives.is_empty()
                            || sidebar_state.last_drive_refresh.elapsed() > DRIVE_CACHE_DURATION
                            || consume_drive_list_dirty()
                        {
                            sidebar_state.cached_drives = get_drive_infos();
                            sidebar_state.last_drive_refresh = Instant::now();
                        }

                        for drive in sidebar_state.cached_drives.iter() {
                            let is_selected = sidebar_state
                                .item_clicked
                                .as_ref()
                                .map(|p| p == &drive.path)
                                .unwrap_or(false);

                            let resp =
                                sidebar_drive_item(ui, icon_cache, &drive, palette, is_selected);
                            if resp.clicked() {
                                if is_raw_physical_drive_path(&drive.path) {
                                    sidebar_state.non_ntfs_popup_path = Some(drive.path.clone());
                                } else {
                                    action.nav_to = Some(drive.path.clone());
                                }
                            }
                            if resp.middle_clicked() {
                                if is_raw_physical_drive_path(&drive.path) {
                                    sidebar_state.non_ntfs_popup_path = Some(drive.path.clone());
                                } else {
                                    action.open_new_tab = Some(drive.path.clone());
                                }
                            }
                        }
                    }

                    // Not part of the storage section: stays open even if the
                    // section is collapsed meanwhile.
                    if let Some(_path) = sidebar_state.non_ntfs_popup_path.clone() {
                        let mut open = true;
                        egui::Window::new(&i18n.tr("non_nftsdrive"))
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                            .open(&mut open)
                            .show(ui.ctx(), |ui| {
                                ui.label(&i18n.tr("non_nftsdrive_label1"));
                                ui.label(&i18n.tr("non_nftsdrive_label2"));
                                ui.label(&i18n.tr("non_nftsdrive_label3"));
                                ui.add_space(8.0);
                                if ui.button(&i18n.tr("ok")).clicked() {
                                    sidebar_state.non_ntfs_popup_path = None;
                                }
                            });
                        if !open {
                            sidebar_state.non_ntfs_popup_path = None;
                        }
                    }

                    // Extra bottom padding so last items aren't clipped by scroll boundary.
                    ui.add_space(12.0);
                });
                ui.add_space(2.0);
            });
        });

    action.move_files_to_sidebar_dir = tab_drop_target;
    action
}

/// Rows of the Tags section: color dot + name + object count. Click opens
/// the tags dock focused on that tag; dropping dragged items adds them to it.
#[allow(clippy::too_many_arguments)]
fn draw_sidebar_tags(
    ui: &mut egui::Ui,
    i18n: &I18n,
    palette: &ThemePalette,
    tags: &SidebarTags,
    internal_drag: bool,
    pointer_pos: Option<egui::Pos2>,
    pointer_released: bool,
    action: &mut SidebarAction,
) {
    if tags.groups.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(i18n.tr("tag_empty_state"))
                        .size(palette.text_size)
                        .color(ui.visuals().weak_text_color()),
                )
                .wrap(),
            );
        });
        return;
    }

    let highlight = tags
        .highlight
        .filter(|(_, since)| since.elapsed() < TAG_FOCUS_HIGHLIGHT);
    let hint = i18n.tr("tag_sidebar_hint");
    for group in tags.groups {
        let selected = tags.dock_open && tags.focused == Some(group.id);
        let flash = highlight
            .filter(|(id, _)| *id == group.id)
            .map(|(_, since)| {
                1.0 - since.elapsed().as_secs_f32() / TAG_FOCUS_HIGHLIGHT.as_secs_f32()
            });
        let (resp, drop_hover) = sidebar_tag_item(
            ui,
            palette,
            group,
            selected,
            flash,
            internal_drag,
            pointer_pos,
            &hint,
        );
        if drop_hover && pointer_released {
            action.add_to_tag = Some(group.id);
        }
        if resp.clicked() {
            action.open_tag = Some(group.id);
        }
        Popup::context_menu(&resp)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                apply_eden_text_overrides(ui, palette);
                if ui.button(i18n.tr("tag_show_in_panel")).clicked() {
                    action.open_tag = Some(group.id);
                    ui.close();
                }
            });
    }
}

/// One tag row. Returns the response and whether an internal item drag is
/// hovering the row (a release there drops onto the tag).
#[allow(clippy::too_many_arguments)]
fn sidebar_tag_item(
    ui: &mut egui::Ui,
    palette: &ThemePalette,
    group: &TagGroup,
    selected: bool,
    flash: Option<f32>,
    internal_drag: bool,
    pointer_pos: Option<egui::Pos2>,
    hint: &str,
) -> (egui::Response, bool) {
    let height = if palette.sidebar_item_spacing_y > 1.0 {
        18.0 * palette.sidebar_item_spacing_y
    } else {
        18.0
    };
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    // egui does not report hover while another widget owns the drag, so test
    // the pointer position directly (like the favorites drop targets).
    let drop_hover = internal_drag && pointer_pos.is_some_and(|pos| rect.contains(pos));

    let radius = egui::CornerRadius::same(palette.medium_radius);
    let fill = if selected || resp.is_pointer_button_down_on() {
        palette.primary_active
    } else if resp.hovered() || drop_hover {
        palette.primary_hover
    } else {
        egui::Color32::TRANSPARENT
    };
    let painter = ui.painter();
    if fill != egui::Color32::TRANSPARENT {
        painter.rect_filled(rect, radius, fill);
    }
    if let Some(alpha) = flash {
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(
                1.5,
                palette.primary_active.gamma_multiply(alpha.clamp(0.0, 1.0)),
            ),
            egui::StrokeKind::Inside,
        );
        ui.ctx().request_repaint();
    }
    if drop_hover {
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.5, palette.primary_active),
            egui::StrokeKind::Inside,
        );
    }

    // Color dot centered in the icon column of the other sidebar rows.
    let icon_size = palette.sidebar_icon_size;
    let dot_center = egui::pos2(rect.min.x + 4.0 + icon_size * 0.5, rect.center().y);
    let [r, g, b, _] = group.color.to_array();
    let dot_outline = egui::Color32::from_rgb(
        (r as f32 * 0.6) as u8,
        (g as f32 * 0.6) as u8,
        (b as f32 * 0.6) as u8,
    );
    painter.circle(
        dot_center,
        (icon_size * 0.28).clamp(3.0, 6.0),
        egui::Color32::from_rgb(r, g, b),
        egui::Stroke::new(1.0, dot_outline),
    );

    // Count, right aligned and dimmed.
    let font_id = FontId::new(palette.text_size, FontFamily::Proportional);
    let count_galley = painter.layout_no_wrap(
        group.items.len().to_string(),
        font_id.clone(),
        ui.visuals().weak_text_color(),
    );
    let count_width = count_galley.size().x;
    let count_pos = egui::pos2(
        rect.right() - 6.0 - count_width,
        rect.center().y - count_galley.size().y * 0.5,
    );
    painter.galley(count_pos, count_galley, ui.visuals().weak_text_color());

    // Name, truncated before the count.
    let text_offset_x = palette.text_size + icon_size + 4.0;
    let text_width = (rect.width() - text_offset_x - count_width - 12.0).max(0.0);
    let color = ui.visuals().text_color();
    let (display_name, truncated) =
        truncate_item_text(ui, &group.name, text_width, &font_id, color);
    ui.painter().text(
        egui::pos2(rect.min.x + text_offset_x, rect.center().y - 2.0),
        egui::Align2::LEFT_CENTER,
        display_name,
        font_id,
        color,
    );

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &group.name));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = if internal_drag {
        resp
    } else {
        let tooltip = if truncated {
            format!("{}\n{}", group.name, hint)
        } else {
            hint.to_string()
        };
        resp.on_hover_text(
            egui::RichText::new(tooltip)
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
    };
    (resp, drop_hover)
}

/// Extra vertical padding of a section header row above and below its text.
const SECTION_HEADER_PAD_Y: f32 = 2.0;
/// How long a file drag must hover a collapsed section header to expand it.
const SPRING_LOAD_DELAY: Duration = Duration::from_millis(600);

/// Draws a collapsible section header and handles toggling it (click, keyboard
/// and, if `spring_load`, a file drag resting on the collapsed header).
/// Returns whether the section is expanded.
#[allow(clippy::too_many_arguments)]
fn section_header(
    ui: &mut egui::Ui,
    i18n: &I18n,
    sidebar_state: &mut SidebarState,
    section: SidebarSection,
    palette: &ThemePalette,
    spring_load: bool,
    drag_active: bool,
    pointer_pos: Option<egui::Pos2>,
    action: &mut SidebarAction,
) -> bool {
    let collapsed = sidebar_state.collapsed_sections.is_collapsed(section);
    let label = i18n.tr(section.key());
    let resp = draw_section_header(ui, &label, collapsed, palette, drag_active, pointer_pos);
    let mut toggle = resp.clicked();

    if spring_load && collapsed && drag_active && !toggle {
        let over = pointer_pos.is_some_and(|pos| resp.rect.contains(pos));
        match sidebar_state.spring_hover {
            Some((hovered, since)) if hovered == section => {
                if !over {
                    sidebar_state.spring_hover = None;
                } else if since.elapsed() >= SPRING_LOAD_DELAY {
                    toggle = true;
                } else {
                    ui.ctx()
                        .request_repaint_after(SPRING_LOAD_DELAY.saturating_sub(since.elapsed()));
                }
            }
            _ if over => {
                sidebar_state.spring_hover = Some((section, Instant::now()));
                ui.ctx().request_repaint_after(SPRING_LOAD_DELAY);
            }
            _ => {}
        }
    }

    if toggle {
        sidebar_state.collapsed_sections.set(section, !collapsed);
        sidebar_state.spring_hover = None;
        action.sections_changed = true;
        ui.ctx().request_repaint();
    }
    !sidebar_state.collapsed_sections.is_collapsed(section)
}

/// Full-width clickable section header: caret + bold header text.
fn draw_section_header(
    ui: &mut egui::Ui,
    label: &str,
    collapsed: bool,
    palette: &ThemePalette,
    drag_active: bool,
    pointer_pos: Option<egui::Pos2>,
) -> egui::Response {
    let caret_size = (palette.text_size * 0.9).round();
    let caret_x = 4.0;
    let text_x = caret_x + caret_size + 4.0;

    let available_width = ui.available_width();
    let galley = egui::WidgetText::from(
        egui::RichText::new(label)
            .size(palette.text_size)
            .color(palette.text_header_section)
            .strong(),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (available_width - text_x).max(0.0),
        egui::TextStyle::Body,
    );
    let height = galley.size().y + SECTION_HEADER_PAD_Y * 2.0;

    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(available_width, height), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));

    // Pointer feedback like the sidebar items (also while a file drag rests on
    // the header, where egui does not report hover).
    let drag_over = drag_active && pointer_pos.is_some_and(|pos| rect.contains(pos));
    let fill = if resp.is_pointer_button_down_on() {
        palette.primary_active
    } else if resp.hovered() || drag_over {
        palette.primary_hover
    } else {
        egui::Color32::TRANSPARENT
    };
    let painter = ui.painter();
    let radius = egui::CornerRadius::same(palette.medium_radius);
    if fill != egui::Color32::TRANSPARENT {
        painter.rect_filled(rect, radius, fill);
    }
    if resp.has_focus() {
        painter.rect_stroke(
            rect,
            radius,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }

    painter.text(
        egui::pos2(rect.min.x + caret_x + caret_size * 0.5, rect.center().y),
        egui::Align2::CENTER_CENTER,
        if collapsed {
            regular::CARET_RIGHT
        } else {
            regular::CARET_DOWN
        },
        FontId::new(caret_size, FontFamily::Proportional),
        palette.text_header_section,
    );
    painter.galley(
        egui::pos2(rect.min.x + text_x, rect.center().y - galley.size().y * 0.5),
        galley,
        palette.text_header_section,
    );

    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn draw_sidebar_item(
    ui: &mut egui::Ui,
    icon_cache: &IconCache,
    path: &PathBuf,
    label: &str,
    is_dir: bool,
    is_recycle_bin: bool,
    palette: &ThemePalette,
    draggable: bool,
    rect_and_resp: Option<(egui::Rect, egui::Response)>,
) -> egui::Response {
    let height = if palette.sidebar_item_spacing_y > 1.0 {
        18.0 * palette.sidebar_item_spacing_y
    } else {
        18.0
    };

    // --- Get rect + response ---
    let (rect, resp) = if let Some((rect, resp)) = rect_and_resp {
        (rect, resp)
    } else {
        let available_width = ui.available_width();
        ui.allocate_exact_size(
            egui::vec2(available_width, height),
            if draggable {
                egui::Sense::click_and_drag()
            } else {
                egui::Sense::click()
            },
        )
    };

    // --- Hover background + cursor ---
    if resp.hovered() {
        ui.ctx().set_cursor_icon(if draggable {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Default
        });

        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(palette.medium_radius),
            palette.primary_hover,
        );

        if draggable {
            let handle_width = 12.0;
            let handle_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - handle_width - 4.0, rect.top()),
                egui::vec2(handle_width, rect.height()),
            );

            ui.painter().text(
                handle_rect.center(),
                egui::Align2::CENTER_CENTER,
                regular::DOTS_SIX_VERTICAL,
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
                palette.icon_color,
            );
        }
    }

    // --- Icon ---
    let icon_size = egui::vec2(palette.sidebar_icon_size, palette.sidebar_icon_size);
    let icon_padding = 4.0;

    let icon_pos = egui::pos2(rect.min.x + 4.0, rect.center().y - icon_size.y / 2.0);

    let text_offset_x = if is_recycle_bin {
        ui.painter().text(
            egui::pos2(icon_pos.x + icon_size.x * 0.5, rect.center().y),
            egui::Align2::CENTER_CENTER,
            regular::TRASH,
            egui::FontId::new(icon_size.y * 0.85, egui::FontFamily::Proportional),
            palette.icon_color,
        );

        palette.text_size + icon_size.x + icon_padding
    } else if let Some(glyph) = icon_cache.get_custom_folder_icon(path, is_dir) {
        // Custom Phosphor icon for recognized folders.
        ui.painter().text(
            egui::pos2(icon_pos.x + icon_size.x * 0.5, rect.center().y),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::new(icon_size.y, egui::FontFamily::Proportional),
            palette.icon_color,
        );

        palette.text_size + icon_size.x + icon_padding
    } else if let Some(icon) = icon_cache.get(path, is_dir) {
        // Windows shell icon fallback.
        ui.painter().image(
            (&icon).into(),
            egui::Rect::from_min_size(icon_pos, icon_size),
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        palette.text_size + icon_size.x + icon_padding
    } else {
        palette.text_size + 20.0 + icon_padding
    };

    // --- Text ---
    let text_width = rect.width() - text_offset_x;
    let font_id = egui::FontId::new(palette.text_size, egui::FontFamily::Proportional);
    let color = ui.visuals().text_color();

    let (display_name, truncated) = truncate_item_text(ui, label, text_width, &font_id, color);

    let text_pos = egui::pos2(rect.min.x + text_offset_x, rect.center().y - 2.0);

    ui.painter().text(
        text_pos,
        egui::Align2::LEFT_CENTER,
        display_name,
        font_id,
        color,
    );

    // --- Tooltip + cursor ---
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    if truncated {
        resp.on_hover_text(
            egui::RichText::new(label)
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
    } else {
        resp.on_hover_text(
            egui::RichText::new(path.to_string_lossy())
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
    }
}

fn favorites_item_layout(
    ui: &mut egui::Ui,
    palette: &ThemePalette,
) -> (egui::Rect, egui::Response) {
    let available_width = ui.available_width();

    let height = if palette.sidebar_item_spacing_y > 1.0 {
        18.0 * palette.sidebar_item_spacing_y
    } else {
        18.0
    };

    ui.allocate_exact_size(
        egui::vec2(available_width, height),
        egui::Sense::click_and_drag(),
    )
}

/// Height of the thin usage bar under a drive row's label.
const DRIVE_BAR_HEIGHT: f32 = 3.0;
/// Gap between the label line and the usage bar.
const DRIVE_BAR_GAP: f32 = 1.0;

/// Draw a compact single-line drive item: icon + label with a thin usage bar
/// under the label; used/total size in the tooltip.
fn sidebar_drive_item(
    ui: &mut egui::Ui,
    icon_cache: &IconCache,
    drive: &DriveInfo,
    palette: &ThemePalette,
    selected: bool,
) -> egui::Response {
    let available_width = ui.available_width();
    // Same line height as the other sidebar items, plus room for the bar.
    let line_height = if palette.sidebar_item_spacing_y > 1.0 {
        18.0 * palette.sidebar_item_spacing_y
    } else {
        18.0
    };
    let height = line_height + DRIVE_BAR_GAP + DRIVE_BAR_HEIGHT + 1.0;

    let (rect, mut resp) =
        ui.allocate_exact_size(egui::vec2(available_width, height), egui::Sense::click());

    // Background (selected > active click > hover)
    let fill_color = if selected {
        palette.primary_active
    } else if resp.is_pointer_button_down_on() {
        palette.primary_active
    } else if resp.hovered() {
        palette.primary_hover
    } else {
        egui::Color32::TRANSPARENT
    };

    if fill_color != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(palette.medium_radius),
            fill_color,
        );
    }

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // --- Top row: icon + label ---
    let icon_size = egui::vec2(palette.sidebar_icon_size, palette.sidebar_icon_size);
    let icon_padding = 4.0;

    // Icon and label are centered on the label line (the row minus the bar).
    let line_center_y = rect.min.y + line_height / 2.0;
    let text_offset_x = if let Some(icon) = icon_cache.get(&drive.path, true) {
        let icon_pos = egui::pos2(rect.min.x + 4.0, line_center_y - icon_size.y / 2.0);

        ui.painter().image(
            (&icon).into(),
            egui::Rect::from_min_size(icon_pos, icon_size),
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
            palette.icon_windows,
        );

        palette.text_size + icon_size.x + icon_padding
    } else {
        palette.text_size + 20.0 + icon_padding
    };

    // --- Label ---
    let font_id = FontId::new(palette.text_size, FontFamily::Proportional);
    let color = ui.visuals().text_color();
    let text_width = (available_width - text_offset_x - 4.0).max(0.0);
    let (display_name, truncated) =
        truncate_item_text(ui, &drive.display, text_width, &font_id, color);

    ui.painter().text(
        // Same baseline nudge as `draw_sidebar_item`.
        egui::pos2(rect.min.x + text_offset_x, line_center_y - 2.0),
        egui::Align2::LEFT_CENTER,
        display_name,
        font_id,
        color,
    );

    // --- Thin usage bar under the label ---
    let usage = match (drive.total_space, drive.free_space) {
        (Some(total), Some(free)) if total > 0 => Some((total, total.saturating_sub(free))),
        _ => None,
    };
    if let Some((total, used)) = usage {
        let used_ratio = (used as f64 / total as f64).clamp(0.0, 1.0) as f32;
        let bar_left = rect.min.x + text_offset_x;
        let bar_right = (rect.right() - 8.0).max(bar_left);
        let bar_top = rect.min.y + line_height + DRIVE_BAR_GAP;
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(bar_left, bar_top),
            egui::pos2(bar_right, bar_top + DRIVE_BAR_HEIGHT),
        );
        let radius = egui::CornerRadius::same((DRIVE_BAR_HEIGHT / 2.0) as u8);

        ui.painter()
            .rect_filled(bar_rect, radius, palette.drive_usage_background);
        let fill_rect = egui::Rect::from_min_size(
            bar_rect.min,
            egui::vec2(bar_rect.width() * used_ratio, DRIVE_BAR_HEIGHT),
        );
        ui.painter()
            .rect_filled(fill_rect, radius, drive_usage_color(used_ratio, palette));
    }

    // --- Tooltip: full name (if truncated) + used/total ---
    // Same "free / total" wording as the drive list in the item viewer, plus
    // the used percentage the bar shows. Built only while the tooltip is shown.
    if truncated || usage.is_some() {
        resp = resp.on_hover_ui(|ui| {
            let size_text = usage.map(|(total, used)| {
                format!(
                    "{} / {} ({:.0}%)",
                    format_size(total - used),
                    format_size(total),
                    used as f64 / total as f64 * 100.0
                )
            });
            let tooltip = match (truncated, size_text) {
                (true, Some(size)) => format!("{}\n{}", drive.display, size),
                (true, None) => drive.display.clone(),
                (false, Some(size)) => size,
                (false, None) => String::new(),
            };
            ui.label(
                egui::RichText::new(tooltip)
                    .size(palette.tooltip_text_size)
                    .color(palette.tooltip_text_color),
            );
        });
    }

    resp
}
