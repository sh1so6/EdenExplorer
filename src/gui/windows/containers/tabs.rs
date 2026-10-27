use crate::core::fs::{MY_PC_PATH, MY_RECYCLE_BIN_PATH};
use crate::core::utils::text::apply_eden_text_overrides;
use crate::core::utils::widgets::apply_eden_visual_overrides;
use crate::gui::i18n::I18n;
use crate::gui::theme::ThemePalette;
use crate::gui::utils::truncate_item_text;
use crate::gui::windows::containers::structs::{TabContextAction, TabInfo, TabsAction};
use crate::gui::windows::windowsoverrides::toggle_window_fullscreen;
use eframe::egui;
use egui::{FontFamily, FontId};
use egui_phosphor::{fill, regular};
use std::path::PathBuf;
use windows::Win32::Foundation::HWND;

const PREFERRED_TAB_WIDTH: f32 = 180.0;
const MIN_TAB_WIDTH: f32 = 80.0;
const ADD_TAB_WIDTH: f32 = 28.0;
const TAB_HEIGHT: f32 = 32.0;
const TAB_NAV_BUTTON_WIDTH: f32 = 28.0;

pub const VERTICAL_TABS_MIN_WIDTH: f32 = 120.0;
pub const VERTICAL_TABS_MAX_WIDTH: f32 = 400.0;
pub const VERTICAL_TABS_DEFAULT_WIDTH: f32 = 200.0;
/// Row height of the vertical tab rail (Settings > General), stored in `layout.json`.
pub const VERTICAL_TAB_ROW_HEIGHT_MIN: f32 = 20.0;
pub const VERTICAL_TAB_ROW_HEIGHT_MAX: f32 = 40.0;
pub const VERTICAL_TAB_ROW_HEIGHT_DEFAULT: f32 = 24.0;
/// Space kept around the taller of the title text and the tab icon, so a theme with a
/// larger text size raises the effective minimum row height instead of clipping text.
const VERTICAL_TAB_ROW_MIN_VERTICAL_MARGIN: f32 = 8.0;
const VERTICAL_TABS_HEADER_HEIGHT: f32 = 20.0;
/// Gap between the rail's top/bottom edges and its rows/button; inactive rows and the
/// button also keep it clear of the right divider.
const VERTICAL_TABS_INSET: f32 = 6.0;
/// Gap between the rail's left edge (the sidebar border / resize handle) and the rows.
const VERTICAL_TABS_LEFT_INSET: f32 = 3.0;
/// Horizontal padding inside a row before the icon (leaves room for the active accent bar).
const VERTICAL_TAB_ROW_PADDING: f32 = 10.0;
/// How far the rail background moves from the app background towards the text color.
const VERTICAL_TABS_RAIL_TINT: f32 = 0.04;
/// Keeps the floating scroll bar (and its hit area) off the right divider, whose resize
/// handle reaches 3px into the rail, and away from the active row's opening.
const VERTICAL_TABS_SCROLLBAR_MARGIN: f32 = 4.0;
/// Opacity of the row being dragged to a new position in the vertical rail.
const VERTICAL_TAB_REORDER_SOURCE_OPACITY: f32 = 0.4;
/// Thickness of the insertion line shown while reordering vertical tabs.
const VERTICAL_TAB_REORDER_LINE_WIDTH: f32 = 2.0;
/// Fastest auto-scroll of the rail while a dragged tab sits at (or past) its top or bottom
/// edge, in points per second.
const VERTICAL_TAB_REORDER_SCROLL_SPEED: f32 = 600.0;

/// Egui temp-memory key holding the id of the tab being dragged to a new position in the
/// vertical rail. Absent when no reorder drag is in progress.
fn vertical_tab_reorder_key() -> egui::Id {
    egui::Id::new("eden_vertical_tab_reorder_drag")
}

/// Egui temp-memory key holding the not yet used part of the mouse-wheel input over the
/// vertical rail (in tab steps, signed like the wheel delta). Absent when nothing is pending.
fn vertical_tab_wheel_key() -> egui::Id {
    egui::Id::new("eden_vertical_tab_wheel_accum")
}

/// Adds one wheel event's vertical `delta` to the pending `accum` (both in tab steps, positive
/// = wheel up) and returns the whole steps to take now plus the remainder to keep.
///
/// A line or a page is one step; touchpad points count one step per `row_height`. Turning
/// the wheel the other way drops the remainder of the previous direction.
fn wheel_steps(accum: f32, delta: f32, unit: egui::MouseWheelUnit, row_height: f32) -> (i32, f32) {
    let amount = match unit {
        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta,
        egui::MouseWheelUnit::Point => delta / row_height.max(1.0),
    };
    if !amount.is_finite() || amount == 0.0 {
        return (0, accum);
    }
    let accum = if accum * amount < 0.0 { 0.0 } else { accum };
    let total = accum + amount;
    let steps = total.trunc();
    (steps as i32, total - steps)
}

/// Index of the tab a wheel turn of `steps` (positive = wheel up = towards the first tab)
/// activates, starting at `active_idx` and clamped to `0..len` (no wrap-around).
fn wheel_target_index(active_idx: usize, steps: i32, len: usize) -> usize {
    let last = len.saturating_sub(1) as i64;
    (active_idx as i64 - steps as i64).clamp(0, last) as usize
}

/// Insertion slot (`0..=row_rects.len()`, in the current tab order) for a dragged tab whose
/// pointer is at `pointer_y`: before the first row whose vertical midpoint lies below it.
fn reorder_insert_slot(pointer_y: f32, row_rects: &[egui::Rect]) -> usize {
    row_rects
        .iter()
        .position(|rect| pointer_y < rect.center().y)
        .unwrap_or(row_rects.len())
}

/// Final index of an item moved from `from` into insertion slot `to` (`0..=len`, counted in
/// the order before the move). `None` when the move would not change the order (dropping
/// right above or right below itself) or the indices are out of range.
pub fn reorder_index(from: usize, to: usize, len: usize) -> Option<usize> {
    if from >= len || to > len || to == from || to == from + 1 {
        return None;
    }
    Some(if to > from { to - 1 } else { to })
}

/// The pinned paths reordered to follow the tab order: the slots of pinned paths that are
/// open in `tab_paths` are refilled with those paths in tab order, pinned paths without an
/// open tab keep their slots. `None` when the order would not change (or the pinned list
/// holds duplicates of an open path, which has no well-defined order).
pub fn pinned_paths_in_tab_order(
    pinned: &[PathBuf],
    tab_paths: &[&std::path::Path],
) -> Option<Vec<PathBuf>> {
    let mut open_in_tab_order: Vec<&std::path::Path> = Vec::new();
    for path in tab_paths {
        if pinned.iter().any(|p| p == path) && !open_in_tab_order.contains(path) {
            open_in_tab_order.push(path);
        }
    }
    let open_slots = pinned
        .iter()
        .filter(|p| open_in_tab_order.contains(&p.as_path()))
        .count();
    if open_slots != open_in_tab_order.len() {
        return None;
    }
    let mut next_open = open_in_tab_order.into_iter();
    let reordered: Vec<PathBuf> = pinned
        .iter()
        .map(|p| {
            if tab_paths.contains(&p.as_path()) {
                next_open
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| p.clone())
            } else {
                p.clone()
            }
        })
        .collect();
    (reordered != pinned).then_some(reordered)
}

/// Pointer/drag state shared by the horizontal and vertical tab strips to decide
/// whether dragged files are hovering a tab and should be dropped into its folder.
struct TabDropContext<'a> {
    drag_active: bool,
    hovered_target: Option<&'a PathBuf>,
    pointer_pos: Option<egui::Pos2>,
    pointer_released: bool,
}

impl<'a> TabDropContext<'a> {
    fn new(ui: &egui::Ui, drag_active: bool, hovered_target: Option<&'a PathBuf>) -> Self {
        let pointer_pos = ui.input(|i| i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()));
        let pointer_released =
            ui.input(|i| i.pointer.any_released() && i.pointer.interact_pos().is_some());
        Self {
            drag_active,
            hovered_target,
            pointer_pos,
            pointer_released,
        }
    }

    /// Returns true when dragged files hover `tab` (drawn in `rect`). The active tab is
    /// never a drop target because its folder is already the one being shown.
    fn is_drop_hovered(&self, tab: &TabInfo, rect: egui::Rect, active_id: u64) -> bool {
        if !self.drag_active || tab.id == active_id {
            return false;
        }
        self.hovered_target
            .map(|target| target == &tab.full_path)
            .unwrap_or_else(|| {
                self.pointer_pos
                    .map(|pointer| rect.contains(pointer))
                    .unwrap_or(false)
            })
    }
}

/// Glyph for the tab's leading icon, which doubles as the pin toggle (hover previews the
/// state a click would switch to).
fn tab_icon_glyph(is_pinned: bool, hovered: bool) -> &'static str {
    match (is_pinned, hovered) {
        (true, true) => regular::FOLDER_SIMPLE,
        (true, false) => fill::PUSH_PIN,
        (false, true) => regular::PUSH_PIN,
        (false, false) => regular::FOLDER_SIMPLE,
    }
}

fn tab_pin_tooltip(i18n: &I18n, is_pinned: bool) -> String {
    if is_pinned {
        i18n.tr("tooltip_tab_unpin")
    } else {
        i18n.tr("tooltip_tab_pin")
    }
}

/// Tooltip for a tab: the full title when it is truncated, otherwise its location.
fn tab_hover_tooltip_text(i18n: &I18n, tab: &TabInfo, truncated: bool) -> String {
    if truncated {
        return tab.title.clone();
    }
    let path = tab.full_path.to_string_lossy().to_string();
    if path == MY_PC_PATH {
        i18n.tr("thispc")
    } else if path == MY_RECYCLE_BIN_PATH {
        i18n.tr("recycle_bin")
    } else {
        path
    }
}

/// Makes `rect` behave like a native title bar: drag moves the window, double-click
/// toggles maximized state.
pub fn handle_window_drag_area(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    hwnd: Option<HWND>,
) {
    let resp = ui.interact(rect, id, egui::Sense::click_and_drag());

    if crate::gui::windows::double_click::is_double_click(&resp) {
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

pub fn draw_tabs(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tabs: &[TabInfo],
    tab_scroll_offset: f32,
    active_id: u64,
    palette: &ThemePalette,
    hwnd: Option<HWND>,
    scroll_to_id: Option<u64>,
    drag_active: bool,
    drag_hover_target: Option<PathBuf>,
) -> TabsAction {
    let mut action: TabsAction = TabsAction::default();
    let drop_ctx = TabDropContext::new(ui, drag_active, drag_hover_target.as_ref());
    let mut tab_drop_target: Option<PathBuf> = None;
    let windows_buttons_width = 45.0 * 3.0;
    let split_button_width = 32.0;
    let controls_width = windows_buttons_width + split_button_width;
    let full_width = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;
    // Reserve the + button outside the tab viewport.
    let tab_count = tabs.len() as f32;
    let gaps = (tab_count - 1.0).max(0.0);
    // Space reserved for the entire tab region.
    // This includes the tabs and, when needed, the left/right navigation buttons.
    // The + button and window controls remain outside this region.
    let tab_region_width = (full_width - controls_width - ADD_TAB_WIDTH - spacing).max(0.0);

    // Determine how wide the tabs can be when there are no navigation buttons.
    let natural_tab_width = if tab_count > 0.0 {
        ((tab_region_width - gaps * spacing) / tab_count).max(0.0)
    } else {
        PREFERRED_TAB_WIDTH
    };

    let tabs_need_scroll = tab_count > 0.0 && natural_tab_width < MIN_TAB_WIDTH;

    let tab_width = if tabs_need_scroll {
        MIN_TAB_WIDTH
    } else {
        natural_tab_width.min(PREFERRED_TAB_WIDTH)
    };

    let tabs_content_width = tab_count * tab_width + gaps * spacing;

    // The navigation buttons live INSIDE the fixed tab region.
    let tab_view_width = if tabs_need_scroll {
        (tab_region_width - TAB_NAV_BUTTON_WIDTH * 2.0).max(0.0)
    } else {
        tabs_content_width
    };

    let max_scroll_offset = (tabs_content_width - tab_view_width).max(0.0);

    let effective_scroll_offset = if tabs_need_scroll {
        tab_scroll_offset.min(max_scroll_offset)
    } else {
        0.0
    };

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 32.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(-2.0);
            // ---------- LEFT NAVIGATION ----------
            if tabs_need_scroll && handle_draw_nav_button(ui, palette, "left") {
                action.scroll_left = true;
            }
            // ---------- TABS ----------
            ui.allocate_ui_with_layout(
                egui::vec2(tab_view_width, 32.0),
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    let tabs_rect = ui.available_rect_before_wrap();

                    let pointer_over_tabs = ui
                        .input(|i| i.pointer.hover_pos())
                        .is_some_and(|pos| tabs_rect.contains(pos));

                    if pointer_over_tabs && tabs_need_scroll {
                        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);

                        if scroll_delta > 0.0 {
                            action.scroll_left = true;
                        } else if scroll_delta < 0.0 {
                            action.scroll_right = true;
                        }
                    }
                    egui::ScrollArea::horizontal()
                        .id_salt("tabs_scroll")
                        .auto_shrink([false, true])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .scroll_offset(egui::vec2(effective_scroll_offset, 0.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (index, tab) in tabs.iter().enumerate() {
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(tab_width, TAB_HEIGHT),
                                        egui::Sense::click(),
                                    );

                                    if Some(tab.id) == scroll_to_id {
                                        resp.scroll_to_me(Some(egui::Align::Center));
                                    }

                                    if drop_ctx.is_drop_hovered(tab, rect, active_id) {
                                        let painter = ui
                                            .ctx()
                                            .layer_painter(egui::LayerId::new(
                                                egui::Order::Background,
                                                ui.id().with("tab_drop_bg").with(tab.id),
                                            ))
                                            .with_clip_rect(ui.clip_rect());

                                        painter.rect_filled(
                                            rect,
                                            egui::CornerRadius::same(palette.medium_radius),
                                            palette.primary_hover,
                                        );

                                        if drop_ctx.pointer_released {
                                            tab_drop_target = Some(tab.full_path.clone());
                                        }
                                    }

                                    handle_draw_tab_new_allocated(
                                        ui,
                                        i18n,
                                        tabs,
                                        index,
                                        rect,
                                        resp.clone(),
                                        active_id,
                                        palette,
                                        &mut action,
                                    );
                                }
                            });
                        });
                },
            );

            // ---------- RIGHT NAVIGATION ----------
            if tabs_need_scroll && handle_draw_nav_button(ui, palette, "right") {
                action.scroll_right = true;
            }

            // ---------- ADD NEW TAB BUTTON ----------
            handle_draw_add_new_tab_button(ui, palette, &mut action);

            // ---------- WINDOWS DRAG AREA ----------
            let drag_width = ui.available_width();

            if drag_width > 4.0 {
                let drag_rect = ui.allocate_space(egui::vec2(drag_width, 32.0)).1;
                let drag_id = ui.id().with("tabs_window_drag_area");
                handle_window_drag_area(ui, drag_rect, drag_id, hwnd);
            }
        },
    );
    action.move_files_to_tab_dir = tab_drop_target;
    action.max_scroll_offset = max_scroll_offset;
    action
}

/// Background of the vertical tab rail: the app background nudged slightly towards the text
/// color, so the rail reads as its own layer next to the sidebar and content in both themes.
/// The row height actually drawn: the user's setting within its bounds, but never less
/// than the title text / tab icon plus a small margin.
fn vertical_tab_row_height(setting: f32, palette: &ThemePalette) -> f32 {
    let setting = if setting.is_finite() {
        setting
    } else {
        VERTICAL_TAB_ROW_HEIGHT_DEFAULT
    };
    let content_min =
        palette.text_size.max(palette.tab_icon_size) + VERTICAL_TAB_ROW_MIN_VERTICAL_MARGIN;
    setting
        .clamp(VERTICAL_TAB_ROW_HEIGHT_MIN, VERTICAL_TAB_ROW_HEIGHT_MAX)
        .max(content_min)
        .round()
}

fn vertical_tabs_rail_fill(palette: &ThemePalette) -> egui::Color32 {
    lerp_opaque(
        palette.application_bg_color,
        palette.text_normal,
        VERTICAL_TABS_RAIL_TINT,
    )
}

/// Blends two colors channel-wise in gamma space and returns an opaque result
/// (`t = 0` gives `from`, `t = 1` gives `to`).
fn lerp_opaque(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    egui::Color32::from_rgb(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
    )
}

/// 1px `borders_default` stroke, pre-composited onto the app background.
///
/// `borders_default` is translucent, so drawing it over the tinted rail (or twice where edges
/// meet) would change its shade. The opaque equivalent matches the sidebar border exactly.
fn vertical_tabs_border_stroke(palette: &ThemePalette) -> egui::Stroke {
    let fg = palette.borders_default; // premultiplied
    let bg = palette.application_bg_color;
    let keep = 1.0 - fg.a() as f32 / 255.0;
    let over = |f: u8, b: u8| (f as f32 + b as f32 * keep).round().min(255.0) as u8;
    egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(
            over(fg.r(), bg.r()),
            over(fg.g(), bg.g()),
            over(fg.b(), bg.b()),
        ),
    )
}

/// Draws the tab list as a vertical rail that fills `ui.max_rect()`.
///
/// The rail has a tinted background, a top edge and a divider on its right towards the
/// content. The divider is left open where the active row is, so the active tab visually
/// connects to the content it owns. `draw_left_border` adds a left edge; it is off when the
/// rail sits directly against the sidebar border.
///
/// The mouse wheel over the rail switches tabs (down = next, up = previous, no wrap-around)
/// instead of scrolling the list; the list follows the active tab and keeps a draggable
/// scroll bar. Ctrl / Shift wheel and wheel input during a drag are left alone.
///
/// Returns the same `TabsAction` as the horizontal strip (minus scrolling, which the
/// panel's own `ScrollArea` handles) so both modes share `handle_tabs_action`.
pub fn draw_vertical_tabs(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tabs: &[TabInfo],
    active_id: u64,
    palette: &ThemePalette,
    scroll_to_id: Option<u64>,
    drag_active: bool,
    drag_hover_target: Option<PathBuf>,
    draw_left_border: bool,
    row_height: f32,
) -> TabsAction {
    let mut action = TabsAction::default();

    // Tab reordering: the dragged tab's id lives in temp memory from the drag start until
    // release or Escape.
    let reorder_key = vertical_tab_reorder_key();
    let mut reorder_drag: Option<u64> = ui.data(|d| d.get_temp(reorder_key));
    if let Some(id) = reorder_drag {
        let cancelled = !tabs.iter().any(|tab| tab.id == id)
            || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cancelled {
            reorder_drag = None;
            ui.data_mut(|d| d.remove::<u64>(reorder_key));
            ui.ctx().stop_dragging();
        }
    }
    let mut reorder_released = false;
    // Only filled while reordering (allocation-free otherwise).
    let mut row_rects: Vec<egui::Rect> = Vec::new();

    // A tab reorder and a file drag never overlap: no drop highlight while reordering.
    let drop_ctx = TabDropContext::new(
        ui,
        drag_active && reorder_drag.is_none(),
        drag_hover_target.as_ref(),
    );
    let row_height = vertical_tab_row_height(row_height, palette);
    let mut tab_drop_target: Option<PathBuf> = None;

    let panel_rect = ui.max_rect();
    let border = vertical_tabs_border_stroke(palette);
    let rail_fill = vertical_tabs_rail_fill(palette);
    let painter = ui.painter().clone();
    painter.rect_filled(panel_rect, 0.0, rail_fill);
    painter.hline(panel_rect.x_range(), panel_rect.top() + 0.5, border);
    let inner_left = if draw_left_border {
        painter.vline(panel_rect.left() + 0.5, panel_rect.y_range(), border);
        panel_rect.left() + 1.0
    } else {
        panel_rect.left()
    };

    // Rows run all the way to the right edge (the divider); only the active row uses that
    // last stretch, inactive rows keep `VERTICAL_TABS_INSET` clear of it.
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(
            inner_left + VERTICAL_TABS_LEFT_INSET,
            panel_rect.top() + 1.0 + VERTICAL_TABS_INSET,
        ),
        panel_rect.max - egui::vec2(0.0, VERTICAL_TABS_INSET),
    );
    let mut active_row_rect: Option<egui::Rect> = None;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min))
            // Lets the ScrollArea's edge fades blend into the rail instead of the app bg.
            .ui_stack_info(
                egui::UiStackInfo::new(egui::UiKind::Frame)
                    .with_frame(egui::Frame::NONE.fill(rail_fill)),
            ),
        |ui| {
            // Same gap below the new tab strip as between the rows (the theme's
            // 8 px item spacing plus an inset made a 22 px hole here).
            ui.spacing_mut().item_spacing.y = 2.0;
            handle_draw_vertical_new_tab_button(ui, i18n, palette, &mut action);

            ui.spacing_mut().scroll.bar_outer_margin = VERTICAL_TABS_SCROLLBAR_MARGIN;
            egui::ScrollArea::vertical()
                .id_salt("vertical_tabs_scroll")
                .auto_shrink([false, false])
                // Dragging a row reorders tabs and the wheel switches tabs (see
                // `handle_vertical_tab_wheel`); neither may scroll the list.
                .scroll_source(egui::scroll_area::ScrollSource {
                    scroll_bar: true,
                    drag: egui::scroll_area::DragScroll::Never,
                    mouse_wheel: false,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let collect_rects = reorder_drag.is_some();
                    if collect_rects {
                        row_rects.reserve(tabs.len());
                    }
                    for (index, tab) in tabs.iter().enumerate() {
                        // Stable per-tab ids keep the drag (and an open context menu) on
                        // the same tab even if rows shift.
                        let (_, rect) =
                            ui.allocate_space(egui::vec2(ui.available_width(), row_height));
                        let resp = ui.interact(
                            rect,
                            ui.id().with(("vertical_tab_row", tab.id)),
                            egui::Sense::click_and_drag(),
                        );
                        if collect_rects {
                            row_rects.push(rect);
                        }

                        if reorder_drag.is_none()
                            && !drag_active
                            && resp.drag_started_by(egui::PointerButton::Primary)
                        {
                            reorder_drag = Some(tab.id);
                            ui.data_mut(|d| d.insert_temp(reorder_key, tab.id));
                        } else if reorder_drag == Some(tab.id) {
                            if resp.drag_stopped() {
                                reorder_released = true;
                            } else if !resp.dragged() {
                                // The drag ended elsewhere (e.g. `stop_dragging`).
                                reorder_drag = None;
                                ui.data_mut(|d| d.remove::<u64>(reorder_key));
                            }
                        }

                        if Some(tab.id) == scroll_to_id {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }

                        // Rows scrolled out of view must not catch the pointer.
                        let visible_rect = rect.intersect(ui.clip_rect());
                        if tab.id == active_id && visible_rect.is_positive() {
                            active_row_rect = Some(visible_rect);
                        }
                        let drop_hovered = drop_ctx.is_drop_hovered(tab, visible_rect, active_id);
                        if drop_hovered && drop_ctx.pointer_released {
                            tab_drop_target = Some(tab.full_path.clone());
                        }

                        handle_draw_vertical_tab_row(
                            ui,
                            i18n,
                            tabs,
                            index,
                            rect,
                            resp,
                            active_id,
                            drop_hovered,
                            reorder_drag,
                            palette,
                            &mut action,
                        );
                    }

                    if let Some(dragged_id) = reorder_drag
                        && collect_rects
                    {
                        handle_vertical_tab_reorder_drag(
                            ui,
                            dragged_id,
                            &row_rects,
                            panel_rect.x_range(),
                            reorder_released,
                            palette,
                            &mut action,
                        );
                    }
                });
        },
    );

    if action.activate.is_none() {
        let wheel_enabled = reorder_drag.is_none() && !drag_active;
        handle_vertical_tab_wheel(
            ui,
            tabs,
            active_id,
            panel_rect,
            row_height,
            wheel_enabled,
            &mut action,
        );
    }

    if reorder_drag.is_some() {
        if reorder_released {
            ui.data_mut(|d| d.remove::<u64>(reorder_key));
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }

    // Divider towards the content, open where the active row joins the content area.
    let divider_x = panel_rect.right() - 0.5;
    let divider_top = panel_rect.top() + 1.0;
    let divider_bottom = panel_rect.bottom();
    match active_row_rect {
        Some(active) => {
            let gap_top = active.top().round().max(divider_top);
            let gap_bottom = active.bottom().round().min(divider_bottom);
            if gap_top > divider_top {
                painter.vline(divider_x, divider_top..=gap_top, border);
            }
            if gap_bottom < divider_bottom {
                painter.vline(divider_x, gap_bottom..=divider_bottom, border);
            }
        }
        None => {
            painter.vline(divider_x, divider_top..=divider_bottom, border);
        }
    }

    action.move_files_to_tab_dir = tab_drop_target;
    action
}

/// Turns mouse-wheel events over the vertical rail (`panel_rect`) into `action.activate`
/// of the next / previous tab. The raw events are used (not the smoothed scroll delta) so
/// one notch is exactly one tab; partial input carries over in temp memory.
fn handle_vertical_tab_wheel(
    ui: &egui::Ui,
    tabs: &[TabInfo],
    active_id: u64,
    panel_rect: egui::Rect,
    row_height: f32,
    enabled: bool,
    action: &mut TabsAction,
) {
    let wheel_key = vertical_tab_wheel_key();
    // Layer-aware, so a popup or context menu over the rail keeps its wheel input.
    if !enabled || !ui.rect_contains_pointer(panel_rect) {
        ui.data_mut(|d| d.remove::<f32>(wheel_key));
        return;
    }
    let mut accum: f32 = ui.data(|d| d.get_temp(wheel_key)).unwrap_or(0.0);
    let mut steps = 0i32;
    ui.input(|i| {
        for event in &i.events {
            let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            // Ctrl / Cmd wheel zooms and Shift wheel scrolls sideways; leave those alone.
            if modifiers.command || modifiers.ctrl || modifiers.shift {
                continue;
            }
            let (event_steps, rest) = wheel_steps(accum, delta.y, *unit, row_height);
            steps = steps.saturating_add(event_steps);
            accum = rest;
        }
    });
    if accum == 0.0 {
        ui.data_mut(|d| d.remove::<f32>(wheel_key));
    } else {
        ui.data_mut(|d| d.insert_temp(wheel_key, accum));
    }
    if steps == 0 {
        return;
    }
    let Some(active_idx) = tabs.iter().position(|tab| tab.id == active_id) else {
        return;
    };
    let target = &tabs[wheel_target_index(active_idx, steps, tabs.len())];
    if target.id != active_id {
        action.activate = Some(target.id);
    }
}

/// Per-frame work of a vertical tab reorder drag, run inside the rail's `ScrollArea` after
/// all rows are laid out (`row_rects`, one per tab in order): on `released` it emits
/// `action.reorder`, otherwise it draws the insertion line and auto-scrolls near the edges.
///
/// Only the pointer's height picks the slot, clamped to the visible part of the list so a
/// drop never lands among scrolled-out rows. With the pointer left or right of the rail
/// (`rail_x`) there is no insertion line and releasing cancels the move.
fn handle_vertical_tab_reorder_drag(
    ui: &egui::Ui,
    dragged_id: u64,
    row_rects: &[egui::Rect],
    rail_x: egui::Rangef,
    released: bool,
    palette: &ThemePalette,
    action: &mut TabsAction,
) {
    let (Some(first), Some(last)) = (row_rects.first(), row_rects.last()) else {
        return;
    };
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    if !rail_x.contains(pointer.x) {
        return;
    }
    let viewport = ui.clip_rect();
    let slot = reorder_insert_slot(
        pointer.y.clamp(viewport.top(), viewport.bottom()),
        row_rects,
    );
    if released {
        action.reorder = Some((dragged_id, slot));
        return;
    }

    // Insertion line, centered in the gap between the rows around the slot.
    let half_gap = ui.spacing().item_spacing.y * 0.5;
    let line_y = if slot == 0 {
        first.top() - half_gap
    } else if slot == row_rects.len() {
        last.bottom() + half_gap
    } else {
        (row_rects[slot - 1].bottom() + row_rects[slot].top()) * 0.5
    };
    let half_line = VERTICAL_TAB_REORDER_LINE_WIDTH * 0.5;
    let line_y = line_y.clamp(viewport.top() + half_line, viewport.bottom() - half_line);
    let line_rect = egui::Rect::from_x_y_ranges(
        first.left()..=first.right() - VERTICAL_TABS_INSET,
        line_y - half_line..=line_y + half_line,
    );
    ui.painter()
        .rect_filled(line_rect, egui::CornerRadius::same(1), palette.primary);

    // Auto-scroll while the pointer is within one row of (or beyond) the top/bottom edge,
    // faster the closer it gets. Only when there is something left to scroll to, so a list
    // that fits does not repaint continuously.
    let zone = first.height().min(viewport.height() * 0.25).max(1.0);
    let top_depth = ((viewport.top() + zone - pointer.y) / zone).min(1.0);
    let bottom_depth = ((pointer.y - (viewport.bottom() - zone)) / zone).min(1.0);
    let speed = VERTICAL_TAB_REORDER_SCROLL_SPEED * ui.input(|i| i.stable_dt).min(0.1);
    // Positive deltas move the content down, revealing the rows above.
    let delta = if top_depth > 0.0 && first.top() < viewport.top() - 0.5 {
        speed * top_depth
    } else if bottom_depth > 0.0 && last.bottom() > viewport.bottom() + 0.5 {
        -speed * bottom_depth
    } else {
        0.0
    };
    if delta != 0.0 {
        ui.scroll_with_delta_animation(
            egui::vec2(0.0, delta),
            egui::style::ScrollAnimation::none(),
        );
        ui.ctx().request_repaint();
    }
}

fn handle_draw_vertical_new_tab_button(
    ui: &mut egui::Ui,
    i18n: &I18n,
    palette: &ThemePalette,
    action: &mut TabsAction,
) {
    let label = i18n.tr("vertical_tabs_new_tab");
    // Inset from the divider like inactive rows.
    let width = (ui.available_width() - VERTICAL_TABS_INSET).max(0.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(width, VERTICAL_TABS_HEADER_HEIGHT),
        egui::Sense::click(),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));

    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());
    let hovered = resp.hovered();

    // A slim full-width strip with just the plus; the label is the tooltip.
    let icon_font_id = FontId::new(palette.tab_icon_size, FontFamily::Proportional);

    let painter = ui.painter();
    // Flat like the rows below it: only a hover fill, no border.
    if hovered {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(palette.medium_radius),
            ui.visuals().widgets.hovered.bg_fill,
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        regular::PLUS,
        icon_font_id,
        palette.icon_color,
    );

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let resp = resp.on_hover_text(label);
    if resp.clicked() {
        action.open_new = true;
    }
}

/// Draws one row of the vertical tab rail (`tabs[index]`). `rect` spans up to the rail's
/// right divider. Rows have no close button; tabs close via middle-click or the row's
/// context menu.
fn handle_draw_vertical_tab_row(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tabs: &[TabInfo],
    index: usize,
    rect: egui::Rect,
    resp: egui::Response,
    active_id: u64,
    drop_hovered: bool,
    reorder_drag: Option<u64>,
    palette: &ThemePalette,
    action: &mut TabsAction,
) {
    let tab = &tabs[index];
    let is_active = tab.id == active_id;
    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());
    // Inactive rows (and every row's text) stay clear of the divider; only the active row's
    // background runs into it.
    let item_rect = rect.with_max_x(rect.right() - VERTICAL_TABS_INSET);
    // Includes hovering the pin button, which sits on top of the row response. A row whose
    // context menu is open stays highlighted. While a tab is being reordered only the
    // insertion line marks the pointer.
    let row_hovered = reorder_drag.is_none()
        && (ui.rect_contains_pointer(rect)
            || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp)));

    let font_id = FontId::new(palette.text_size, FontFamily::Proportional);
    let icon_font_id = FontId::new(palette.tab_icon_size, FontFamily::Proportional);
    // The active row sits on the content background, not on the accent fill used by the
    // horizontal strip, so it uses regular (slightly stronger) text colors.
    let label_color = if is_active {
        palette.text_header_section
    } else {
        palette.text_normal
    };
    let icon_color = if tab.is_pinned {
        palette.pinned_tab_color
    } else {
        label_color
    };

    let icon_size = palette.tab_icon_size;
    let spacing = 6.0;
    let padding = VERTICAL_TAB_ROW_PADDING;
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(
            item_rect.left() + padding,
            item_rect.center().y - icon_size * 0.5,
        ),
        egui::vec2(icon_size, icon_size),
    );

    let icon_resp = ui
        .interact(
            icon_rect,
            ui.id().with(("vertical_tab_pin", tab.id)),
            egui::Sense::click(),
        )
        .on_hover_text(
            egui::RichText::new(tab_pin_tooltip(i18n, tab.is_pinned))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        );
    let icon_glyph = tab_icon_glyph(tab.is_pinned, icon_resp.hovered() && reorder_drag.is_none());

    let icon_glyph_width = ui
        .painter()
        .layout_no_wrap(icon_glyph.to_string(), icon_font_id.clone(), icon_color)
        .size()
        .x;
    let icon_draw_width = icon_size.max(icon_glyph_width);
    let text_pos = egui::pos2(
        item_rect.left() + padding + icon_draw_width + spacing,
        item_rect.center().y,
    );
    let text_width = item_rect.width() - icon_draw_width - spacing - 2.0 * padding;
    let (display_title, truncated) =
        truncate_item_text(ui, &tab.title, text_width, &font_id, label_color);

    let mut painter = ui.painter().clone();
    if reorder_drag == Some(tab.id) {
        painter.multiply_opacity(VERTICAL_TAB_REORDER_SOURCE_OPACITY);
    }
    if is_active {
        // Tab shape on the content background, open towards the content: rounded on the
        // left, top/bottom edges running into the divider, no right edge.
        let radius = palette.medium_radius;
        let corners = egui::CornerRadius {
            nw: radius,
            ne: 0,
            sw: radius,
            se: 0,
        };
        painter.rect_filled(rect, corners, palette.application_bg_color);
        let open_right_clip = painter
            .clip_rect()
            .intersect(egui::Rect::EVERYTHING.with_max_x(rect.right()));
        painter.with_clip_rect(open_right_clip).rect_stroke(
            rect.with_max_x(rect.right() + 8.0),
            corners,
            vertical_tabs_border_stroke(palette),
            egui::StrokeKind::Inside,
        );
        // Scales with the row height (5 px at 20, 6 px at the 24 px default, 10 px at 40)
        // so the bar keeps its proportions on compact rows.
        let accent_inset = (rect.height() * 0.25).round().max(4.0);
        let accent_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 2.0, rect.top() + accent_inset),
            egui::pos2(rect.left() + 5.0, rect.bottom() - accent_inset),
        );
        painter.rect_filled(
            accent_rect,
            egui::CornerRadius::same(palette.small_radius),
            palette.primary,
        );
    } else {
        let fill = if drop_hovered {
            palette.primary_hover
        } else if row_hovered {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        painter.rect_filled(
            item_rect,
            egui::CornerRadius::same(palette.medium_radius),
            fill,
        );
    }
    painter.text(
        icon_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        icon_glyph,
        icon_font_id,
        icon_color,
    );
    painter.text(
        text_pos,
        egui::Align2::LEFT_CENTER,
        display_title,
        font_id,
        label_color,
    );

    if resp.middle_clicked() {
        action.close = Some(tab.id);
    } else if icon_resp.clicked() {
        action.toggle_pin = Some(tab.full_path.clone());
    } else if resp.clicked() {
        action.activate = Some(tab.id);
    }

    if icon_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    show_tab_context_menu(
        &resp.union(icon_resp),
        i18n,
        palette,
        tabs,
        index,
        active_id,
        TabStripKind::Vertical,
        action,
    );

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        resp.on_hover_text(
            egui::RichText::new(tab_hover_tooltip_text(i18n, tab, truncated))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        );
    }
}

fn handle_draw_tab_new_allocated(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tabs: &[TabInfo],
    index: usize,
    rect: egui::Rect,
    resp: egui::Response,
    active_id: u64,
    palette: &ThemePalette,
    action: &mut TabsAction,
) {
    let tab = &tabs[index];
    let is_active = tab.id == active_id;
    let corner = if is_active {
        palette.tab_active_radius
    } else {
        palette.tab_inactive_radius
    };
    let tab_fill = if is_active {
        ui.visuals().widgets.active.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };

    // --- Font and colors ---
    let font_id = FontId::new(palette.text_size, FontFamily::Proportional);
    let icon_font_id = FontId::new(palette.tab_icon_size, FontFamily::Proportional);
    let label_color = if is_active {
        palette.tab_text_selected
    } else {
        palette.text_normal
    };
    let icon_color = if tab.is_pinned {
        palette.pinned_tab_color
    } else {
        label_color
    };

    // --- Layout parameters ---
    let icon_size = palette.tab_icon_size;
    let spacing = 6.0;
    let padding = 8.0;
    let close_button_width = 20.0;
    let icon_top_left = egui::pos2(rect.left() + padding, rect.center().y - icon_size * 0.5);
    let icon_rect = egui::Rect::from_min_size(icon_top_left, egui::vec2(icon_size, icon_size));

    let icon_resp = ui.interact(
        icon_rect,
        ui.id().with(("tab_pin", tab.id)),
        egui::Sense::click(),
    );
    let icon_resp = icon_resp.on_hover_text(
        egui::RichText::new(tab_pin_tooltip(i18n, tab.is_pinned))
            .size(palette.tooltip_text_size)
            .color(palette.tooltip_text_color),
    );
    let icon_glyph = tab_icon_glyph(tab.is_pinned, icon_resp.hovered());

    let icon_glyph_width = ui
        .painter()
        .layout_no_wrap(icon_glyph.to_string(), icon_font_id.clone(), icon_color)
        .size()
        .x;
    let icon_draw_width = icon_size.max(icon_glyph_width);
    let text_pos = egui::pos2(
        rect.left() + padding + icon_draw_width + spacing,
        rect.center().y,
    );
    let text_width = rect.width() - icon_draw_width - spacing - 2.0 * padding - close_button_width;

    let (display_title, truncated) =
        truncate_item_text(ui, &tab.title, text_width, &font_id, label_color);

    // --- NOW safe to use painter ---
    let painter = ui.painter();

    // --- Paint background ---
    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());

    let rounding = egui::CornerRadius {
        nw: corner.nw,
        ne: corner.ne,
        sw: 0,
        se: 0,
    };

    painter.rect_filled(rect, rounding, tab_fill);

    painter.text(
        icon_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        icon_glyph,
        icon_font_id,
        icon_color,
    );

    painter.text(
        text_pos,
        egui::Align2::LEFT_CENTER,
        display_title,
        font_id.clone(),
        label_color,
    );

    // --- Draw border ---
    let stroke = if is_active {
        egui::Stroke::new(1.0, palette.borders_active)
    } else {
        egui::Stroke::new(1.0, palette.borders_default)
    };

    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Inside);

    if is_active {
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(2.0, ui.visuals().panel_fill),
        );
    }

    let close_resp = tab_close_button(ui, rect, tab.id, is_active, palette);
    if close_resp.clicked() {
        action.close = Some(tab.id);
    } else if icon_resp.clicked() {
        action.toggle_pin = Some(tab.full_path.clone());
    } else if resp.clicked() {
        action.activate = Some(tab.id);
    }

    if icon_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    show_tab_context_menu(
        &resp.union(icon_resp).union(close_resp),
        i18n,
        palette,
        tabs,
        index,
        active_id,
        TabStripKind::Horizontal,
        action,
    );

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);

        let tooltip_text = tab_hover_tooltip_text(i18n, tab, truncated);

        resp.on_hover_text(
            egui::RichText::new(tooltip_text)
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TabStripKind {
    Horizontal,
    Vertical,
}

/// This PC and the Recycle Bin have no folder path to tag, favorite or copy.
fn is_virtual_tab_path(path: &std::path::Path) -> bool {
    let path = path.to_string_lossy();
    path == MY_PC_PATH || path == MY_RECYCLE_BIN_PATH
}

/// Right-click menu of `tabs[index]`, shared by the horizontal strip and the vertical rail.
///
/// Bulk closing ("other tabs", "tabs below/to the right") skips pinned tabs, like browsers
/// do; a pinned tab can still be closed on its own.
fn show_tab_context_menu(
    response: &egui::Response,
    i18n: &I18n,
    palette: &ThemePalette,
    tabs: &[TabInfo],
    index: usize,
    active_id: u64,
    kind: TabStripKind,
    action: &mut TabsAction,
) {
    let tab = &tabs[index];
    egui::Popup::context_menu(response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Default);
            apply_eden_visual_overrides(ui, palette);
            apply_eden_text_overrides(ui, palette);
            // Grow the menu to fit its longest label (e.g. "Remove from favorites") instead of
            // wrapping it inside the default menu width.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            let is_virtual = is_virtual_tab_path(&tab.full_path);
            let command = |ui: &mut egui::Ui, enabled: bool, label: String| {
                ui.add_enabled(enabled, egui::Button::new(label)).clicked()
            };

            if command(ui, !is_virtual, i18n.tr("tab_menu_add_tag")) {
                action.context = Some(TabContextAction::AddTag(tab.full_path.clone()));
                ui.close();
            }

            let favorite_label = if tab.is_favorite {
                i18n.tr("tab_menu_remove_favorite")
            } else {
                i18n.tr("tab_menu_add_favorite")
            };
            let favorite = ui
                .add_enabled(!is_virtual, egui::Button::new(favorite_label))
                .on_disabled_hover_text(
                    egui::RichText::new(i18n.tr("tooltip_favorites_disabled"))
                        .size(palette.tooltip_text_size)
                        .color(palette.tooltip_text_color),
                );
            if favorite.clicked() {
                let path = tab.full_path.clone();
                action.context = Some(if tab.is_favorite {
                    TabContextAction::RemoveFavorite(path)
                } else {
                    TabContextAction::AddFavorite(path)
                });
                ui.close();
            }

            if command(ui, true, tab_pin_tooltip(i18n, tab.is_pinned)) {
                action.toggle_pin = Some(tab.full_path.clone());
                ui.close();
            }

            if command(ui, true, i18n.tr("tab_menu_duplicate")) {
                action.context = Some(TabContextAction::Duplicate(tab.id));
                ui.close();
            }

            // Opens next to the active tab's own view, so it's pointless for the active tab.
            if tab.id != active_id && command(ui, true, i18n.tr("inputs_opensplit")) {
                action.context = Some(TabContextAction::OpenInSplit(tab.full_path.clone()));
                ui.close();
            }

            if command(ui, !is_virtual, i18n.tr("tab_menu_copy_path")) {
                action.context = Some(TabContextAction::CopyPath(tab.full_path.clone()));
                ui.close();
            }

            ui.separator();

            if command(ui, true, i18n.tr("tab_menu_close")) {
                action.close = Some(tab.id);
                ui.close();
            }

            let can_close_others = tabs
                .iter()
                .any(|other| other.id != tab.id && !other.is_pinned);
            if command(ui, can_close_others, i18n.tr("tab_menu_close_others")) {
                action.context = Some(TabContextAction::CloseOthers(tab.id));
                ui.close();
            }

            let can_close_after = tabs[index + 1..].iter().any(|other| !other.is_pinned);
            let close_after_label = match kind {
                TabStripKind::Vertical => i18n.tr("tab_menu_close_below"),
                TabStripKind::Horizontal => i18n.tr("tab_menu_close_right"),
            };
            if command(ui, can_close_after, close_after_label) {
                action.context = Some(TabContextAction::CloseAfter(tab.id));
                ui.close();
            }
        });
}

fn handle_draw_add_new_tab_button(
    ui: &mut egui::Ui,
    palette: &ThemePalette,
    action: &mut TabsAction,
) {
    let size = egui::vec2(32.0, 32.0);

    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());

    let hovered = resp.hovered();

    let fill = if hovered {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };

    let stroke = if hovered {
        egui::Stroke::new(1.0, palette.borders_active)
    } else {
        egui::Stroke::new(1.0, palette.borders_default)
    };

    let rounding = egui::CornerRadius {
        nw: palette.tab_inactive_radius.nw,
        ne: palette.tab_inactive_radius.ne,
        sw: 0,
        se: 0,
    };

    let painter = ui.painter();

    painter.rect_filled(rect, rounding, fill);
    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Inside);

    let _ = tab_add_button(ui, rect.shrink(3.0), resp.clone(), palette);

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if resp.clicked() {
        action.open_new = true;
    }
}

fn handle_draw_nav_button(ui: &mut egui::Ui, palette: &ThemePalette, direction: &str) -> bool {
    let size = egui::vec2(32.0, 32.0);

    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());

    let hovered = resp.hovered();

    let fill = if hovered {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };

    let stroke = if hovered {
        egui::Stroke::new(1.0, palette.borders_active)
    } else {
        egui::Stroke::new(1.0, palette.borders_default)
    };

    let rounding = egui::CornerRadius {
        nw: palette.tab_inactive_radius.nw,
        ne: palette.tab_inactive_radius.ne,
        sw: 0,
        se: 0,
    };

    let painter = ui.painter();

    painter.rect_filled(rect, rounding, fill);
    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Inside);

    let _ = tab_nav_button(ui, rect.shrink(3.0), resp.clone(), palette, direction);

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp.clicked()
}

fn tab_close_button(
    ui: &mut egui::Ui,
    tab_rect: egui::Rect,
    tab_id: u64,
    is_active: bool,
    palette: &ThemePalette,
) -> egui::Response {
    let size = egui::vec2(18.0, 18.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(
            tab_rect.right() - size.x - 6.0,
            tab_rect.center().y - size.y * 0.5,
        ),
        size,
    );
    let resp = ui.interact(
        rect,
        ui.id().with(("tab_close", tab_id)),
        egui::Sense::click(),
    );
    let hovered = resp.hovered();
    let bg = if hovered {
        palette.tab_close_hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, palette.tab_button_radius, bg);

    let color = if hovered {
        palette.icon_colored_hover
    } else if is_active {
        palette.tab_close_active
    } else {
        palette.tab_close_normal
    };

    let font_id = FontId::new(palette.tab_icon_size, FontFamily::Proportional);

    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        regular::X,
        font_id,
        color,
    );

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

fn tab_add_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    resp: egui::Response,
    palette: &ThemePalette,
) -> egui::Response {
    let hovered = resp.hovered();

    let bg = if hovered {
        palette.tab_add_hover
    } else {
        egui::Color32::TRANSPARENT
    };

    let visual_rect = rect.shrink(4.0);

    ui.painter()
        .rect_filled(visual_rect, palette.tab_button_radius, bg);

    let color = if hovered {
        palette.icon_colored_hover
    } else {
        palette.icon_color
    };

    let font_id = FontId::new(palette.tab_icon_size, FontFamily::Proportional);

    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        regular::PLUS,
        font_id,
        color,
    );

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

fn tab_nav_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    resp: egui::Response,
    palette: &ThemePalette,
    direction: &str,
) -> egui::Response {
    let hovered = resp.hovered();

    let bg = if hovered {
        palette.tab_add_hover
    } else {
        egui::Color32::TRANSPARENT
    };

    let visual_rect = rect.shrink(4.0);

    ui.painter()
        .rect_filled(visual_rect, palette.tab_button_radius, bg);

    let color = if hovered {
        palette.icon_colored_hover
    } else {
        palette.icon_color
    };

    let font_id = FontId::new(palette.tab_icon_size, FontFamily::Proportional);

    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if direction == "left" {
            regular::CARET_LEFT
        } else {
            regular::CARET_RIGHT
        },
        font_id,
        color,
    );

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::theme::{ThemeMode, get_default_palette};

    #[test]
    fn wheel_steps_counts_lines_pages_and_points() {
        use egui::MouseWheelUnit::{Line, Page, Point};
        // One notch is one step; the sign follows the wheel (positive = up).
        assert_eq!(wheel_steps(0.0, 1.0, Line, 30.0), (1, 0.0));
        assert_eq!(wheel_steps(0.0, -1.0, Line, 30.0), (-1, 0.0));
        assert_eq!(wheel_steps(0.0, -3.0, Line, 30.0), (-3, 0.0));
        assert_eq!(wheel_steps(0.0, 2.0, Page, 30.0), (2, 0.0));
        // High-resolution wheels: fractions add up across events.
        assert_eq!(wheel_steps(0.0, -0.5, Line, 30.0), (0, -0.5));
        assert_eq!(wheel_steps(-0.5, -0.5, Line, 30.0), (-1, 0.0));
        // Touchpad points: one step per row height, the rest is kept.
        assert_eq!(wheel_steps(0.0, 15.0, Point, 30.0), (0, 0.5));
        assert_eq!(wheel_steps(0.5, 30.0, Point, 30.0), (1, 0.5));
        assert_eq!(wheel_steps(0.0, -75.0, Point, 30.0), (-2, -0.5));
        // Reversing drops the remainder of the other direction.
        assert_eq!(wheel_steps(0.9, -0.5, Line, 30.0), (0, -0.5));
        assert_eq!(wheel_steps(-0.9, 1.0, Line, 30.0), (1, 0.0));
        // No input keeps the remainder; junk input is ignored.
        assert_eq!(wheel_steps(0.25, 0.0, Line, 30.0), (0, 0.25));
        assert_eq!(wheel_steps(0.25, f32::NAN, Point, 30.0), (0, 0.25));
        // A degenerate row height does not divide by zero.
        assert_eq!(wheel_steps(0.0, 2.0, Point, 0.0), (2, 0.0));
    }

    #[test]
    fn wheel_target_index_moves_and_clamps() {
        // Wheel down (negative steps) goes to the next tab, up to the previous one.
        assert_eq!(wheel_target_index(2, -1, 5), 3);
        assert_eq!(wheel_target_index(2, 1, 5), 1);
        assert_eq!(wheel_target_index(2, -2, 5), 4);
        // Clamped at both ends, no wrap-around.
        assert_eq!(wheel_target_index(4, -1, 5), 4);
        assert_eq!(wheel_target_index(0, 1, 5), 0);
        assert_eq!(wheel_target_index(1, i32::MIN, 5), 4);
        assert_eq!(wheel_target_index(1, i32::MAX, 5), 0);
        assert_eq!(wheel_target_index(0, -1, 1), 0);
    }

    #[test]
    fn reorder_index_maps_slots_to_final_positions() {
        // Dropping right above or below itself is a no-op.
        assert_eq!(reorder_index(2, 2, 5), None);
        assert_eq!(reorder_index(2, 3, 5), None);
        // Moving up lands at the slot.
        assert_eq!(reorder_index(2, 0, 5), Some(0));
        assert_eq!(reorder_index(4, 1, 5), Some(1));
        // Moving down lands one before the slot (the tab left its old place).
        assert_eq!(reorder_index(0, 2, 5), Some(1));
        assert_eq!(reorder_index(0, 5, 5), Some(4));
        assert_eq!(reorder_index(1, 4, 5), Some(3));
        // Out of range.
        assert_eq!(reorder_index(5, 0, 5), None);
        assert_eq!(reorder_index(0, 6, 5), None);
        assert_eq!(reorder_index(0, 0, 1), None);
        assert_eq!(reorder_index(0, 1, 1), None);
    }

    #[test]
    fn reorder_insert_slot_uses_row_midpoints() {
        let rows: Vec<egui::Rect> = (0..3)
            .map(|i| {
                let top = i as f32 * 26.0;
                egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(100.0, top + 24.0))
            })
            .collect();
        assert_eq!(reorder_insert_slot(-50.0, &rows), 0);
        assert_eq!(reorder_insert_slot(11.0, &rows), 0);
        assert_eq!(reorder_insert_slot(13.0, &rows), 1);
        assert_eq!(reorder_insert_slot(40.0, &rows), 2);
        assert_eq!(reorder_insert_slot(70.0, &rows), 3);
        assert_eq!(reorder_insert_slot(500.0, &rows), 3);
        assert_eq!(reorder_insert_slot(0.0, &[]), 0);
    }

    #[test]
    fn pinned_paths_follow_the_tab_order() {
        let p = |s: &str| PathBuf::from(s);
        let pinned = vec![p("A"), p("X"), p("B"), p("C")];
        let tabs = [p("C"), p("T"), p("B"), p("A")];
        let tab_paths: Vec<&std::path::Path> = tabs.iter().map(|t| t.as_path()).collect();
        // X has no open tab and keeps its slot; the open ones follow the tab order.
        assert_eq!(
            pinned_paths_in_tab_order(&pinned, &tab_paths),
            Some(vec![p("C"), p("X"), p("B"), p("A")])
        );

        // Already in tab order: no change, so nothing is saved.
        let in_order = vec![p("A"), p("X"), p("B")];
        let tabs = [p("A"), p("B"), p("T")];
        let tab_paths: Vec<&std::path::Path> = tabs.iter().map(|t| t.as_path()).collect();
        assert_eq!(pinned_paths_in_tab_order(&in_order, &tab_paths), None);

        // Duplicate tabs of a pinned path count once, at the first one.
        let pinned = vec![p("A"), p("B")];
        let tabs = [p("B"), p("A"), p("B")];
        let tab_paths: Vec<&std::path::Path> = tabs.iter().map(|t| t.as_path()).collect();
        assert_eq!(
            pinned_paths_in_tab_order(&pinned, &tab_paths),
            Some(vec![p("B"), p("A")])
        );

        // No pins, or a duplicated pin of an open path: left alone.
        assert_eq!(pinned_paths_in_tab_order(&[], &tab_paths), None);
        let dup = vec![p("B"), p("A"), p("B")];
        assert_eq!(pinned_paths_in_tab_order(&dup, &tab_paths), None);
    }

    #[test]
    fn vertical_tab_row_height_is_bounded_and_fits_the_text() {
        let palette = get_default_palette(ThemeMode::Dark);
        assert_eq!(
            vertical_tab_row_height(VERTICAL_TAB_ROW_HEIGHT_DEFAULT, &palette),
            VERTICAL_TAB_ROW_HEIGHT_DEFAULT
        );
        assert_eq!(
            vertical_tab_row_height(1.0, &palette),
            VERTICAL_TAB_ROW_HEIGHT_MIN
        );
        assert_eq!(
            vertical_tab_row_height(500.0, &palette),
            VERTICAL_TAB_ROW_HEIGHT_MAX
        );
        assert_eq!(
            vertical_tab_row_height(f32::NAN, &palette),
            VERTICAL_TAB_ROW_HEIGHT_DEFAULT
        );
        // Whole pixels, so row edges stay crisp.
        assert_eq!(vertical_tab_row_height(26.4, &palette), 26.0);

        // A larger theme text size raises the effective minimum instead of clipping.
        let mut large_text = palette.clone();
        large_text.text_size = 20.0;
        assert_eq!(
            vertical_tab_row_height(VERTICAL_TAB_ROW_HEIGHT_MIN, &large_text),
            20.0 + VERTICAL_TAB_ROW_MIN_VERTICAL_MARGIN
        );
    }
}
