use crate::core::fs::FileItem;
use crate::gui::i18n::I18n;
use crate::gui::icons::IconCache;
use crate::gui::theme::ThemePalette;
use crate::gui::utils::{clickable_icon, draw_object_drag_ghost, rgba_color_edit_button};
use crate::gui::windows::containers::enums::{ItemViewerAction, ItemViewerContextAction};
use crate::gui::windows::containers::sidebar::draw_sidebar_item;
use crate::gui::windows::containers::structs::TagsState;
use eframe::egui;
use egui::FontId;
use egui::ScrollArea;
use egui::containers::{Popup, PopupCloseBehavior};
use egui_phosphor::regular;
use std::path::PathBuf;

/// Default height of the tags panel docked below the explorer content.
pub const TAGS_DOCK_DEFAULT_HEIGHT: f32 = 240.0;
/// Smallest height the docked tags panel can be resized to.
pub const TAGS_DOCK_MIN_HEIGHT: f32 = 100.0;
/// Upper bound for a stored dock height. At draw time the height is also
/// limited by the window, so this only rejects nonsensical stored values.
pub const TAGS_DOCK_MAX_HEIGHT: f32 = 4000.0;

/// Smallest height kept for the explorer content above the tags dock.
const TAGS_DOCK_CONTENT_MIN_HEIGHT: f32 = 150.0;
/// Height of the resize separator between the content and the dock.
const TAGS_DOCK_SEPARATOR_HEIGHT: f32 = 6.0;
/// Below this height (tiny window) the dock is not drawn at all.
const TAGS_DOCK_HIDE_BELOW: f32 = 40.0;
/// The explorer content container's left margin; the dock lines up with it.
const TAGS_DOCK_LEFT_INSET: f32 = 3.0;

/// Where the explorer content, the resize separator and the docked tags
/// panel go inside the explorer content area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagsDockLayout {
    pub content: egui::Rect,
    pub separator: egui::Rect,
    pub panel: egui::Rect,
    /// Panel height actually drawn (the preference clamped to the window).
    pub height: f32,
    pub min_height: f32,
    pub max_height: f32,
}

/// Splits `area` into explorer content (top) and tags dock (bottom) for a
/// preferred dock height. The content keeps at least
/// `TAGS_DOCK_CONTENT_MIN_HEIGHT`, so in a small window the dock shrinks first
/// (below its own minimum if needed) and is hidden (`None`) when it would be
/// unusably small.
pub fn layout_tags_dock(
    area: egui::Rect,
    preferred_height: f32,
    pixels_per_point: f32,
) -> Option<TagsDockLayout> {
    let max_height =
        (area.height() - TAGS_DOCK_CONTENT_MIN_HEIGHT - TAGS_DOCK_SEPARATOR_HEIGHT).max(0.0);
    let min_height = TAGS_DOCK_MIN_HEIGHT.min(max_height);
    let preferred = if preferred_height.is_finite() {
        preferred_height
    } else {
        TAGS_DOCK_DEFAULT_HEIGHT
    };
    let height = preferred.clamp(min_height, max_height);
    if height < TAGS_DOCK_HIDE_BELOW {
        return None;
    }

    let ppp = pixels_per_point.max(0.01);
    let panel_top = ((area.bottom() - height) * ppp).round() / ppp;
    let separator_top = panel_top - TAGS_DOCK_SEPARATOR_HEIGHT;
    let left = area.left() + TAGS_DOCK_LEFT_INSET;
    Some(TagsDockLayout {
        content: egui::Rect::from_min_max(area.min, egui::pos2(area.right(), separator_top)),
        separator: egui::Rect::from_min_max(
            egui::pos2(left, separator_top),
            egui::pos2(area.right(), panel_top),
        ),
        panel: egui::Rect::from_min_max(egui::pos2(left, panel_top), area.max),
        height,
        min_height,
        max_height,
    })
}

/// What happened in the docked tags panel this frame.
#[derive(Default)]
pub struct TagsPanelResponse {
    /// Tags were edited (renamed, recolored, reordered); persist them.
    pub changed: bool,
    /// The panel's close button was clicked.
    pub close_requested: bool,
}

/// Height of the docked panel's header row (tag icon, title, close button).
const TAGS_DOCK_HEADER_HEIGHT: f32 = 28.0;
/// How long a tag focused from the sidebar stays highlighted.
pub const TAG_FOCUS_HIGHLIGHT: std::time::Duration = std::time::Duration::from_millis(1000);

/// Draws the tags panel docked below the explorer content: a slim header
/// (title + close button) and the tag groups. `focused` paints the accent
/// line that shows the panel has keyboard focus (explorer shortcuts are off).
pub fn draw_tags(
    ui: &mut egui::Ui,
    i18n: &I18n,
    icon_cache: &IconCache,
    palette: &ThemePalette,
    tags_state: &mut TagsState,
    focused: bool,
) -> TagsPanelResponse {
    let mut changed = false;
    let mut close_requested = false;
    let mut drag_state = tags_state.drag_state.take();
    let mut rename_state = tags_state.rename_state.take();
    let mut delete_confirmation = tags_state.delete_confirmation.take();
    let pending_focus = tags_state.pending_focus.take();
    let highlight = tags_state
        .highlight
        .filter(|(_, since)| since.elapsed() < TAG_FOCUS_HIGHLIGHT);
    if highlight.is_none() {
        tags_state.highlight = None;
    }
    let pointer_pos = ui.ctx().input(|input| input.pointer.hover_pos());
    let pointer_released = ui.ctx().input(|input| input.pointer.primary_released());
    let groups_len = tags_state.groups.len();

    let tabs_width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(tabs_width, ui.available_height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            if draw_dock_header(ui, i18n, palette, focused) {
                close_requested = true;
            }

                if tags_state.groups.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(i18n.tr("tag_empty_state"))
                                .size(palette.text_size)
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                    return;
                }

                ScrollArea::vertical()
                    .id_salt("tags_dock_scroll")
                    .auto_shrink([false; 2])
                    .max_height(ui.available_height())
                    .min_scrolled_height(ui.available_height())
                    .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.add_space(-4.0); // tighten the gap below the dock header
                    ui.spacing_mut().item_spacing.y = 8.0;

                    for group_index in 0..groups_len {
                        let group_id = tags_state.groups[group_index].id;
                        let group_name = tags_state.groups[group_index].name.clone();
                        let group_items = tags_state.groups[group_index].items.clone();
                        let group_color = tags_state.groups[group_index].color;
                        let focus_this_group = pending_focus == Some(group_id);
                        let highlight_alpha = highlight
                            .filter(|(id, _)| *id == group_id)
                            .map(|(_, since)| {
                                1.0 - since.elapsed().as_secs_f32()
                                    / TAG_FOCUS_HIGHLIGHT.as_secs_f32()
                            });
                        let editing_this_group = rename_state
                            .as_ref()
                            .map(|state| state.group_id == group_id)
                            .unwrap_or(false);
                        let drag_source_index = drag_state
                            .as_ref()
                            .filter(|drag| drag.group_id == group_id && drag.active)
                            .map(|drag| drag.source_index);
                        let mut drop_index: Option<usize> = None;
                        let mut should_clear_drag = false;
                        let mut clear_rename_state = false;

                        let group = &mut tags_state.groups[group_index];

                        let group_frame = egui::Frame::NONE
                            .stroke(egui::Stroke::NONE)
                            .fill(egui::Color32::TRANSPARENT)
                            .inner_margin(egui::Margin::symmetric(10, 10));

                        let group_response = group_frame.show(ui, |ui| {
                            ui.set_width(ui.available_width());

                            let header_id = ui.make_persistent_id(("tag_group", group_id));
                            let mut header_state =
                                egui::collapsing_header::CollapsingState::load_with_default_open(
                                    ui.ctx(),
                                    header_id,
                                    true,
                                );
                            if focus_this_group {
                                // Focused from the sidebar: always show its objects.
                                header_state.set_open(true);
                            }

                            let header_response = header_state.show_header(ui, |ui| {
                                if editing_this_group {
                                    if let Some(rename) = rename_state
                                        .as_mut()
                                        .filter(|state| state.group_id == group_id)
                                    {
                                        let edit_id = ui.id().with("tag_rename_input").with(group_id);
                                        let edit_response = ui.add(
                                            egui::TextEdit::singleline(&mut rename.buffer)
                                                .id(edit_id)
                                                .desired_width(ui.available_width() - 180.0)
                                                .font(egui::FontId::new(
                                                    palette.text_size,
                                                    egui::FontFamily::Proportional,
                                                )),
                                        );

                                        if rename.should_focus {
                                            ui.memory_mut(|mem| mem.request_focus(edit_id));
                                            edit_response.request_focus();
                                            if edit_response.has_focus() {
                                                rename.should_focus = false;
                                            }
                                        }

                                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));

                                        if enter || edit_response.lost_focus() {
                                            let new_name = rename.buffer.trim().to_string();
                                            if !new_name.is_empty() && group.name != new_name {
                                                group.name = new_name;
                                                changed = true;
                                            }
                                            clear_rename_state = true;
                                        } else if escape {
                                            clear_rename_state = true;
                                        }
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new(&group_name)
                                            .size(palette.text_size)
                                            .color(ui.visuals().text_color())
                                            .strong(),
                                    );
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if rgba_color_edit_button(ui, &mut group.color).changed() {
                                            changed = true;
                                        }
                                        ui.label(
                                            egui::RichText::new(format!("({})", group_items.len()))
                                                .size(palette.text_size)
                                                .color(palette.tooltip_text_color),
                                        );
                                        if clickable_icon(ui, regular::TRASH, palette)
                                            .on_hover_text(i18n.tr("tag_delete_group"))
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            delete_confirmation = Some(group_id);
                                        }
                                        if clickable_icon(ui, regular::PENCIL_SIMPLE, palette)
                                            .on_hover_text(i18n.tr("inputs_rename"))
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            rename_state = Some(
                                                crate::gui::windows::containers::structs::TagRenameState {
                                                    group_id,
                                                    buffer: group_name.clone(),
                                                    should_focus: true,
                                                },
                                            );
                                        }
                                    },
                                );
                            });

                            let (_, header_inner, _) = header_response.body(|ui| {
                                let collapsible_body_frame = egui::Frame::NONE
                                    .stroke(egui::Stroke::new(1.0, group_color.gamma_multiply(0.8)))
                                    .fill(palette.row_bg.linear_multiply(0.18))
                                    .inner_margin(egui::Margin::symmetric(10, 10));

                                collapsible_body_frame.show(ui, |ui| {
                                    if group_items.is_empty() {
                                        ui.label(
                                            egui::RichText::new(i18n.tr("tag_empty_group"))
                                                .size(palette.text_size)
                                                .color(palette.text_normal),
                                        );
                                        return;
                                    }

                                    let drag_is_active = drag_source_index.is_some();
                                    let mut item_rects = Vec::with_capacity(group_items.len());
                                    let mut item_responses = Vec::with_capacity(group_items.len());

                                    if drag_is_active {
                                        for _ in &group_items {
                                            let (rect, resp) = tag_item_layout(ui);
                                            item_rects.push(rect);
                                            item_responses.push(resp);
                                        }

                                        if let Some(drag_source_index) = drag_source_index {
                                            if let Some(pointer) = pointer_pos {
                                                drop_index = compute_drop_index(
                                                    &item_rects,
                                                    pointer.y,
                                                    drag_source_index,
                                                );
                                            }
                                        }
                                    }

                                    for (item_index, path) in group_items.iter().enumerate() {
                                        let label = tag_item_label(path);
                                        // Cached: a file system stat per row per frame
                                        // is too slow for large tags.
                                        let is_dir = match tags_state.is_dir_cache.get(path) {
                                            Some(is_dir) => *is_dir,
                                            None => {
                                                let is_dir = path.is_dir();
                                                tags_state
                                                    .is_dir_cache
                                                    .insert(path.clone(), is_dir);
                                                is_dir
                                            }
                                        };
                                        let resp = if drag_is_active {
                                            let rect = item_rects[item_index];
                                            let item_resp = item_responses[item_index].clone();
                                            draw_sidebar_item(
                                                ui,
                                                icon_cache,
                                                path,
                                                &label,
                                                is_dir,
                                                false,
                                                palette,
                                                true,
                                                Some((rect, item_resp)),
                                            )
                                        } else {
                                            draw_sidebar_item(
                                                ui,
                                                icon_cache,
                                                path,
                                                &label,
                                                is_dir,
                                                false,
                                                palette,
                                                true,
                                                None,
                                            )
                                        };

                                        if drag_state.is_none() && resp.drag_started() {
                                            drag_state = Some(crate::gui::windows::containers::structs::TagDragState {
                                                group_id,
                                                source_index: item_index,
                                                active: true,
                                            });
                                        }

                                        if resp.clicked() && drag_state.is_none() {
                                            let file_item = FileItem {
                                                name: label.clone(),
                                                path: path.clone(),
                                                is_dir,
                                                is_hidden: false,
                                                recycle_bin_pidl: None,
                                                file_size: None,
                                                modified_time: None,
                                                created_time: None,
                                                deleted_time: None,
                                                modified_time_raw: None,
                                                created_time_raw: None,
                                                deleted_time_raw: None,
                                                original_directory: None,
                                                total_space: None,
                                                free_space: None,
                                            };
                                            if let Some(action) = handle_row_click(&file_item) {
                                                tags_state.pending_action = Some(action);
                                            }
                                        }

                                        Popup::context_menu(&resp)
                                            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                                            .show(|ui| {
                                                let file_item = FileItem {
                                                    name: label.clone(),
                                                    path: path.clone(),
                                                    is_dir,
                                                    is_hidden: false,
                                                    recycle_bin_pidl: None,
                                                    file_size: None,
                                                    modified_time: None,
                                                    created_time: None,
                                                    deleted_time: None,
                                                    modified_time_raw: None,
                                                    created_time_raw: None,
                                                    deleted_time_raw: None,
                                                    original_directory: None,
                                                    total_space: None,
                                                    free_space: None,
                                                };
                                                let mut action = None;
                                                handle_context_menu_actions_tags(
                                                    ui,
                                                    i18n,
                                                    &file_item,
                                                    &mut action,
                                                    palette,
                                                );
                                                if let Some(a) = action {
                                                    tags_state.pending_action = Some(a);
                                                }
                                            });
                                    }

                                    if let Some(drag_source_index) = drag_source_index {
                                        if let Some(drop) = drop_index {
                                            if drop < item_rects.len() {
                                                let rect = item_rects[drop];
                                                draw_insert_line(
                                                    ui,
                                                    palette,
                                                    rect.top(),
                                                    rect.left(),
                                                    rect.right(),
                                                );
                                            } else if let Some(last) = item_rects.last().copied() {
                                                draw_insert_line(
                                                    ui,
                                                    palette,
                                                    last.bottom(),
                                                    last.left(),
                                                    last.right(),
                                                );
                                            }
                                        }

                                        if pointer_released {
                                            if let Some(drop) = drop_index {
                                                if drag_source_index < group.items.len()
                                                    && drag_source_index != drop
                                                {
                                                    let item = group.items.remove(drag_source_index);
                                                    let mut target = drop;

                                                    if drop > drag_source_index {
                                                        target -= 1;
                                                    }

                                                    target = target.min(group.items.len());
                                                    group.items.insert(target, item);
                                                    changed = true;
                                                }
                                            }

                                            should_clear_drag = true;
                                        }

                                        if let Some(label_path) = group_items.get(drag_source_index) {
                                            draw_object_drag_ghost(
                                                ui,
                                                palette,
                                                &tag_item_label(label_path),
                                                true,
                                            );
                                        }
                                    }
                                });
                            });

                            if clear_rename_state
                                && rename_state
                                    .as_ref()
                                    .map(|state| state.group_id == group_id)
                                    .unwrap_or(false)
                            {
                                rename_state = None;
                            }
                            header_inner.response.rect
                        });

                        if focus_this_group {
                            group_response.response.scroll_to_me(Some(egui::Align::TOP));
                            ui.ctx().request_repaint();
                        }
                        if let Some(alpha) = highlight_alpha {
                            // Brief outline around the header row that fades out,
                            // so the eye finds the group that was just focused.
                            let header_rect = group_response
                                .inner
                                .expand2(egui::vec2(6.0, 3.0))
                                .intersect(group_response.response.rect);
                            ui.painter().rect_stroke(
                                header_rect,
                                egui::CornerRadius::same(palette.medium_radius),
                                egui::Stroke::new(
                                    2.0,
                                    palette.primary_active.gamma_multiply(alpha.clamp(0.0, 1.0)),
                                ),
                                egui::StrokeKind::Inside,
                            );
                            ui.ctx().request_repaint();
                        }

                        if should_clear_drag {
                            drag_state = None;
                        }

                        if group_index + 1 < groups_len {
                            ui.add_space(-12.0); // remove vertical margins between tag groups
                        }
                    }
                });
            });
            });

    if pointer_released && drag_state.is_some() {
        drag_state = None;
    }

    tags_state.drag_state = drag_state;
    tags_state.rename_state = rename_state;
    tags_state.delete_confirmation = delete_confirmation;
    TagsPanelResponse {
        changed,
        close_requested,
    }
}

/// Slim header of the docked tags panel: tag icon + title on the left, close
/// button on the right. Returns whether the close button was clicked.
fn draw_dock_header(ui: &mut egui::Ui, i18n: &I18n, palette: &ThemePalette, focused: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TAGS_DOCK_HEADER_HEIGHT),
        egui::Sense::hover(),
    );
    if focused {
        // Same accent as a focused split pane.
        ui.painter().hline(
            rect.x_range(),
            rect.top() + 1.0,
            egui::Stroke::new(2.0, palette.borders_active),
        );
    }

    let mut close = false;
    let mut header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("tags_dock_header")
            .max_rect(rect.shrink2(egui::vec2(10.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    header_ui.label(
        egui::RichText::new(regular::TAG)
            .size(palette.text_size + 2.0)
            .color(palette.icon_color),
    );
    header_ui.label(
        egui::RichText::new(i18n.tr("tags"))
            .size(palette.text_size)
            .color(palette.text_header_section)
            .strong(),
    );
    header_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if clickable_icon(ui, regular::X, palette)
            .on_hover_text(
                egui::RichText::new(i18n.tr("tooltip_hide_tags_panel"))
                    .size(palette.tooltip_text_size)
                    .color(palette.tooltip_text_color),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            close = true;
        }
    });
    close
}

pub fn draw_delete_confirmation_popup(
    ctx: &egui::Context,
    i18n: &I18n,
    palette: &ThemePalette,
    tags_state: &mut TagsState,
) -> bool {
    let Some(group_id) = tags_state.delete_confirmation else {
        return false;
    };

    let mut changed = false;
    let mut close_requested = false;
    let mut confirmed = false;

    let group_name = tags_state
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    egui::Area::new(egui::Id::new("tag_delete_modal_bg"))
        .order(egui::Order::Middle)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = ctx.content_rect();
            ui.painter()
                .rect_filled(rect, 0.0, palette.modal_background_effect_color);
        });

    egui::Window::new(i18n.tr("tag_delete_group"))
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::popup(&ctx.style_of(ctx.theme()))
                .corner_radius(egui::CornerRadius::same(8)),
        )
        .show(ctx, |ui| {
            let mut style = (**ui.style()).clone();
            style.text_styles = [
                (egui::TextStyle::Heading, egui::FontId::proportional(14.0)),
                (
                    egui::TextStyle::Body,
                    egui::FontId::proportional(palette.text_size),
                ),
                (
                    egui::TextStyle::Button,
                    egui::FontId::proportional(palette.text_size),
                ),
                (
                    egui::TextStyle::Small,
                    egui::FontId::proportional(palette.text_size),
                ),
            ]
            .into();
            ui.set_style(style);

            ui.label(egui::RichText::new(i18n.tr("tag_delete_confirm")).strong());
            ui.add_space(8.0);

            ui.label(
                egui::RichText::new(format!("{}: \"{}\"", i18n.tr("tag_group_name"), group_name))
                    .color(ui.visuals().text_color().linear_multiply(0.8)),
            );

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button(i18n.tr("ok")).clicked() {
                    confirmed = true;
                    close_requested = true;
                }

                if ui.button(i18n.tr("close")).clicked() {
                    close_requested = true;
                }
            });
        });

    if close_requested {
        if confirmed {
            tags_state.groups.retain(|g| g.id != group_id);
            changed = true;
        }
        tags_state.delete_confirmation = None;
    } else {
        tags_state.delete_confirmation = Some(group_id);
    }

    changed
}

fn tag_item_layout(ui: &mut egui::Ui) -> (egui::Rect, egui::Response) {
    ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 18.0),
        egui::Sense::click_and_drag(),
    )
}

pub fn draw_tag_picker_popup(
    ctx: &egui::Context,
    i18n: &I18n,
    palette: &ThemePalette,
    tags_state: &mut TagsState,
) -> bool {
    let Some(mut picker) = tags_state.picker.take() else {
        return false;
    };

    let mut changed = false;
    let mut close_requested = false;

    egui::Area::new(egui::Id::new("tag_modal_bg"))
        .order(egui::Order::Middle)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = ctx.content_rect();
            ui.painter()
                .rect_filled(rect, 0.0, palette.modal_background_effect_color);
        });

    egui::Window::new(i18n.tr("tag_manage"))
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::popup(&ctx.style_of(ctx.theme()))
                .corner_radius(egui::CornerRadius::same(8)),
        )
        .show(ctx, |ui| {
            let mut style = (**ui.style()).clone();
            style.text_styles = [
                (egui::TextStyle::Heading, egui::FontId::proportional(14.0)),
                (
                    egui::TextStyle::Body,
                    egui::FontId::proportional(palette.text_size),
                ),
                (
                    egui::TextStyle::Button,
                    egui::FontId::proportional(palette.text_size),
                ),
                (
                    egui::TextStyle::Small,
                    egui::FontId::proportional(palette.text_size),
                ),
            ]
            .into();
            ui.set_style(style);

            ui.label(egui::RichText::new(i18n.tr("tag_add_to_existing_group")).strong());
            ui.add_space(6.0);

            let all_tags: Vec<crate::gui::windows::containers::structs::TagInfo> =
                tags_state.tags_for_paths(&picker.paths);
            let group_choices: Vec<(u64, String, egui::Color32, usize, bool)> = tags_state
                .groups
                .iter()
                .map(|group| {
                    let is_member = all_tags.iter().any(|t| t.name == group.name);
                    (
                        group.id,
                        group.name.clone(),
                        group.color,
                        group.items.len(),
                        is_member,
                    )
                })
                .collect();

            if group_choices.is_empty() {
                ui.label(
                    egui::RichText::new(i18n.tr("tag_no_groups"))
                        .color(ui.visuals().text_color().linear_multiply(0.8)),
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);

                    for (group_id, group_name, group_color, item_count, is_member) in &group_choices
                    {
                        let prefix = if *is_member { "✓ " } else { "" };
                        let button_label = format!("{}{} ({})", prefix, group_name, item_count);
                        let button = egui::Button::new(button_label)
                            .fill(group_color.gamma_multiply(if *is_member { 0.45 } else { 0.25 }))
                            .stroke(egui::Stroke::new(1.0, group_color.gamma_multiply(0.6)));

                        if ui
                            .add(button)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            if *is_member {
                                if tags_state.remove_paths_from_group(*group_id, &picker.paths) {
                                    changed = true;
                                }
                            } else {
                                if tags_state.add_paths_to_group(*group_id, &picker.paths) {
                                    changed = true;
                                }
                            }
                        }
                    }
                });
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.collapsing(i18n.tr("tag_create_new_group"), |ui| {
                ui.horizontal(|ui| {
                    let name_id = ui.id().with("tag_group_name_input");
                    let name_response = ui.add(
                        egui::TextEdit::singleline(&mut picker.new_group_name)
                            .id(name_id)
                            .hint_text(i18n.tr("tag_group_name"))
                            .desired_width(200.0)
                            .font(egui::FontId::new(
                                palette.text_size,
                                egui::FontFamily::Proportional,
                            )),
                    );

                    if picker.focus_requested {
                        ui.memory_mut(|mem| mem.request_focus(name_id));
                        name_response.request_focus();
                        if name_response.has_focus() {
                            picker.focus_requested = false;
                        }
                    }

                    if rgba_color_edit_button(ui, &mut picker.new_group_color)
                        .on_hover_text(i18n.tr("tag_group_color"))
                        .changed()
                    {
                        changed = true;
                    }
                });

                ui.add_space(6.0);

                if ui
                    .add_enabled(
                        !picker.new_group_name.trim().is_empty(),
                        egui::Button::new(i18n.tr("tag_create_group")),
                    )
                    .clicked()
                {
                    if tags_state.create_group_and_add(
                        picker.new_group_name.clone(),
                        picker.new_group_color,
                        &picker.paths,
                    ) {
                        changed = true;
                    }
                    close_requested = true;
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(i18n.tr("close")).clicked() {
                    close_requested = true;
                }
            });
        });

    if !close_requested {
        tags_state.picker = Some(picker);
    } else {
        changed = true;
    }

    changed
}

fn tag_item_label(path: &PathBuf) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

fn compute_drop_index(
    item_rects: &[egui::Rect],
    pointer_y: f32,
    source_index: usize,
) -> Option<usize> {
    if item_rects.is_empty() {
        return Some(0);
    }

    let mut drop_index: Option<usize> = None;

    for (index, rect) in item_rects.iter().enumerate() {
        let midpoint = rect.center().y;
        let new_index = if pointer_y < midpoint {
            index
        } else {
            index + 1
        };

        if new_index != source_index && new_index != source_index + 1 {
            drop_index = Some(new_index);
        }

        if pointer_y < rect.bottom() {
            break;
        }
    }

    if let Some(last) = item_rects.last() {
        if pointer_y > last.bottom() {
            drop_index = Some(item_rects.len());
        }
    }

    drop_index
}

fn draw_insert_line(ui: &mut egui::Ui, palette: &ThemePalette, y: f32, left: f32, right: f32) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("tag_insert_line"),
    ));

    painter.line_segment(
        [egui::pos2(left + 6.0, y), egui::pos2(right - 6.0, y)],
        egui::Stroke::new(2.0, palette.primary_active),
    );
}

fn handle_context_menu_actions_tags(
    ui: &mut egui::Ui,
    i18n: &I18n,
    file: &FileItem,
    action: &mut Option<ItemViewerAction>,
    _palette: &ThemePalette,
) {
    // Apply context-menu-specific typography
    let mut style = (**ui.style()).clone();
    style.text_styles = [
        (
            egui::TextStyle::Body,
            FontId::proportional(_palette.text_size),
        ),
        (
            egui::TextStyle::Button,
            FontId::proportional(_palette.text_size),
        ),
        (
            egui::TextStyle::Small,
            FontId::proportional(_palette.text_size),
        ),
        (
            egui::TextStyle::Heading,
            FontId::proportional(_palette.text_size + 2.0),
        ),
    ]
    .into();
    style.spacing.button_padding = egui::vec2(4.0, 2.0);
    style.spacing.item_spacing = egui::vec2(6.0, 2.0);
    style.spacing.menu_margin = egui::Margin::same(4);
    style.spacing.interact_size =
        egui::vec2(style.spacing.interact_size.x, _palette.text_size + 6.0);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.hovered.bg_fill = _palette.primary;
    style.visuals.widgets.hovered.weak_bg_fill = _palette.primary;
    style.visuals.widgets.active.bg_fill = _palette.primary;
    style.visuals.widgets.active.weak_bg_fill = _palette.primary;
    ui.set_style(style);

    let context_paths = vec![file.path.clone()];

    // --- NORMAL FILE VIEW ---
    // Determine button label based on selection count
    let label = i18n.tr("open_default_program");

    // Check if all selected files are not directories
    let all_files = context_paths.iter().all(|path| !path.is_dir());

    // Add the button with dynamic label
    if ui
        .add_enabled(all_files, egui::Button::new(label))
        .clicked()
    {
        *action = Some(ItemViewerAction::OpenWithDefault(context_paths.clone()));
        ui.close();
    }

    if ui.button(i18n.tr("inputs_copy_path")).clicked() {
        *action = Some(ItemViewerAction::Context(
            ItemViewerContextAction::CopyPath(context_paths.clone()),
        ));
        ui.close();
    }

    // Properties (multi-select aware)
    if ui.button(i18n.tr("properties")).clicked() {
        *action = Some(ItemViewerAction::Context(
            ItemViewerContextAction::Properties(context_paths.clone()),
        ));
        ui.close();
    }

    if ui.button(i18n.tr("tag_manage")).clicked() {
        *action = Some(ItemViewerAction::Context(
            ItemViewerContextAction::ManageTags(context_paths.clone()),
        ));
        ui.close();
    }
}

fn handle_row_click(object: &FileItem) -> Option<ItemViewerAction> {
    return Some(if object.is_dir {
        ItemViewerAction::Open(object.path.clone())
    } else {
        ItemViewerAction::OpenWithDefault(vec![object.path.clone()])
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(height: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 40.0), egui::vec2(800.0, height))
    }

    #[test]
    fn dock_layout_uses_preferred_height_when_it_fits() {
        let layout = layout_tags_dock(area(700.0), 240.0, 1.0).unwrap();
        assert_eq!(layout.height, 240.0);
        assert_eq!(layout.panel.height(), 240.0);
        assert_eq!(layout.panel.bottom(), 740.0);
        assert_eq!(layout.separator.height(), TAGS_DOCK_SEPARATOR_HEIGHT);
        assert_eq!(layout.separator.bottom(), layout.panel.top());
        assert_eq!(layout.content.bottom(), layout.separator.top());
        assert_eq!(layout.content.left(), 10.0);
        assert_eq!(layout.panel.left(), 10.0 + TAGS_DOCK_LEFT_INSET);
        assert_eq!(layout.max_height, 700.0 - 150.0 - 6.0);
    }

    #[test]
    fn dock_layout_keeps_content_minimum_and_shrinks_dock_first() {
        // Preference larger than the window allows: the content keeps 150.
        let layout = layout_tags_dock(area(500.0), 1000.0, 1.0).unwrap();
        assert_eq!(layout.content.height(), TAGS_DOCK_CONTENT_MIN_HEIGHT);
        assert_eq!(layout.height, 500.0 - 150.0 - 6.0);

        // Preference below the minimum is raised to it.
        let layout = layout_tags_dock(area(700.0), 20.0, 1.0).unwrap();
        assert_eq!(layout.height, TAGS_DOCK_MIN_HEIGHT);

        // Small window: the dock goes below its own minimum before the content
        // loses space...
        let layout = layout_tags_dock(area(220.0), 240.0, 1.0).unwrap();
        assert_eq!(layout.height, 220.0 - 150.0 - 6.0);
        assert_eq!(layout.content.height(), TAGS_DOCK_CONTENT_MIN_HEIGHT);

        // ...and disappears when it would be unusably small.
        assert!(layout_tags_dock(area(180.0), 240.0, 1.0).is_none());
        assert!(layout_tags_dock(area(0.0), 240.0, 1.0).is_none());
    }

    #[test]
    fn dock_layout_rejects_non_finite_preference() {
        let layout = layout_tags_dock(area(700.0), f32::NAN, 1.0).unwrap();
        assert_eq!(layout.height, TAGS_DOCK_DEFAULT_HEIGHT);
    }

    #[test]
    fn dock_layout_snaps_panel_top_to_pixels() {
        let layout = layout_tags_dock(area(700.3), 240.0, 1.5).unwrap();
        let top_px = layout.panel.top() * 1.5;
        assert!((top_px - top_px.round()).abs() < 1e-3);
    }
}
