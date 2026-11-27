//! Visual styles shared by the wide Settings window and the narrow player panel.
use super::theme;
use crate::player::options::Section;
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, UiBuilder, UiStackInfo, WidgetInfo, WidgetType, pos2, vec2,
};

const SCOPE: &str = "classic_settings";
const MAIN_WINDOW: &str = "classic_main_settings";
const PILL: CornerRadius = CornerRadius::same(255);
const CATEGORY_RAIL_WIDTH: f32 = 122.;

pub(super) fn active(ui: &Ui) -> bool {
    ui.stack()
        .iter()
        .any(|layer| layer.info.tags.contains(SCOPE))
}

pub(super) fn window_surface(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(17, 18, 21)
    } else {
        Color32::from_rgb(245, 246, 248)
    }
}

fn group_surface(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(26, 28, 32)
    } else {
        Color32::WHITE
    }
}

fn standalone(ui: &Ui) -> bool {
    ui.stack()
        .iter()
        .any(|layer| layer.info.tags.contains(MAIN_WINDOW))
}

fn selected(dark: bool) -> Color32 {
    Color32::from_gray(if dark { 244 } else { 37 })
}

pub(super) fn style(ui: &mut Ui) {
    let dark = ui.visuals().dark_mode;
    let main = standalone(ui);
    let colors = theme::palette(ui);
    let (chip, hover) = match (main, dark) {
        (true, true) => (Color32::from_rgb(46, 48, 54), Color32::from_rgb(59, 62, 70)),
        (true, false) => (
            Color32::from_rgb(236, 238, 242),
            Color32::from_rgb(224, 227, 233),
        ),
        (false, _) => (
            Color32::from_gray(if dark { 40 } else { 237 }),
            colors.hover,
        ),
    };
    let focus = Stroke::new(1.5, theme::focus_color(ui));
    let style = ui.style_mut();
    style.spacing.item_spacing = if main { vec2(10., 7.) } else { vec2(8., 6.) };
    style.spacing.interact_size.y = if main { 34. } else { 30. };
    style.spacing.button_padding = vec2(11., 6.);
    style.spacing.icon_width = 14.;
    style.spacing.icon_spacing = 7.;
    style.visuals.indent_has_left_vline = false;
    style.visuals.extreme_bg_color = chip;
    style.visuals.selection.bg_fill = hover;
    let widgets = &mut style.visuals.widgets;
    for (state, fill, border) in [
        (&mut widgets.inactive, chip, Stroke::NONE),
        (&mut widgets.hovered, hover, Stroke::NONE),
        (&mut widgets.active, hover, focus),
        (&mut widgets.open, hover, Stroke::NONE),
    ] {
        state.corner_radius = PILL;
        state.expansion = 0.;
        state.bg_stroke = border;
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
    }
}

pub(super) fn pill(ui: &mut Ui, label: &str, selected_choice: bool, quiet: bool) -> Response {
    pill_button(ui, label, selected_choice, quiet, false)
}

fn pill_button(
    ui: &mut Ui,
    label: &str,
    selected_choice: bool,
    quiet: bool,
    full_width: bool,
) -> Response {
    let dark = ui.visuals().dark_mode;
    let colors = theme::palette(ui);
    let mut widget = egui::Button::new(if selected_choice {
        theme::strong(
            label,
            13.,
            if dark {
                Color32::from_gray(24)
            } else {
                Color32::WHITE
            },
        )
    } else {
        theme::text(label, 13., colors.text)
    })
    .corner_radius(PILL);
    if selected_choice {
        widget = widget.fill(selected(dark));
    } else if quiet {
        widget = widget.frame_when_inactive(false);
    }
    if full_width {
        widget = widget.right_text(());
    }
    add_pill(ui, widget, selected_choice)
}

fn add_pill(ui: &mut Ui, widget: egui::Button<'_>, selected_choice: bool) -> Response {
    let response = ui
        .add(widget)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            PILL,
            Stroke::new(1.5, theme::focus_color(ui)),
            StrokeKind::Inside,
        );
    } else if selected_choice && response.hovered() {
        let colors = theme::palette(ui);
        ui.painter()
            .rect_filled(response.rect, PILL, colors.on_primary.gamma_multiply(0.08));
    }
    response
}

pub(super) fn page(
    ui: &mut Ui,
    section: &mut Section,
    sidebar: bool,
    body: impl FnOnce(&mut Ui, Section),
) {
    let wide = !sidebar && ui.available_width() >= 635.;
    let info = UiStackInfo::default().with_tag(SCOPE);
    let info = if sidebar {
        info
    } else {
        info.with_tag(MAIN_WINDOW)
    };
    ui.scope_builder(UiBuilder::new().ui_stack_info(info), |ui| {
        style(ui);
        let sections = &Section::ALL;
        let form = ui.id().with("section_content");
        if wide {
            ui.horizontal_top(|ui| {
                let height = ui.available_height().max(1.);
                ui.allocate_ui_with_layout(
                    vec2(CATEGORY_RAIL_WIDTH, height),
                    Layout::top_down_justified(Align::Min),
                    |ui| {
                        ui.set_width(CATEGORY_RAIL_WIDTH);
                        ui.add_space(9.);
                        egui::ScrollArea::vertical()
                            .id_salt("classic_categories_wide")
                            .max_height(ui.available_height().max(1.))
                            .show(ui, |ui| categories(ui, section, sections, true, false));
                    },
                );
                ui.add_space(6.);
                ui.vertical(|ui| form_body(ui, form, *section, true, body));
            });
        } else {
            let fully_visible = ui.clip_rect().width() + 1. >= ui.available_width();
            ui.scope(|ui| {
                ui.style_mut().always_scroll_the_only_direction = true;
                egui::ScrollArea::horizontal()
                    .id_salt("classic_categories_narrow")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| categories(ui, section, sections, false, fully_visible))
                    });
            });
            ui.separator();
            form_body(ui, form, *section, false, body);
        }
    });
}

fn categories(
    ui: &mut Ui,
    section: &mut Section,
    sections: &[Section],
    wide: bool,
    scroll_selected: bool,
) {
    let mut requested = None;
    let mut controls = Vec::with_capacity(sections.len());
    for (index, &category) in sections.iter().enumerate() {
        let current = *section == category;
        let response = ui
            .push_id(category.title(), |ui| {
                pill_button(ui, category.title(), current, wide, wide)
            })
            .inner;
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::SelectableLabel,
                ui.is_enabled(),
                current,
                category.title(),
            )
        });
        if response.clicked() {
            *section = category;
            response.request_focus();
        }
        if response.has_focus() {
            let previous = if wide {
                egui::Key::ArrowUp
            } else {
                egui::Key::ArrowLeft
            };
            let next = if wide {
                egui::Key::ArrowDown
            } else {
                egui::Key::ArrowRight
            };
            requested = ui.input_mut(|input| {
                let modifiers = egui::Modifiers::NONE;
                if input.consume_key(modifiers, previous) {
                    Some(index.saturating_sub(1))
                } else if input.consume_key(modifiers, next) {
                    Some((index + 1).min(sections.len() - 1))
                } else {
                    None
                }
            });
        }
        let last = ui.id().with("classic_active_category");
        if current
            && scroll_selected
            && ui.data_mut(|data| {
                let old = data.get_temp::<usize>(last);
                data.insert_temp(last, index);
                old != Some(index)
            })
        {
            response.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
        }
        controls.push(response.id);
    }
    if let Some(index) = requested {
        *section = sections[index];
        ui.memory_mut(|memory| {
            memory.move_focus(egui::FocusDirection::None);
            memory.request_focus(controls[index]);
        });
    }
}

fn form_body(
    ui: &mut Ui,
    id: egui::Id,
    section: Section,
    wide: bool,
    body: impl FnOnce(&mut Ui, Section),
) {
    // Keep control focus and scroll positions when the category rail changes orientation.
    ui.scope_builder(UiBuilder::new().id(id), |ui| {
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating_width = 4.;
        scroll.floating_allocated_width = 8.;
        scroll.dormant_handle_opacity = 0.45;
        let height = ui.available_height().max(1.);
        let title_size = if wide {
            34.
        } else if standalone(ui) {
            25.
        } else {
            23.
        };
        egui::ScrollArea::vertical()
            .id_salt(section.title())
            .max_height(height)
            .min_scrolled_height(height.min(48.))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let contents = |ui: &mut Ui| {
                    let colors = theme::palette(ui);
                    ui.add_space(if wide { 5. } else { 3. });
                    ui.label(theme::strong(section.title(), title_size, colors.text));
                    ui.add_space(6.);
                    body(ui, section);
                };
                if standalone(ui) {
                    let inset = if wide { 12 } else { 6 };
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(inset, 0))
                        .show(ui, contents);
                } else {
                    contents(ui);
                }
            });
    });
}

pub(super) fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    let id = ui.id().with(("setting_group", title));
    let colors = theme::palette(ui);
    if standalone(ui) {
        egui::Frame::new()
            .fill(group_surface(ui.visuals()))
            .stroke(Stroke::new(1., colors.line.gamma_multiply(0.65)))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(14, 9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::strong(title, 15., colors.text));
                ui.scope_builder(UiBuilder::new().id(id), body);
            });
    } else {
        ui.separator();
        ui.label(theme::strong(title, 14., colors.text));
        ui.scope_builder(UiBuilder::new().id(id), body);
    }
    ui.add_space(6.);
}

pub(super) fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui, egui::Id) -> R) -> R {
    let width = ui.available_width();
    let row_height = ui.spacing().interact_size.y;
    let control_width = (width * 0.46).min(180.);
    let label_width = (width - control_width - ui.spacing().item_spacing.x).max(1.);
    ui.allocate_ui_with_layout(
        vec2(width, row_height),
        Layout::left_to_right(Align::Center),
        |ui| {
            let caption = ui
                .allocate_ui_with_layout(
                    vec2(label_width, row_height),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_width(label_width);
                        ui.add(egui::Label::new(egui::RichText::new(label).size(13.)).wrap())
                    },
                )
                .inner;
            ui.allocate_ui_with_layout(
                vec2(control_width, row_height),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(control_width);
                    ui.spacing_mut().slider_width = (control_width - 58.).max(18.);
                    control(ui, caption.id)
                },
            )
            .inner
        },
    )
    .inner
}

pub(super) fn switch(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let colors = theme::palette(ui);
    let width = ui.available_width().max(48.);
    let galley = ui.painter().layout(
        theme::visual(label).into_owned(),
        FontId::proportional(13.),
        colors.text,
        (width - 48.).max(1.),
    );
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(width, (galley.size().y + 8.).max(32.)), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *value, label));
    let dark = ui.visuals().dark_mode;
    let (track, thumb): (u8, u8) = match (dark, *value) {
        (true, false) => (80, 154),
        (true, true) => (116, 244),
        (false, false) => (195, 109),
        (false, true) => (164, 38),
    };
    let rail = Rect::from_center_size(rect.right_center() - vec2(22., 0.), vec2(34., 14.));
    let thumb_center = rail.center() + vec2(if *value { 9. } else { -9. }, 0.);
    let tone = Color32::from_gray(if response.hovered() {
        thumb.saturating_add(12)
    } else {
        thumb
    });
    let painter = ui.painter();
    painter.rect_filled(rail, PILL, Color32::from_gray(track));
    painter.circle_filled(thumb_center, 10., tone);
    if label == "Autoplay the next video" {
        let ink = if tone.r() > 128 {
            Color32::from_gray(27)
        } else {
            Color32::WHITE
        };
        if *value {
            painter.add(Shape::convex_polygon(
                vec![
                    thumb_center + vec2(-2., -4.),
                    thumb_center + vec2(4., 0.),
                    thumb_center + vec2(-2., 4.),
                ],
                ink,
                Stroke::NONE,
            ));
        } else {
            for dx in [-1.8, 1.8] {
                painter.vline(
                    thumb_center.x + dx,
                    (thumb_center.y - 3.)..=(thumb_center.y + 3.),
                    Stroke::new(1.4, ink),
                );
            }
        }
    }
    if response.has_focus() {
        painter.rect_stroke(
            rail.expand(5.),
            PILL,
            Stroke::new(1.5, theme::focus_color(ui)),
            StrokeKind::Outside,
        );
    }
    painter.galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.),
        galley,
        colors.text,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn combo<R>(
    ui: &mut Ui,
    label: &str,
    current: String,
    contents: impl FnOnce(&mut Ui, &mut theme::MenuRows) -> R,
) -> egui::InnerResponse<Option<R>> {
    let width = ui.available_width();
    let dark = ui.visuals().dark_mode;
    let ink = theme::palette(ui).text;
    let highlight = if dark {
        Color32::from_white_alpha(26)
    } else {
        Color32::from_black_alpha(15)
    };
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    for state in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        state.weak_bg_fill = highlight;
        state.bg_stroke = Stroke::NONE;
        state.corner_radius = CornerRadius::same(8);
    }
    let mut selected_from_menu = false;
    let combo = egui::ComboBox::from_id_salt(label)
        .selected_text(theme::text(current, 14., ink))
        .width(width)
        .height(250.)
        .truncate()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .icon(|ui, rect, visuals, open| {
            let center = rect.center();
            let down = if open { -1. } else { 1. };
            ui.painter().add(Shape::line(
                vec![
                    center + vec2(-5., -2.5 * down),
                    center + vec2(0., 2.5 * down),
                    center + vec2(5., -2.5 * down),
                ],
                Stroke::new(1.4, visuals.fg_stroke.color),
            ));
        })
        .popup_style(theme::menu_style(ui.ctx()))
        .show_ui(ui, |ui| {
            // Leave room for option labels even when the trigger sits in a narrow settings row.
            ui.set_min_width(184_f32.min((ui.ctx().content_rect().width() - 24.).max(1.)));
            let mut rows = theme::MenuRows::default();
            let result = contents(ui, &mut rows);
            selected_from_menu = ui.will_parent_close();
            if !selected_from_menu {
                rows.focus(ui);
            }
            result
        });
    let still_open = egui::ComboBox::is_open(ui.ctx(), combo.response.id);
    ui.ctx()
        .accesskit_node_builder(combo.response.id, |node| node.set_expanded(still_open));
    if combo.inner.is_some() && !still_open {
        // Selection and Escape return keyboard focus to the trigger.
        // An outside click can focus whatever was clicked next.
        let outside_click =
            ui.input(|input| input.pointer.any_click()) && !combo.response.clicked();
        if selected_from_menu || !outside_click {
            combo.response.request_focus();
        }
    }
    if combo.response.has_focus() && !still_open {
        ui.painter().rect_stroke(
            combo.response.rect,
            8,
            Stroke::new(1.5, theme::focus_color(ui)),
            StrokeKind::Inside,
        );
    }
    combo
}

pub(super) fn choice_item(ui: &mut Ui, label: &str, current: bool) -> Response {
    let colors = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, 8, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let galley = egui::WidgetText::from(theme::text(label, 14., colors.text)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.width() - 35. - 12.).max(1.),
        egui::TextStyle::Button,
    );
    let truncated = galley.elided;
    ui.painter().galley(
        pos2(rect.left() + 35., rect.center().y - galley.size().y * 0.5),
        galley,
        colors.text,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            8,
            Stroke::new(1.5, theme::focus_color(ui)),
            StrokeKind::Inside,
        );
    }
    if current {
        let p = rect.left_center() + vec2(19., 0.);
        ui.painter().add(Shape::line(
            vec![p + vec2(-6., 0.), p + vec2(-2., 4.), p + vec2(7., -5.)],
            Stroke::new(1.7, colors.text),
        ));
    }
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), current, label)
    });
    if truncated {
        response.on_hover_text(label)
    } else {
        response
    }
}

pub(super) fn close_button(ui: &mut Ui, label: &str) -> Response {
    let widget = egui::Button::new(egui::RichText::new("×").size(19.))
        .min_size(vec2(30., 30.))
        .corner_radius(PILL);
    let response = add_pill(ui, widget, false).on_hover_text(label);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response
}
