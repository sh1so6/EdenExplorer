use crate::core::fs::{MY_PC_PATH, MY_RECYCLE_BIN_PATH};
use crate::core::portable;
use crate::core::utils::text::apply_eden_text_overrides;
use crate::core::utils::widgets::apply_eden_visual_overrides;
use crate::gui::i18n::I18n;
use crate::gui::icons::IconCache;
use crate::gui::theme::ThemePalette;
use crate::gui::utils::{clickable_icon, expand_environment_variables};
use crate::gui::windows::containers::enums::ItemViewerNavAction;
use crate::gui::windows::containers::structs::{
    Breadcrumb, ItemViewerDisplayMode, ItemViewerNavBarAction, RenderedBreadcrumb, TabView,
};
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use egui::{FontFamily, FontId};
use egui_phosphor::{fill, regular};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Narrowest navigation bar worth showing in the vertical-tabs title-bar
/// strip: the button group (~236px) plus ~164px of breadcrumb, enough for
/// "root > ... > <typical folder name>". Narrower bars stay inside their pane
/// instead. Update when buttons are added or removed.
pub const NAVBAR_MIN_STRIP_WIDTH: f32 = 400.0;

pub fn draw_itemviewer_navigation_bar(
    ui: &mut egui::Ui,
    i18n: &I18n,
    icon_cache: &IconCache,
    tab: &mut TabView,
    tab_id: u64,
    palette: &ThemePalette,
    is_favorited: bool,
    drag_active: bool,
    drag_hover_target: Option<PathBuf>,
) -> ItemViewerNavBarAction {
    let mut action = ItemViewerNavBarAction::default();
    let tabbar_rect = ui.available_rect_before_wrap();
    ui.set_clip_rect(tabbar_rect);

    let pointer_pos = ui.input(|i| i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()));
    let pointer_released =
        ui.input(|i| i.pointer.any_released() && i.pointer.interact_pos().is_some());
    let hovered_target_ref = drag_hover_target.as_ref();
    let mut breadcrumb_drop_target: Option<PathBuf> = None;
    let pointer_in_tabbar = pointer_pos
        .map(|pos| tabbar_rect.contains(pos))
        .unwrap_or(false);
    let can_go_back = tab.nav.can_go_back();
    let can_go_forward = tab.nav.can_go_forward();
    let is_recycle_bin = tab.nav.is_recycle_bin();

    ui.horizontal(|ui| {
        ui.add_space(1.5);
        let toolbar_action = draw_navigation_bar_buttons(
            ui,
            i18n,
            palette,
            is_favorited,
            &tab.nav.current,
            tab.nav.is_root(),
            is_recycle_bin,
            can_go_back,
            can_go_forward,
            tab.display_mode,
        );

        merge_toolbar_action(&mut action, toolbar_action);
        if action.toggle_gallery {
            tab.display_mode = match tab.display_mode {
                ItemViewerDisplayMode::Details => ItemViewerDisplayMode::Gallery,
                ItemViewerDisplayMode::Gallery => ItemViewerDisplayMode::Details,
            };
        }

        ui.separator();

        let breadcrumb_width = ui.available_width();

        if tab.breadcrumb_path_editing {
            let text_edit_id = ui.id().with(("breadcrumbs_path_edit", tab_id));

            if tab.breadcrumb_select_all_on_focus {
                let mut state =
                    egui::widgets::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                        .unwrap_or_default();
                let cursor_end = CCursor::new(tab.breadcrumb_path_buffer.chars().count());
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::two(CCursor::new(0), cursor_end)));
                state.store(ui.ctx(), text_edit_id);
            }

            let mut offset_x = 0.0;
            if tab.breadcrumb_path_error {
                let t = (ui.input(|i| i.time) - tab.breadcrumb_path_error_animation_time) as f32;

                if t < 0.4 {
                    let frequency = 30.0_f32;
                    let amplitude = 4.0 * (1.0 - t / 0.4); // decay
                    offset_x = (t * frequency).sin() * amplitude;
                }
            }

            ui.add_space(offset_x);

            let time_since_error = ui.input(|i| i.time) - tab.breadcrumb_path_error_animation_time;
            let error_strength = (1.0 - time_since_error * 2.0).clamp(0.0, 1.0);

            let stroke_color = if tab.breadcrumb_path_error {
                egui::Color32::from_rgba_premultiplied(255, 80, 80, (255.0 * error_strength) as u8)
            } else {
                ui.visuals().widgets.inactive.bg_stroke.color
            };

            let frame = egui::Frame::NONE
                .stroke(egui::Stroke::new(1.5, stroke_color))
                .corner_radius(egui::CornerRadius::same(4));

            let mut resp = frame
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut tab.breadcrumb_path_buffer)
                            .id(text_edit_id)
                            .frame(if tab.breadcrumb_path_error {
                                egui::Frame::NONE
                            } else {
                                egui::Frame::default()
                            })
                            .desired_width(ui.available_width() - 40.0)
                            .font(FontId::new(
                                palette.text_size,
                                egui::FontFamily::Proportional,
                            )),
                    )
                })
                .inner;

            if resp.changed() {
                tab.breadcrumb_path_error = false;
            }

            if tab.breadcrumb_path_error && resp.hovered() {
                resp = resp.on_hover_text(
                    egui::RichText::new(i18n.tr("tooltip_path_does_not_exist"))
                        .size(palette.tooltip_text_size)
                        .color(palette.tooltip_text_color),
                );
            }

            if !tab.breadcrumb_just_started_editing || tab.breadcrumb_path_error {
                resp.request_focus();
                tab.breadcrumb_just_started_editing = true;
            }

            if resp.has_focus() {
                tab.breadcrumb_select_all_on_focus = false;
            }

            action.is_breadcrumb_path_edit_active = resp.has_focus();

            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));

            let mut exit_edit_mode = false;

            if enter {
                let input = tab.breadcrumb_path_buffer.trim().trim_matches('"');
                let expanded_input = expand_environment_variables(input);
                let new_path = PathBuf::from(&expanded_input);

                if new_path.exists() {
                    action.nav_to = Some(new_path);
                    exit_edit_mode = true;
                } else {
                    println!(
                        "{}: {} ({}: {})",
                        i18n.tr("tooltip_invalid_path"),
                        tab.breadcrumb_path_buffer,
                        i18n.tr("tooltip_invalid_path_expanded"),
                        expanded_input
                    );
                    tab.breadcrumb_path_error = true;
                    tab.breadcrumb_path_error_animation_time = ui.input(|i| i.time);
                }
            } else if escape || resp.lost_focus() {
                tab.breadcrumb_path_buffer = tab.nav.current.to_string_lossy().to_string();
                exit_edit_mode = true;
            }

            if exit_edit_mode {
                tab.breadcrumb_path_editing = false;
                tab.breadcrumb_just_started_editing = false;
                tab.breadcrumb_path_error = false;
                tab.breadcrumb_path_error_animation_time = 0.0;

                ui.memory_mut(|mem| mem.surrender_focus(text_edit_id));
            }
        } else if tab.nav.is_root() || tab.nav.is_recycle_bin() {
            let pc_icon_path = PathBuf::from("C:\\");
            if tab.nav.is_recycle_bin() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(regular::TRASH)
                            .size(14.0)
                            .color(palette.text_header_section),
                    )
                    .selectable(false),
                );
            } else if let Some(icon) = icon_cache.get(&pc_icon_path, true) {
                ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(14.0, 14.0)));
            }

            if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(if tab.nav.is_recycle_bin() {
                            i18n.tr("recycle_bin")
                        } else {
                            i18n.tr("thispc")
                        })
                        .size(palette.text_size)
                        .color(palette.text_header_section),
                    )
                    .selectable(false)
                    .sense(egui::Sense::click()),
                )
                .clicked()
            {
                action.nav_to = Some(if tab.nav.is_recycle_bin() {
                    PathBuf::from(MY_RECYCLE_BIN_PATH)
                } else {
                    PathBuf::from(MY_PC_PATH)
                });
            }
        } else {
            let font_id = egui::FontId::new(palette.text_size, egui::FontFamily::Proportional);
            let breadcrumbs = build_breadcrumbs(&tab.nav.current);
            let layout = layout_breadcrumbs(ui, &breadcrumbs, breadcrumb_width, &font_id);
            // Per view: split panes draw separate navigation bars.
            let ellipsis_popup_id = ui.id().with(("breadcrumb_ellipsis_popup", tab_id));

            // A path change can remove the ellipsis while its popup is open.
            if layout.omitted.is_empty() {
                egui::Popup::close_id(ui.ctx(), ellipsis_popup_id);
            }

            let mut first = true;
            let mut breadcrumbs_right = 0.0;

            for crumb in &layout.segments {
                if !first {
                    let old_spacing = ui.spacing().item_spacing;
                    ui.spacing_mut().item_spacing.x = 10.0;

                    ui.label(
                        egui::RichText::new(">")
                            .size(palette.text_size)
                            .color(palette.text_header_section),
                    );

                    ui.spacing_mut().item_spacing = old_spacing;
                }
                first = false;

                let resp = draw_breadcrumb(ui, crumb, palette);

                handle_breadcrumb_drag(
                    ui,
                    &resp,
                    crumb,
                    palette,
                    drag_active,
                    hovered_target_ref,
                    pointer_pos,
                    pointer_released,
                    &mut breadcrumb_drop_target,
                    &mut action,
                );

                handle_breadcrumb_hover(ui, &resp);

                handle_breadcrumb_click(&resp, crumb, drag_active, &mut action);

                if crumb.is_ellipsis {
                    show_omitted_breadcrumbs_popup(
                        &resp,
                        ellipsis_popup_id,
                        &layout.omitted,
                        palette,
                        drag_active,
                        &mut action,
                    );
                }

                breadcrumbs_right = resp.rect.right();
            }

            // ---- Detect click in empty area to enter path editing ----
            let available_rect = ui.available_rect_before_wrap();
            let empty_area = egui::Rect::from_min_max(
                egui::pos2(breadcrumbs_right, available_rect.top()),
                available_rect.right_bottom(),
            );

            if ui
                .interact(
                    empty_area,
                    ui.id().with("breadcrumb_empty"),
                    egui::Sense::click(),
                )
                .clicked()
            {
                tab.breadcrumb_path_editing = true;
                tab.breadcrumb_path_buffer = tab.nav.current.to_string_lossy().to_string();
            }
        }
    });

    if action.nav.is_none() && pointer_in_tabbar && !tab.breadcrumb_path_editing {
        if can_go_back && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1)) {
            action.nav = Some(ItemViewerNavAction::Back);
        } else if can_go_forward
            && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2))
        {
            action.nav = Some(ItemViewerNavAction::Forward);
        }
    }

    action.move_files_to_breadcrumb_dir = breadcrumb_drop_target;
    action
}

fn nav_icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    palette: &ThemePalette,
    enabled: bool,
    hover_text: &str,
) -> egui::Response {
    let font_id = egui::FontId::default();
    let resp = ui.add_enabled(
        enabled,
        egui::Label::new(
            egui::RichText::new(icon)
                .font(font_id.clone())
                .color(ui.visuals().text_color()),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );

    if enabled && resp.hovered() {
        ui.painter().text(
            resp.rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            font_id.clone(),
            palette.primary,
        );
    }

    let resp = resp.on_hover_text(
        egui::RichText::new(hover_text)
            .size(palette.tooltip_text_size)
            .color(palette.tooltip_text_color),
    );

    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn draw_navigation_bar_buttons(
    ui: &mut egui::Ui,
    i18n: &I18n,
    palette: &ThemePalette,
    is_favorited: bool,
    current_dir: &Path,
    is_root: bool,
    is_recycle_bin: bool,
    can_go_back: bool,
    can_go_forward: bool,
    display_mode: ItemViewerDisplayMode,
) -> ItemViewerNavBarAction {
    let mut action = ItemViewerNavBarAction::default();

    // Navigation buttons
    if nav_icon_button(
        ui,
        regular::ARROW_LEFT,
        palette,
        can_go_back,
        &i18n.tr("tooltip_nav_back"),
    )
    .clicked()
        && can_go_back
    {
        action.nav = Some(ItemViewerNavAction::Back);
    }

    if nav_icon_button(
        ui,
        regular::ARROW_RIGHT,
        palette,
        can_go_forward,
        &i18n.tr("tooltip_nav_forward"),
    )
    .clicked()
        && can_go_forward
    {
        action.nav = Some(ItemViewerNavAction::Forward);
    }

    let can_go_up = !is_root && !is_recycle_bin;
    if nav_icon_button(
        ui,
        regular::ARROW_UP,
        palette,
        can_go_up,
        &i18n.tr("tooltip_nav_up"),
    )
    .clicked()
        && can_go_up
    {
        action.nav = Some(ItemViewerNavAction::Up);
    }

    if clickable_icon(ui, regular::ARROWS_CLOCKWISE, palette)
        .on_hover_text(
            egui::RichText::new(i18n.tr("tooltip_refresh"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        action.refresh_current_directory = true;
    }

    // Action buttons
    if nav_icon_button(
        ui,
        regular::FOLDER_PLUS,
        palette,
        !is_recycle_bin,
        &i18n.tr("tooltip_newfolder"),
    )
    .clicked()
        && !is_recycle_bin
    {
        action.create_folder = true;
    }

    if nav_icon_button(
        ui,
        regular::FILE_PLUS,
        palette,
        !is_recycle_bin,
        &i18n.tr("tooltip_newfile"),
    )
    .clicked()
        && !is_recycle_bin
    {
        action.create_file = true;
    }

    let star_icon = if is_favorited {
        fill::STAR
    } else {
        regular::STAR
    };
    let star_color = if is_favorited {
        palette.button_favorite_fill
    } else {
        palette.tooltip_text_color
    };

    let star_font = if is_favorited {
        FontId::new(palette.text_size, FontFamily::Name("phosphor_fill".into()))
    } else {
        FontId::new(palette.text_size, FontFamily::Proportional)
    };

    let star_resp = ui.add_enabled(
        !is_root && !is_recycle_bin,
        egui::Label::new(
            egui::RichText::new(star_icon)
                .font(star_font)
                .color(star_color),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );

    let star_resp = if is_root || is_recycle_bin {
        star_resp.on_hover_text(
            egui::RichText::new(i18n.tr("tooltip_favorites_disabled"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
    } else {
        star_resp
            .on_hover_text(
                egui::RichText::new(if is_favorited {
                    i18n.tr("tooltip_favorites_remove")
                } else {
                    i18n.tr("tooltip_favorites_add")
                })
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    };

    if star_resp.clicked() {
        if is_favorited {
            action.remove_favorite = true;
        } else {
            action.add_favorite = true;
        }
    }

    let gallery_color = if display_mode == ItemViewerDisplayMode::Gallery {
        palette.primary
    } else {
        ui.visuals().text_color()
    };
    if ui
        .add_enabled(
            !is_recycle_bin && !is_root,
            egui::Label::new(egui::RichText::new(regular::IMAGES_SQUARE).color(gallery_color))
                .selectable(false)
                .sense(egui::Sense::click()),
        )
        .on_hover_text(
            egui::RichText::new(i18n.tr("tooltip_gallery_view"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && !is_recycle_bin
    {
        action.toggle_gallery = true;
    }

    if nav_icon_button(
        ui,
        regular::TERMINAL,
        palette,
        !is_recycle_bin,
        &i18n.tr("tooltip_open_terminal"),
    )
    .clicked()
        && !is_recycle_bin
    {
        open_default_terminal(current_dir);
    }

    action
}

pub(crate) fn open_default_terminal(current_dir: &Path) {
    let start_dir = if current_dir.to_string_lossy() == MY_PC_PATH || !current_dir.exists() {
        dirs::home_dir().unwrap_or_else(|| current_dir.to_path_buf())
    } else {
        current_dir.to_path_buf()
    };

    let launched = Command::new("wt.exe")
        .arg("-d")
        .arg(&start_dir)
        .spawn()
        .is_ok()
        || Command::new("powershell.exe")
            .current_dir(&start_dir)
            .spawn()
            .is_ok()
        || Command::new("cmd.exe")
            .current_dir(&start_dir)
            .spawn()
            .is_ok();

    if !launched {
        eprintln!("Failed to open terminal");
    }
}

fn measure_breadcrumb_width(ui: &egui::Ui, font_id: &egui::FontId, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), ui.visuals().text_color())
        .size()
        .x
}

/// Label of the crumb standing in for omitted ancestors, and the suffix of truncated labels.
const BREADCRUMB_ELLIPSIS: &str = "...";
/// Horizontal inner margin of a breadcrumb frame (6px on each side, see `draw_breadcrumb`).
const BREADCRUMB_ITEM_PADDING: f32 = 12.0;
/// Item spacing used after the ">" separator label (see the breadcrumb loop).
const BREADCRUMB_SEPARATOR_SPACING: f32 = 10.0;

/// Shortens `text` to its longest prefix that, followed by [`BREADCRUMB_ELLIPSIS`], is at most
/// `max_width` wide. Returns the shortened label and its measured width.
///
/// Always keeps at least one character, even if that overflows `max_width`. Returns `None`
/// when `text` has fewer than two characters and so cannot be shortened.
///
/// Binary search over char boundaries: `O(log n)` measurements instead of one per char.
fn truncate_label_to_width(
    text: &str,
    max_width: f32,
    measure: &impl Fn(&str) -> f32,
) -> Option<(String, f32)> {
    // ends[k - 1] is the byte length of the prefix holding the first k chars.
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .skip(1)
        .chain(std::iter::once(text.len()))
        .collect();
    let char_count = ends.len();

    if char_count < 2 {
        return None;
    }

    let candidate = |chars: usize| {
        let prefix = &text[..ends[chars - 1]];
        let trimmed = prefix.trim_end();
        let prefix = if trimmed.is_empty() { prefix } else { trimmed };
        let label = format!("{prefix}{BREADCRUMB_ELLIPSIS}");
        let width = measure(&label);
        (label, width)
    };

    let mut best = None;
    let (mut low, mut high) = (1, char_count - 1);

    while low <= high {
        let mid = low + (high - low) / 2;
        let (label, width) = candidate(mid);

        if width <= max_width {
            best = Some((label, width));
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }

    Some(best.unwrap_or_else(|| candidate(1)))
}

fn rendered_breadcrumb(crumb: &Breadcrumb, width: f32) -> RenderedBreadcrumb {
    RenderedBreadcrumb {
        label: crumb.label.clone(),
        full_label: crumb.label.clone(),
        path: crumb.path.clone(),
        truncated: false,
        is_ellipsis: false,
        width,
    }
}

/// Renders `crumb`, truncating its label if its full `width` exceeds `max_width`.
fn fit_breadcrumb(
    crumb: &Breadcrumb,
    width: f32,
    max_width: f32,
    measure: &impl Fn(&str) -> f32,
) -> RenderedBreadcrumb {
    let mut rendered = rendered_breadcrumb(crumb, width);

    if width > max_width {
        let max_label_width = max_width - BREADCRUMB_ITEM_PADDING;

        // A very short label can be narrower than its truncated form; keep it whole then.
        if let Some((label, label_width)) =
            truncate_label_to_width(&crumb.label, max_label_width, measure)
                .filter(|(_, label_width)| label_width + BREADCRUMB_ITEM_PADDING < width)
        {
            rendered.label = label;
            rendered.truncated = true;
            rendered.width = label_width + BREADCRUMB_ITEM_PADDING;
        }
    }

    rendered
}

fn build_breadcrumbs(path: &Path) -> Vec<Breadcrumb> {
    use std::path::Component;

    if portable::is_portable_path(&path.to_path_buf()) {
        return portable::build_breadcrumb_segments(&path.to_path_buf())
            .unwrap_or_default()
            .into_iter()
            .map(|(label, path)| Breadcrumb { label, path })
            .collect();
    }

    let mut breadcrumbs = Vec::new();
    let mut current = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                current.push(prefix.as_os_str());

                breadcrumbs.push(Breadcrumb {
                    label: prefix.as_os_str().to_string_lossy().into_owned(),
                    path: current.clone(),
                });
            }

            Component::RootDir => {
                current.push(Path::new("\\"));
            }

            Component::Normal(name) => {
                current.push(name);

                breadcrumbs.push(Breadcrumb {
                    label: name.to_string_lossy().into_owned(),
                    path: current.clone(),
                });
            }

            _ => {}
        }
    }

    breadcrumbs
}

/// Breadcrumbs that fit the navigation bar, plus the ancestors hidden behind the
/// ellipsis crumb (in path order; empty when nothing was omitted).
#[derive(Debug, Default)]
struct BreadcrumbLayout {
    segments: Vec<RenderedBreadcrumb>,
    omitted: Vec<Breadcrumb>,
}

fn layout_breadcrumbs(
    ui: &egui::Ui,
    breadcrumbs: &[Breadcrumb],
    available_width: f32,
    font_id: &egui::FontId,
) -> BreadcrumbLayout {
    let measure = |text: &str| measure_breadcrumb_width(ui, font_id, text);
    // Spacing before ">", the ">" label itself, and the spacing after it.
    let separator_width = ui.spacing().item_spacing.x + measure(">") + BREADCRUMB_SEPARATOR_SPACING;

    compute_breadcrumb_layout(breadcrumbs, available_width, separator_width, measure)
}

/// Fits `breadcrumbs` into `available_width`. Pure: text widths come from `measure`.
///
/// Priorities, highest first:
/// 1. The current folder (last crumb) is always shown, truncated only when even
///    `first > ... > current` doesn't fit.
/// 2. The first crumb (drive, UNC share or portable device) is always shown. A long one is
///    truncated before the current folder; when both are long they share the space.
/// 3. Ancestors are added right to left (nearest to the current folder first) while they fit;
///    the rest collapse into one ellipsis crumb whose `path` is the deepest omitted folder.
///
/// In extreme cases (`first > ... > c...` alone is too wide) the result overflows slightly
/// rather than dropping a crumb.
fn compute_breadcrumb_layout(
    breadcrumbs: &[Breadcrumb],
    available_width: f32,
    separator_width: f32,
    measure: impl Fn(&str) -> f32,
) -> BreadcrumbLayout {
    let Some(last) = breadcrumbs.len().checked_sub(1) else {
        return BreadcrumbLayout::default();
    };

    let widths: Vec<f32> = breadcrumbs
        .iter()
        .map(|crumb| measure(&crumb.label) + BREADCRUMB_ITEM_PADDING)
        .collect();

    // Fast path: everything fits.
    let total_width = widths.iter().sum::<f32>() + separator_width * last as f32;

    if total_width <= available_width {
        return BreadcrumbLayout {
            segments: breadcrumbs
                .iter()
                .zip(&widths)
                .map(|(crumb, &width)| rendered_breadcrumb(crumb, width))
                .collect(),
            omitted: Vec::new(),
        };
    }

    if last == 0 {
        return BreadcrumbLayout {
            segments: vec![fit_breadcrumb(
                &breadcrumbs[0],
                widths[0],
                available_width,
                &measure,
            )],
            omitted: Vec::new(),
        };
    }

    // Ancestors between the first crumb and the current folder.
    let has_middle = last > 1;
    let ellipsis_width = if has_middle {
        measure(BREADCRUMB_ELLIPSIS) + BREADCRUMB_ITEM_PADDING
    } else {
        0.0
    };
    let ellipsis_slot = if has_middle {
        ellipsis_width + separator_width
    } else {
        0.0
    };

    // Reserve the current folder at full width, then the first crumb and the ellipsis.
    let mut budget = available_width - widths[last] - widths[0] - separator_width - ellipsis_slot;

    // Visible ancestors are `first_visible..last`; fill them in from the right.
    let mut first_visible = last;

    while first_visible > 1 {
        let needed = widths[first_visible - 1] + separator_width;

        if needed > budget {
            break;
        }

        budget -= needed;
        first_visible -= 1;
    }

    let omitted = breadcrumbs[1..first_visible].to_vec();

    let mut segments = Vec::with_capacity(last - first_visible + 3);
    segments.push(rendered_breadcrumb(&breadcrumbs[0], widths[0]));

    // Width of everything except the first crumb and the current folder (with separators).
    let mut middle_width = separator_width;

    if let Some(deepest) = omitted.last() {
        segments.push(RenderedBreadcrumb {
            label: BREADCRUMB_ELLIPSIS.into(),
            full_label: BREADCRUMB_ELLIPSIS.into(),
            path: deepest.path.clone(),
            truncated: false,
            is_ellipsis: true,
            width: ellipsis_width,
        });
        middle_width += ellipsis_slot;
    }

    for index in first_visible..last {
        segments.push(rendered_breadcrumb(&breadcrumbs[index], widths[index]));
        middle_width += widths[index] + separator_width;
    }

    // Ancestors are only added when the first crumb and the current folder fit whole, so
    // truncation below only happens with no visible ancestor. The current folder keeps its
    // full width while the first crumb can spare it; if both are long they split the space.
    let space = available_width - middle_width;
    let first_budget = if widths[0] + widths[last] <= space {
        widths[0]
    } else {
        (space - widths[last]).max(space / 2.0)
    };

    let mut first = fit_breadcrumb(&breadcrumbs[0], widths[0], first_budget, &measure);
    let current = fit_breadcrumb(
        &breadcrumbs[last],
        widths[last],
        space - first.width,
        &measure,
    );

    // Even the minimal current label overflows: give it room taken from the first crumb.
    if first.width + current.width > space {
        first = fit_breadcrumb(&breadcrumbs[0], widths[0], space - current.width, &measure);
    }

    segments[0] = first;
    segments.push(current);

    BreadcrumbLayout { segments, omitted }
}

fn draw_breadcrumb(
    ui: &mut egui::Ui,
    crumb: &RenderedBreadcrumb,
    palette: &ThemePalette,
) -> egui::Response {
    let inner = egui::Frame::NONE
        .fill(egui::Color32::TRANSPARENT)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .corner_radius(egui::CornerRadius::same(palette.medium_radius))
        .show(ui, |ui| {
            let resp = ui.add(
                egui::Label::new(
                    egui::RichText::new(&crumb.label)
                        .size(palette.text_size)
                        .color(palette.text_header_section),
                )
                .selectable(false)
                .sense(egui::Sense::click()),
            );

            if crumb.truncated {
                resp.on_hover_text(&crumb.full_label)
            } else {
                resp
            }
        });

    inner.response.union(inner.inner)
}

fn handle_breadcrumb_hover(ui: &egui::Ui, resp: &egui::Response) {
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn handle_breadcrumb_click(
    resp: &egui::Response,
    crumb: &RenderedBreadcrumb,
    drag_active: bool,
    action: &mut ItemViewerNavBarAction,
) {
    // The ellipsis crumb opens the omitted-folders popup instead of navigating.
    if drag_active || crumb.is_ellipsis || !resp.clicked() {
        return;
    }

    action.nav_to = Some(crumb.path.clone());
}

/// Dropdown under the ellipsis crumb listing the omitted ancestors in path order.
/// Picking one navigates there.
fn show_omitted_breadcrumbs_popup(
    resp: &egui::Response,
    popup_id: egui::Id,
    omitted: &[Breadcrumb],
    palette: &ThemePalette,
    drag_active: bool,
    action: &mut ItemViewerNavBarAction,
) {
    let mut popup = egui::Popup::menu(resp)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick);

    if drag_active {
        popup = popup.open_memory(None::<egui::SetOpenCommand>);
    }

    popup.show(|ui| {
        apply_eden_visual_overrides(ui, palette);
        apply_eden_text_overrides(ui, palette);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        egui::ScrollArea::vertical()
            .max_height(ui.ctx().content_rect().height() * 0.6)
            .show(ui, |ui| {
                for crumb in omitted {
                    let label = format!("{}  {}", regular::FOLDER_SIMPLE, crumb.label);
                    let button = ui.add(egui::Button::new(
                        egui::RichText::new(label).color(palette.text_normal),
                    ));

                    if button.clicked() {
                        action.nav_to = Some(crumb.path.clone());
                        ui.close();
                    }
                }
            });
    });
}

/// Highlights `crumb` while files are dragged over it and records it as the drop target on
/// release. The ellipsis crumb's `path` is the deepest omitted folder (the last popup entry).
fn handle_breadcrumb_drag(
    ui: &egui::Ui,
    resp: &egui::Response,
    crumb: &RenderedBreadcrumb,
    palette: &ThemePalette,
    drag_active: bool,
    hovered_target_ref: Option<&PathBuf>,
    pointer_pos: Option<egui::Pos2>,
    pointer_released: bool,
    breadcrumb_drop_target: &mut Option<PathBuf>,
    action: &mut ItemViewerNavBarAction,
) {
    if !drag_active {
        return;
    }

    let hovered = hovered_target_ref
        .map(|target| *target == crumb.path)
        .unwrap_or_else(|| {
            pointer_pos
                .map(|pointer| resp.rect.contains(pointer))
                .unwrap_or(false)
        });

    if !hovered {
        return;
    }

    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            ui.id().with("breadcrumb_drop_bg").with(&crumb.path),
        ))
        .with_clip_rect(ui.clip_rect());

    painter.rect_filled(
        resp.rect,
        egui::CornerRadius::same(palette.medium_radius),
        palette.primary_hover,
    );

    if pointer_released {
        *breadcrumb_drop_target = Some(crumb.path.clone());
        action.move_files_to_breadcrumb_dir_rect = Some(resp.rect);
    }
}

fn merge_toolbar_action(action: &mut ItemViewerNavBarAction, toolbar: ItemViewerNavBarAction) {
    if action.nav.is_none() {
        action.nav = toolbar.nav;
    }

    if action.nav_to.is_none() {
        action.nav_to = toolbar.nav_to;
    }

    action.refresh_current_directory |= toolbar.refresh_current_directory;
    action.create_folder |= toolbar.create_folder;
    action.create_file |= toolbar.create_file;
    action.add_favorite |= toolbar.add_favorite;
    action.remove_favorite |= toolbar.remove_favorite;
    action.toggle_gallery |= toolbar.toggle_gallery;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const CHAR_WIDTH: f32 = 10.0;
    const SEPARATOR: f32 = 20.0;

    /// Monospace stand-in for egui text measurement.
    fn measure(text: &str) -> f32 {
        text.chars().count() as f32 * CHAR_WIDTH
    }

    fn crumbs(labels: &[&str]) -> Vec<Breadcrumb> {
        let mut path = PathBuf::new();
        labels
            .iter()
            .map(|label| {
                path.push(label);
                Breadcrumb {
                    label: (*label).to_owned(),
                    path: path.clone(),
                }
            })
            .collect()
    }

    fn layout(labels: &[&str], available_width: f32) -> BreadcrumbLayout {
        compute_breadcrumb_layout(&crumbs(labels), available_width, SEPARATOR, measure)
    }

    fn labels(layout: &BreadcrumbLayout) -> Vec<&str> {
        layout.segments.iter().map(|c| c.label.as_str()).collect()
    }

    fn omitted_labels(layout: &BreadcrumbLayout) -> Vec<&str> {
        layout.omitted.iter().map(|c| c.label.as_str()).collect()
    }

    fn total_width(layout: &BreadcrumbLayout) -> f32 {
        layout.segments.iter().map(|c| c.width).sum::<f32>()
            + SEPARATOR * layout.segments.len().saturating_sub(1) as f32
    }

    fn width_of(labels: &[&str]) -> f32 {
        labels
            .iter()
            .map(|label| measure(label) + BREADCRUMB_ITEM_PADDING)
            .sum::<f32>()
            + SEPARATOR * labels.len().saturating_sub(1) as f32
    }

    const DEEP: [&str; 6] = ["C:", "Users", "someone", "Documents", "Projects", "Eden"];

    #[test]
    fn everything_fits_without_ellipsis() {
        let result = layout(&DEEP, width_of(&DEEP));

        assert_eq!(labels(&result), DEEP);
        assert!(result.omitted.is_empty());
        assert!(
            result
                .segments
                .iter()
                .all(|c| !c.truncated && !c.is_ellipsis)
        );
    }

    #[test]
    fn empty_path_yields_nothing() {
        let result = layout(&[], 100.0);

        assert!(result.segments.is_empty());
        assert!(result.omitted.is_empty());
    }

    #[test]
    fn single_crumb_is_truncated_only_when_too_wide() {
        assert_eq!(labels(&layout(&["C:"], 100.0)), ["C:"]);

        let result = layout(&["Very long portable device"], 100.0);

        assert_eq!(result.segments.len(), 1);
        assert!(result.segments[0].truncated);
        assert!(result.segments[0].label.ends_with(BREADCRUMB_ELLIPSIS));
        assert_eq!(result.segments[0].full_label, "Very long portable device");
        assert!(result.segments[0].width <= 100.0);
    }

    #[test]
    fn two_crumbs_truncate_the_current_folder() {
        let names = ["C:", "A folder with a very long name"];
        let result = layout(&names, 200.0);

        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].label, "C:");
        assert!(!result.segments.iter().any(|c| c.is_ellipsis));
        assert!(result.segments[1].truncated);
        assert_eq!(result.segments[1].full_label, names[1]);
        assert!(total_width(&result) <= 200.0);
    }

    #[test]
    fn ancestors_fill_from_the_right_and_current_stays_whole() {
        // Room for C: > ... > Projects > Eden but not Documents.
        let width = width_of(&["C:", BREADCRUMB_ELLIPSIS, "Projects", "Eden"]) + 5.0;
        let result = layout(&DEEP, width);

        assert_eq!(labels(&result), ["C:", "...", "Projects", "Eden"]);
        assert_eq!(omitted_labels(&result), ["Users", "someone", "Documents"]);
        assert!(result.segments.iter().all(|c| !c.truncated));
        assert!(total_width(&result) <= width);
    }

    #[test]
    fn ellipsis_targets_the_deepest_omitted_folder() {
        let width = width_of(&["C:", BREADCRUMB_ELLIPSIS, "Projects", "Eden"]);
        let result = layout(&DEEP, width);
        let ellipsis = &result.segments[1];

        assert!(ellipsis.is_ellipsis);
        assert_eq!(ellipsis.path, result.omitted.last().unwrap().path);
        assert_eq!(ellipsis.path, PathBuf::from_iter(&DEEP[..4]));
    }

    #[test]
    fn current_survives_when_first_and_current_overflow() {
        // The old layout truncated the "..." crumb itself and lost the current folder.
        let names = ["C:", "Users", "An extremely long current folder name"];
        let width = 200.0;
        let result = layout(&names, width);

        assert_eq!(result.segments.len(), 3);
        assert_eq!(result.segments[0].label, "C:");
        assert!(result.segments[1].is_ellipsis);
        let current = result.segments.last().unwrap();
        assert!(!current.is_ellipsis);
        assert!(current.truncated);
        assert_eq!(current.full_label, names[2]);
        assert_eq!(current.path, PathBuf::from_iter(names));
        assert_eq!(omitted_labels(&result), ["Users"]);
        assert!(total_width(&result) <= width);
    }

    #[test]
    fn extreme_width_keeps_first_and_current() {
        for width in [0.0, 10.0, 40.0, 80.0, 120.0] {
            let result = layout(&DEEP, width);

            assert_eq!(result.segments.len(), 3, "width {width}");
            assert_eq!(result.segments[0].full_label, "C:");
            assert!(result.segments[1].is_ellipsis);
            assert_eq!(result.segments[2].full_label, "Eden");
            assert_eq!(result.segments[2].path, PathBuf::from_iter(DEEP));
        }
    }

    #[test]
    fn long_first_crumb_shrinks_before_current_overflows() {
        // Portable device names and UNC shares can make the first crumb very wide.
        let names = [r"\\fileserver\departments", "Shared", "Reports"];
        let width = 260.0;
        let result = layout(&names, width);

        let first = &result.segments[0];
        let current = result.segments.last().unwrap();
        assert!(first.truncated);
        assert_eq!(first.full_label, names[0]);
        assert!(result.segments[1].is_ellipsis);
        assert_eq!(current.label, "Reports");
        assert!(!current.truncated);
        assert!(total_width(&result) <= width);

        // Narrower than even the minimal `\... > ... > R...`: overflow minimally, keep both.
        let width = 160.0;
        let result = layout(&names, width);
        let current = result.segments.last().unwrap();
        assert_eq!(result.segments.len(), 3);
        assert_eq!(result.segments[0].full_label, names[0]);
        assert!(result.segments[0].truncated);
        assert!(!current.is_ellipsis);
        assert_eq!(current.full_label, "Reports");
        assert_eq!(
            total_width(&result),
            width_of(&[r"\...", BREADCRUMB_ELLIPSIS, "R..."])
        );
    }

    #[test]
    fn long_first_and_long_current_share_the_space() {
        let names = [r"\\fileserver\departments", "Quarterly reports 2024"];
        let width = 300.0;
        let result = layout(&names, width);

        assert_eq!(result.segments.len(), 2);
        assert!(result.segments.iter().all(|c| c.truncated));
        assert!(result.segments[1].width >= (width - SEPARATOR) / 2.0 - CHAR_WIDTH);
        assert!(total_width(&result) <= width);
    }

    #[test]
    fn short_first_crumb_is_never_truncated() {
        for width in [0.0, 30.0, 60.0, 100.0] {
            let result = layout(&["C:", "Users", "Quarterly reports 2024"], width);

            assert_eq!(result.segments[0].label, "C:", "width {width}");
            assert!(result.segments.last().unwrap().truncated, "width {width}");
        }
    }

    #[test]
    fn layout_fits_whenever_minimal_form_fits() {
        let minimal = width_of(&["C:", BREADCRUMB_ELLIPSIS, "E..."]);

        for step in 0..=60 {
            let width = minimal + step as f32 * 5.0;
            let result = layout(&DEEP, width);

            assert!(total_width(&result) <= width, "width {width}");
            assert_eq!(result.segments.first().unwrap().label, "C:");
            assert_eq!(result.segments.last().unwrap().full_label, "Eden");

            // First crumb + omitted + visible ancestors + current rebuild the whole path.
            let mut visible: Vec<&str> = result
                .segments
                .iter()
                .filter(|c| !c.is_ellipsis)
                .map(|c| c.full_label.as_str())
                .collect();
            let rest = visible.split_off(1);
            visible.extend(omitted_labels(&result));
            visible.extend(rest);
            assert_eq!(visible, DEEP, "width {width}");
        }
    }

    #[test]
    fn truncation_finds_longest_fitting_prefix() {
        let text = "Quarterly reports 2024";

        for max_width in [40.0, 55.0, 90.0, 150.0, 210.0] {
            let (label, width) = truncate_label_to_width(text, max_width, &measure).unwrap();
            let expected = (1..text.chars().count())
                .rev()
                .map(|n| {
                    let prefix: String = text.chars().take(n).collect();
                    format!("{}{BREADCRUMB_ELLIPSIS}", prefix.trim_end())
                })
                .find(|candidate| measure(candidate) <= max_width)
                .unwrap_or_else(|| format!("Q{BREADCRUMB_ELLIPSIS}"));

            assert_eq!(label, expected, "max width {max_width}");
            assert_eq!(width, measure(&label));
        }
    }

    #[test]
    fn truncation_keeps_one_char_and_handles_multibyte() {
        // Japanese for "document": six 3-byte chars.
        let text = "\u{30c9}\u{30ad}\u{30e5}\u{30e1}\u{30f3}\u{30c8}";

        let (label, _) = truncate_label_to_width(text, 0.0, &measure).unwrap();
        assert_eq!(label, "\u{30c9}...");

        let (label, _) = truncate_label_to_width(text, 60.0, &measure).unwrap();
        assert_eq!(label, "\u{30c9}\u{30ad}\u{30e5}...");

        assert!(truncate_label_to_width("X", 0.0, &measure).is_none());
    }

    #[test]
    fn truncation_uses_logarithmic_measurements() {
        let text = "x".repeat(1024);
        let calls = Cell::new(0);
        let counting = |s: &str| {
            calls.set(calls.get() + 1);
            measure(s)
        };

        truncate_label_to_width(&text, 5000.0, &counting).unwrap();

        assert!(calls.get() <= 11, "{} measurements", calls.get());
    }

    #[cfg(windows)]
    #[test]
    fn builds_drive_and_unc_breadcrumbs() {
        let drive = build_breadcrumbs(Path::new(r"C:\Users\someone"));
        let drive: Vec<_> = drive
            .iter()
            .map(|c| (c.label.as_str(), c.path.clone()))
            .collect();
        assert_eq!(
            drive,
            [
                ("C:", PathBuf::from("C:")),
                ("Users", PathBuf::from(r"C:\Users")),
                ("someone", PathBuf::from(r"C:\Users\someone")),
            ]
        );

        let unc = build_breadcrumbs(Path::new(r"\\server\share\dir"));
        let unc: Vec<_> = unc.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(unc, [r"\\server\share", "dir"]);

        assert_eq!(build_breadcrumbs(Path::new(r"C:\")).len(), 1);
    }
}
