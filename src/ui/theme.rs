use std::{borrow::Cow, sync::Arc};

use eframe::egui::{
    self, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Pos2, Rect,
    Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::{model::Appearance, player::options::InterfaceTheme};

#[derive(Clone, Copy)]
pub struct Palette {
    pub back: Color32,
    pub side: Color32,
    pub card: Color32,
    pub well: Color32,
    pub hover: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub active: Color32,
    pub on_primary: Color32,
    primary_hover: Color32,
    danger_fill: Color32,
    danger_hover: Color32,
}

impl Palette {
    pub const DARK: Self = Self {
        back: Color32::from_gray(15),
        side: Color32::from_gray(15),
        card: Color32::from_gray(33),
        well: Color32::from_gray(39),
        hover: Color32::from_gray(52),
        line: Color32::from_gray(64),
        text: Color32::from_gray(241),
        muted: Color32::from_gray(170),
        accent: Color32::WHITE,
        active: Color32::from_gray(39),
        on_primary: Color32::from_gray(15),
        primary_hover: Color32::from_gray(217),
        danger_fill: Color32::from_rgb(110, 46, 49),
        danger_hover: Color32::from_rgb(137, 55, 60),
    };

    pub const LIGHT: Self = Self {
        back: Color32::from_gray(255),
        side: Color32::from_gray(255),
        card: Color32::WHITE,
        well: Color32::from_gray(245),
        hover: Color32::from_gray(229),
        line: Color32::from_gray(211),
        text: Color32::from_gray(15),
        muted: Color32::from_gray(96),
        accent: Color32::from_gray(15),
        active: Color32::from_gray(242),
        on_primary: Color32::WHITE,
        primary_hover: Color32::from_gray(49),
        danger_fill: Color32::from_rgb(255, 222, 222),
        danger_hover: Color32::from_rgb(249, 196, 196),
    };

    pub fn for_visuals(visuals: &egui::Visuals) -> Self {
        if visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

pub fn colors(ctx: &egui::Context, visuals: &egui::Visuals) -> Palette {
    interface_palette(visuals, interface_theme(ctx).glass_ui())
}

pub fn interface_theme(ctx: &egui::Context) -> InterfaceTheme {
    ctx.data(|data| data.get_temp(egui::Id::new("interface_theme")))
        .unwrap_or_default()
}

pub(super) fn solid_glass(ctx: &egui::Context) -> bool {
    interface_theme(ctx) == InterfaceTheme::GlassOpaque
        || (interface_theme(ctx).translucent() && glass_strength(ctx) == 0)
        || super::control_bar::DisplayMaterial::system(ctx)
            != super::control_bar::DisplayMaterial::Glass
}

pub fn glass_strength(ctx: &egui::Context) -> u8 {
    ctx.data(|d| d.get_temp(egui::Id::new("glass_strength")))
        .unwrap_or(crate::player::options::DEFAULT_GLASS_STRENGTH)
}

pub fn glass_amount(ctx: &egui::Context) -> f32 {
    f32::from(glass_strength(ctx)) / 100.
}

/// Controls share the window material, with a light surface treatment of their own.
pub fn control_fill(ui: &Ui, base: Color32) -> Color32 {
    if interface_theme(ui.ctx()).translucent() && !solid_glass(ui.ctx()) {
        base.gamma_multiply(1. - 0.55 * glass_amount(ui.ctx()))
    } else {
        base
    }
}

/// A panel that shares the window backdrop, including in the player and inspector.
pub fn sidebar_frame(horizontal: i8, vertical: i8) -> Frame {
    layer(Color32::TRANSPARENT, horizontal, vertical)
}

pub fn panel_frame(ui: &Ui, x: i8, y: i8) -> Frame {
    let colors = palette(ui);
    let fill = if menu_material(ui.ctx()) == super::control_bar::DisplayMaterial::Glass {
        control_fill(ui, colors.card)
    } else {
        colors.card
    };
    layer(fill, x, y).corner_radius(12)
}

pub fn form_group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    let colors = palette(ui);
    ui.label(strong(title, 13., colors.text));
    layer(control_fill(ui, colors.well), 12, 10)
        .corner_radius(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

pub fn disclosure_row(
    ui: &mut Ui,
    label: &str,
    state: &mut egui::collapsing_header::CollapsingState,
) {
    let colors = palette(ui);
    let width = ui.available_width();
    let galley = ui.painter().layout(
        label.into(),
        FontId::proportional(13.),
        colors.text,
        (width - 40.).max(1.),
    );
    let (_, rect) = ui.allocate_space(vec2(width, (galley.size().y + 12.).max(32.)));
    let mut response = ui
        .interact(rect, state.id(), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        state.set_open(!state.is_open());
        response.mark_changed();
    }
    let open = state.is_open();
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::CollapsingHeader, ui.is_enabled(), label));
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_expanded(open));

    let p = ui.painter();
    let radius = if super::classic_settings::active(ui) {
        255
    } else {
        8
    };
    if response.is_pointer_button_down_on() {
        p.rect_filled(rect, radius, control_fill(ui, colors.hover));
    } else if response.hovered() {
        p.rect_filled(rect, radius, control_fill(ui, colors.well));
    }
    if response.has_focus() {
        p.rect_stroke(
            rect,
            radius,
            Stroke::new(1., focus_color(ui)),
            StrokeKind::Inside,
        );
    }
    p.galley(
        rect.left_center() + vec2(10., -galley.size().y / 2.),
        galley,
        colors.text,
    );
    let center = rect.right_center() - vec2(15., 0.);
    let points = if open {
        [vec2(-4., -2.), vec2(0., 2.), vec2(4., -2.)]
    } else {
        [vec2(-2., -4.), vec2(2., 0.), vec2(-2., 4.)]
    };
    p.add(Shape::line(
        points.map(|offset| center + offset).to_vec(),
        Stroke::new(1.4, colors.muted),
    ));
    p.hline(
        rect.x_range().shrink(10.),
        rect.bottom(),
        Stroke::new(1., colors.line.gamma_multiply(0.5)),
    );
}

pub(super) fn focus_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(145, 190, 255)
    } else {
        Color32::from_rgb(35, 95, 182)
    }
}

pub fn style_form(ui: &mut Ui) {
    if !interface_theme(ui.ctx()).glass_ui() {
        super::classic_settings::style(ui);
    }
}

pub fn setting_toggle(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    if !interface_theme(ui.ctx()).glass_ui() {
        return super::classic_settings::switch(ui, value, label);
    }
    if ui.layout().main_dir.is_horizontal() {
        return ui.checkbox(value, label);
    }
    let colors = palette(ui);
    let width = ui.available_width();
    let galley = ui.painter().layout(
        label.into(),
        FontId::proportional(13.),
        colors.text,
        (width - 52.).max(1.),
    );
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(width, galley.size().y.max(28.)), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *value, label));
    let rail = Rect::from_center_size(rect.right_center() - vec2(17., 0.), vec2(34., 20.));
    let fill = if *value {
        colors.accent
    } else {
        colors.muted.gamma_multiply(0.65)
    };
    ui.painter().rect_filled(rail, 10, fill);
    ui.painter().circle_filled(
        pos2(
            if *value {
                rail.right() - 10.
            } else {
                rail.left() + 10.
            },
            rail.center().y,
        ),
        7.,
        if *value {
            colors.on_primary
        } else {
            colors.back
        },
    );
    if response.has_focus() || response.hovered() {
        ui.painter().rect_stroke(
            rail.expand(2.),
            12,
            Stroke::new(1., colors.muted),
            StrokeKind::Outside,
        );
    }
    ui.painter().galley(
        rect.left_center() - vec2(0., galley.size().y / 2.),
        galley,
        colors.text,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Glass controls over video share a dark bezel and a high-contrast symbol.
pub fn media_badge(painter: &egui::Painter, rect: Rect, icon: Icon) {
    #[cfg(target_os = "macos")]
    if super::macos::media_buttons::request(painter, rect, icon) {
        return;
    }
    let radius = rect.height() * 0.5;
    let ctx = painter.ctx();
    let fill = if solid_glass(ctx) {
        Color32::from_gray(45)
    } else {
        Color32::from_rgba_unmultiplied(32, 34, 38, (245. - 110. * glass_amount(ctx)) as u8)
    };
    painter.rect(
        rect,
        radius as u8,
        fill,
        Stroke::new(1., Color32::from_white_alpha(68)),
        StrokeKind::Inside,
    );
    icon.paint(painter, rect.center(), rect.height() * 0.50, Color32::WHITE);
}

pub fn glass_tint(ctx: &egui::Context, color: Color32) -> Color32 {
    color.gamma_multiply(1. - glass_amount(ctx))
}

pub fn apply_style(ctx: &egui::Context, style: InterfaceTheme, strength: u8) {
    let strength = strength.min(100);
    if strength != glass_strength(ctx) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("glass_strength"), strength));
        ctx.request_repaint();
    }
    let key = egui::Id::new("interface_theme");
    if ctx.data(|d| d.get_temp::<InterfaceTheme>(key)) == Some(style) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(key, style));
    for (theme, base) in [
        (egui::Theme::Dark, egui::Visuals::dark()),
        (egui::Theme::Light, egui::Visuals::light()),
    ] {
        let palette = interface_palette(&base, style.glass_ui());
        let mut widgets = visuals(base, palette);
        if !style.glass_ui() {
            for state in [
                &mut widgets.widgets.inactive,
                &mut widgets.widgets.hovered,
                &mut widgets.widgets.active,
                &mut widgets.widgets.open,
            ] {
                state.corner_radius = 18.into();
                state.bg_stroke = Stroke::NONE;
                state.expansion = 0.;
            }
        }
        ctx.set_visuals_of(theme, widgets);
    }
    ctx.request_repaint();
}

fn interface_palette(visuals: &egui::Visuals, glass: bool) -> Palette {
    let mut palette = Palette::for_visuals(visuals);
    if glass {
        if visuals.dark_mode {
            palette.back = Color32::from_rgb(24, 25, 29);
            palette.side = palette.back;
            palette.well = Color32::from_rgb(47, 49, 56);
            palette.line = Color32::from_rgb(63, 65, 73);
            palette.card = Color32::from_rgb(39, 40, 45);
            palette.active = Color32::from_rgb(52, 57, 67);
        } else {
            palette.back = Color32::from_rgb(243, 244, 247);
            palette.side = palette.back;
            palette.well = Color32::from_rgb(231, 233, 239);
            palette.line = Color32::from_rgb(207, 210, 218);
            palette.card = Color32::from_rgb(253, 253, 255);
            palette.active = Color32::from_rgb(225, 230, 240);
        }
    }
    palette
}

pub fn palette(ui: &Ui) -> Palette {
    colors(ui.ctx(), ui.visuals())
}

pub(super) fn menu_material(ctx: &egui::Context) -> super::control_bar::DisplayMaterial {
    use super::control_bar::DisplayMaterial;
    let material = DisplayMaterial::system(ctx);
    if material == DisplayMaterial::Glass
        && (!interface_theme(ctx).translucent() || glass_strength(ctx) == 0)
    {
        DisplayMaterial::Opaque
    } else {
        material
    }
}

pub(super) fn slider_fill(ui: &Ui) -> Color32 {
    if interface_theme(ui.ctx()).glass_ui() {
        Color32::from_rgb(10, 132, 255)
    } else {
        Color32::from_gray(if ui.visuals().dark_mode { 244 } else { 37 })
    }
}

pub(super) fn slider(ui: &mut Ui, slider: egui::Slider<'_>) -> Response {
    slider_with_fill(ui, slider, slider_fill(ui))
}

pub(super) fn slider_with_fill(ui: &mut Ui, slider: egui::Slider<'_>, filled: Color32) -> Response {
    let ink = Color32::from_gray(if ui.visuals().dark_mode { 244 } else { 37 });
    let rail = Color32::from_gray(if ui.visuals().dark_mode { 80 } else { 195 });
    let rail_width = ui.spacing().slider_width;
    let response = ui
        .allocate_ui_with_layout(
            vec2(rail_width, 20.),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let style = ui.style_mut();
                style.spacing.interact_size.y = 20.;
                style.spacing.slider_rail_height = 4.;
                style.spacing.button_padding = vec2(6., 3.);
                style.visuals.selection.bg_fill = filled;
                let widgets = &mut style.visuals.widgets;
                widgets.inactive.bg_fill = rail;
                widgets.inactive.corner_radius = egui::CornerRadius::same(2);
                widgets.inactive.expansion = -2.5;
                widgets.inactive.fg_stroke = Stroke::new(2., ink);
                for state in [&mut widgets.hovered, &mut widgets.active] {
                    state.bg_fill = ink;
                    state.expansion = -1.;
                    state.fg_stroke = Stroke::new(1., ink);
                }
                ui.add(
                    slider
                        .trailing_fill(true)
                        .handle_shape(egui::style::HandleShape::Circle),
                )
            },
        )
        .inner;
    if response.drag_started_by(egui::PointerButton::Primary)
        && response
            .interact_pointer_pos()
            .is_some_and(|point| point.x <= response.rect.left() + rail_width)
    {
        response.request_focus();
    }
    // YouTube menus indicate focus with the slider's larger, brighter thumb.
    if response.has_focus()
        && (interface_theme(ui.ctx()).glass_ui() || !ui.stack().contained_in(egui::UiKind::Menu))
    {
        ui.painter().rect_stroke(
            response.rect.expand(2.).intersect(ui.clip_rect()),
            8,
            Stroke::new(1., focus_color(ui)),
            StrokeKind::Inside,
        );
    }
    response
}

fn youtube_menu_style(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.item_spacing.y = 0.;
    style.spacing.button_padding = vec2(12., 6.);
    style.spacing.interact_size.y = 40.;
    let visuals = &mut style.visuals;
    visuals.menu_corner_radius = 12.into();
    visuals.window_fill = Color32::from_gray(if visuals.dark_mode { 35 } else { 255 });
    visuals.window_stroke = Stroke::NONE;
    let highlight = if visuals.dark_mode {
        Color32::from_white_alpha(26)
    } else {
        Color32::from_black_alpha(15)
    };
    for state in [
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        state.weak_bg_fill = highlight;
        state.corner_radius = 8.into();
        state.expansion = 0.;
    }
}

pub(super) fn menu_style(ctx: &egui::Context) -> egui::style::StyleModifier {
    use super::control_bar::DisplayMaterial;

    let material = menu_material(ctx);
    let glass = interface_theme(ctx).glass_ui();
    if !glass && material != DisplayMaterial::HighContrast {
        return egui::style::StyleModifier::new(youtube_menu_style);
    }
    egui::style::StyleModifier::new(move |style: &mut egui::Style| {
        egui::containers::menu::menu_style(style);
        style.spacing.interact_size.y = 28.0;
        style.spacing.item_spacing.y = 2.0;
        style.spacing.button_padding = vec2(9.0, 4.0);
        style.spacing.menu_margin = egui::Margin::same(6);
        let visuals = &mut style.visuals;
        let colors = interface_palette(visuals, glass);
        let dark = visuals.dark_mode;
        let contrast = if dark { Color32::WHITE } else { Color32::BLACK };
        visuals.menu_corner_radius = 10.into();
        visuals.window_fill = if material == DisplayMaterial::Glass && !cfg!(target_os = "macos") {
            if dark {
                Color32::from_rgba_unmultiplied(30, 30, 30, 246)
            } else {
                Color32::from_rgba_unmultiplied(252, 252, 252, 246)
            }
        } else {
            colors.card
        };
        visuals.window_stroke = Stroke::new(
            1.0,
            if material == DisplayMaterial::HighContrast {
                contrast
            } else {
                colors.line
            },
        );
        for widgets in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widgets.corner_radius = 5.into();
            widgets.expansion = 0.0;
        }
        visuals.widgets.hovered.weak_bg_fill = colors.hover;
        visuals.widgets.hovered.bg_fill = colors.hover;
        visuals.widgets.active.weak_bg_fill = colors.active;
        visuals.widgets.active.bg_fill = colors.active;
        visuals.widgets.active.bg_stroke = Stroke::new(
            1.0,
            if material == DisplayMaterial::HighContrast {
                contrast
            } else {
                colors.accent
            },
        );
        visuals.widgets.open.weak_bg_fill = colors.hover;
    })
}

pub(super) const FOLDER_GLASS_RADIUS: u8 = 14;

pub(super) fn folder_popup<R>(
    anchor: &Response,
    native_glass: bool,
    title: &str,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    use super::control_bar::DisplayMaterial;

    let ctx = &anchor.ctx;
    let material = menu_material(ctx);
    let glass_ui = interface_theme(ctx).glass_ui();
    let popup = egui::Popup::menu(anchor).style(menu_style(ctx));
    popup
        .gap(4.)
        .width(240.)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .frame(Frame::NONE)
        .show(|ui| {
            if ui.is_sizing_pass() {
                ctx.request_discard("size folder popup");
            }
            let colors = palette(ui);
            let fill = if native_glass {
                glass_tint(ctx, colors.card)
            } else if material == DisplayMaterial::Glass && !cfg!(target_os = "macos") {
                colors.card.gamma_multiply(1. - 0.08 * glass_amount(ctx))
            } else {
                colors.card
            };
            let style = ui.style_mut();
            style.visuals.widgets.hovered.weak_bg_fill = colors.hover;
            style.visuals.widgets.active.weak_bg_fill = colors.active;
            style.visuals.widgets.hovered.corner_radius = if glass_ui { 7 } else { 5 }.into();
            style.visuals.widgets.active.corner_radius =
                style.visuals.widgets.hovered.corner_radius;
            ui.set_opacity(1.0);
            let mut frame = Frame::popup(ui.style())
                .fill(fill)
                .corner_radius(if glass_ui { FOLDER_GLASS_RADIUS } else { 10 });
            if native_glass {
                frame = frame
                    .stroke(Stroke::NONE)
                    .shadow(egui::epaint::Shadow::NONE);
            } else if material != DisplayMaterial::HighContrast {
                frame = frame.stroke(Stroke::new(1., colors.line));
            }
            frame
                .show(ui, |ui| {
                    ui.set_width(228.);
                    ui.horizontal(|ui| {
                        ui.add_space(9.);
                        ui.label(strong(title, 12., colors.muted));
                    });
                    content(ui)
                })
                .inner
        })
}

pub(super) fn danger_text(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(255, 150, 145)
    } else {
        Color32::from_rgb(165, 32, 25)
    }
}

pub(super) fn context_menu<R>(
    response: &Response,
    width: f32,
    content: impl FnOnce(&mut Ui, &mut MenuRows) -> R,
) -> Option<egui::InnerResponse<R>> {
    let mut popup = egui::Popup::context_menu(response);
    if control_click(response) {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    popup
        .style(menu_style(&response.ctx))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(width)
        .frame(Frame::NONE)
        .show(|ui| {
            if ui.is_sizing_pass() {
                ui.ctx().request_discard("size context menu");
            }
            ui.set_opacity(1.0);
            Frame::popup(ui.style())
                .show(ui, |ui| {
                    ui.set_min_width(width - 14.0);
                    let mut rows = MenuRows::default();
                    let result = content(ui, &mut rows);
                    rows.focus(ui);
                    result
                })
                .inner
        })
}

pub(super) fn control_click(response: &Response) -> bool {
    cfg!(target_os = "macos")
        && response.clicked_by(egui::PointerButton::Primary)
        && response.ctx.input(|i| i.modifiers.ctrl)
}

#[derive(Default)]
pub(super) struct MenuRows {
    rows: Vec<(egui::Id, Rect)>,
    submenu_open: bool,
    preferred: Option<egui::Id>,
}

impl MenuRows {
    pub fn add(&mut self, response: Response) -> Response {
        if response.enabled() {
            self.rows.push((response.id, response.rect));
        }
        response
    }

    pub fn submenu(&mut self, ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui, &mut Self)) {
        let menu = ui.menu_button(label, |ui| {
            let mut rows = Self::default();
            content(ui, &mut rows);
            rows.focus(ui);
        });
        self.add(menu.response);
        self.submenu_open = menu.inner.is_some();
    }

    pub(super) fn prefer(&mut self, response: &Response) {
        if response.enabled() {
            self.preferred = Some(response.id);
        }
    }

    pub(super) fn focus(self, ui: &Ui) {
        use egui::{FocusDirection, Key, Modifiers};
        if self.submenu_open
            || self.rows.is_empty()
            || ui.is_sizing_pass()
            || ui.should_close()
            || ui.input(|input| input.key_pressed(Key::Escape))
        {
            return;
        }
        let ctx = ui.ctx();
        let direction = if super::consume_press(ctx, Key::ArrowDown, Modifiers::NONE)
            || super::consume_press(ctx, Key::Tab, Modifiers::NONE)
        {
            1
        } else if super::consume_press(ctx, Key::ArrowUp, Modifiers::NONE)
            || super::consume_press(ctx, Key::Tab, Modifiers::SHIFT)
        {
            -1
        } else {
            0
        };
        let target = ui.memory_mut(|memory| {
            let current = memory
                .focused()
                .and_then(|id| self.rows.iter().position(|(row, _)| *row == id));
            if current.is_some() && direction == 0 {
                return None;
            }
            let (id, rect) = current.map_or_else(
                || {
                    self.rows
                        .iter()
                        .copied()
                        .find(|(row, _)| Some(*row) == self.preferred)
                        .unwrap_or(self.rows[0])
                },
                |row| {
                    self.rows
                        [(row as isize + direction).rem_euclid(self.rows.len() as isize) as usize]
                },
            );
            memory.move_focus(FocusDirection::None);
            memory.request_focus(id);
            Some(rect)
        });
        if let Some(rect) = target.filter(|_| self.preferred.is_some()) {
            ui.scroll_to_rect(rect, None);
        }
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, data) in [
        (
            "Roboto",
            &include_bytes!("../../assets/Roboto-Regular.ttf")[..],
        ),
        (
            "Inter",
            &include_bytes!("../../assets/Inter-Regular.otf")[..],
        ),
        (
            "Inter Semibold",
            &include_bytes!("../../assets/Inter-SemiBold.otf")[..],
        ),
        (
            "Vazirmatn",
            &include_bytes!("../../assets/Vazirmatn-Regular.ttf")[..],
        ),
        (
            "Noto CJK SC",
            &include_bytes!("../../assets/NotoSansCJKsc-Regular.otf")[..],
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(data)));
    }
    let family = fonts.families.entry(FontFamily::Proportional).or_default();
    family.splice(
        0..0,
        ["Inter".into(), "Vazirmatn".into(), "Noto CJK SC".into()],
    );
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.extend(["Vazirmatn".into(), "Noto CJK SC".into()]);
    let family = fonts.families.entry(FontFamily::Proportional).or_default();
    let fallback = family.clone();
    fonts.families.insert(
        semibold(),
        ["Inter Semibold".to_owned()]
            .into_iter()
            .chain(fallback)
            .collect(),
    );
    fonts
        .families
        .insert(timestamp_font(12.0).family, vec!["Roboto".to_owned()]);
    ctx.set_fonts(fonts);
    ctx.set_visuals_of(
        egui::Theme::Dark,
        visuals(egui::Visuals::dark(), Palette::DARK),
    );
    ctx.set_visuals_of(
        egui::Theme::Light,
        visuals(egui::Visuals::light(), Palette::LIGHT),
    );
    #[cfg(target_os = "linux")]
    watch_system_theme(ctx.clone());
    ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.32;
        style.interaction.tooltip_grace_time = 0.5;
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.interact_size = vec2(38.0, 31.0);
        style.spacing.button_padding = vec2(11.0, 6.0);
        style.spacing.indent = 16.0;
        style.spacing.scroll.bar_width = 7.0;
        style.spacing.scroll.floating_width = 7.0;
        style.spacing.scroll.floating = true;
        style.spacing.scroll.floating_allocated_width =
            style.spacing.scroll.bar_width + style.spacing.scroll.bar_outer_margin;
        style.spacing.scroll.foreground_color = true;
        style.spacing.scroll.fade.strength = 0.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    });
}

fn visuals(mut visuals: egui::Visuals, colors: Palette) -> egui::Visuals {
    visuals.panel_fill = colors.back;
    visuals.window_fill = colors.card;
    visuals.extreme_bg_color = colors.well;
    visuals.faint_bg_color = colors.side;
    visuals.override_text_color = Some(colors.text);
    visuals.window_stroke = Stroke::new(1.0, colors.line);
    visuals.selection.bg_fill = colors.active;
    visuals.selection.stroke = Stroke::new(1.0, colors.accent);
    visuals.hyperlink_color = colors.accent;
    visuals.text_cursor.stroke.color = colors.text;
    visuals.warn_fg_color = colors.text;
    visuals.error_fg_color = colors.text;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.bg_fill = colors.well;
        widget.weak_bg_fill = colors.well;
        widget.bg_stroke = Stroke::new(1.0, colors.line);
        widget.fg_stroke = Stroke::new(1.0, colors.text);
        widget.corner_radius = 7.into();
    }
    visuals.widgets.hovered.weak_bg_fill = colors.hover;
    visuals.widgets.hovered.bg_fill = colors.hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, colors.muted);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, colors.accent);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, colors.line);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, colors.muted);
    visuals
}

pub fn apply(ctx: &egui::Context, appearance: Appearance) {
    let preference = match appearance {
        Appearance::Dark => egui::ThemePreference::Dark,
        Appearance::Light => egui::ThemePreference::Light,
        Appearance::System => {
            #[cfg(target_os = "linux")]
            let system = ctx.data(|data| data.get_temp::<egui::Theme>(portal_theme_id()));
            #[cfg(not(target_os = "linux"))]
            let system: Option<egui::Theme> = None;
            system.map_or(egui::ThemePreference::System, Into::into)
        }
    };
    if ctx.options(|options| options.theme_preference) != preference {
        ctx.set_theme(preference);
        ctx.request_discard("Appearance changed");
    }
    let native = match appearance {
        Appearance::Dark => egui::SystemTheme::Dark,
        Appearance::Light => egui::SystemTheme::Light,
        Appearance::System => egui::SystemTheme::SystemDefault,
    };
    let windows = ctx.input(|i| i.raw.viewports.keys().copied().collect::<Vec<_>>());
    for id in windows {
        let key = egui::Id::new(("native_appearance", id));
        if ctx.data(|d| d.get_temp::<Appearance>(key)) != Some(appearance) {
            ctx.data_mut(|d| d.insert_temp(key, appearance));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::SetTheme(native));
            ctx.request_repaint_of(id);
        }
    }
}

#[cfg(target_os = "linux")]
fn portal_theme_id() -> egui::Id {
    egui::Id::new("desktop_portal_color_scheme")
}

#[cfg(target_os = "linux")]
fn watch_system_theme(ctx: egui::Context) {
    std::thread::spawn(move || -> zbus::Result<()> {
        use std::collections::HashMap;
        use zbus::{blocking, zvariant::OwnedValue};

        type PortalSettings = HashMap<String, HashMap<String, OwnedValue>>;
        const NAMESPACE: &str = "org.freedesktop.appearance";
        const KEY: &str = "color-scheme";
        let connection = blocking::Connection::session()?;
        let settings = blocking::Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        let signals =
            settings.receive_signal_with_args("SettingChanged", &[(0, NAMESPACE), (1, KEY)])?;
        let update = |value: OwnedValue| {
            if let Ok(scheme) = u32::try_from(value) {
                let theme = if scheme == 1 {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                };
                ctx.data_mut(|data| data.insert_temp(portal_theme_id(), theme));
                ctx.request_repaint();
            }
        };
        if let Ok(mut values) = settings.call::<_, _, PortalSettings>("ReadAll", &([NAMESPACE],))
            && let Some(mut appearance) = values.remove(NAMESPACE)
            && let Some(value) = appearance.remove(KEY)
        {
            update(value);
        }
        for signal in signals {
            if let Ok((_, _, value)) = signal.body().deserialize::<(String, String, OwnedValue)>() {
                update(value);
            }
        }
        Ok(())
    });
}

fn semibold() -> FontFamily {
    FontFamily::Name("MyTube semibold".into())
}
pub fn timestamp_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Playback time".into()))
}

pub(super) fn paint_overlay_time(painter: &egui::Painter, center: Pos2, time: String, size: f32) {
    let label = painter.layout_no_wrap(time, timestamp_font(size), Color32::PLACEHOLDER);
    let origin = center - label.size() * 0.5;
    painter.galley(
        origin + vec2(0., 1.),
        label.clone(),
        Color32::from_black_alpha(110),
    );
    painter.galley(origin, label, Color32::WHITE);
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, semibold())
}

pub fn text(label: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(visual(&label.into()).into_owned())
        .size(size)
        .color(color)
}

pub fn strong(label: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(visual(&label.into()).into_owned())
        .font(font(size))
        .color(color)
}

/// Keep the indexed strings in logical order. For labels, resolve visual
/// order across font runs while leaving Arabic letters together for shaping.
pub fn visual(label: &str) -> Cow<'_, str> {
    use unicode_bidi::{BidiClass, BidiInfo, bidi_class};
    if label.is_ascii()
        || !label
            .chars()
            .any(|c| matches!(bidi_class(c), BidiClass::AL | BidiClass::R))
    {
        return Cow::Borrowed(label);
    }
    let info = BidiInfo::new(label, None);
    let mut output = String::with_capacity(label.len());
    for paragraph in &info.paragraphs {
        let (levels, runs) = info.visual_runs(paragraph, paragraph.range.clone());
        for run in runs {
            let text = &label[run.clone()];
            if !levels[run.start].is_rtl() {
                output.push_str(text);
                continue;
            }
            let joined = |c| {
                matches!(bidi_class(c), BidiClass::AL | BidiClass::R | BidiClass::NSM)
                    || matches!(c, '\u{200c}' | '\u{200d}')
            };
            let mut pieces = Vec::new();
            let mut start = 0;
            let mut previous = None;
            for (offset, character) in text.char_indices() {
                let current = joined(character);
                if let Some(was) = previous
                    && was != current
                {
                    pieces.push((&text[start..offset], was));
                    start = offset;
                }
                previous = Some(current);
            }
            if let Some(joined) = previous {
                pieces.push((&text[start..], joined));
            }
            for (piece, joined) in pieces.into_iter().rev() {
                if joined {
                    output.push_str(piece);
                } else {
                    output.extend(piece.chars().rev().map(|c| match c {
                        '(' => ')',
                        ')' => '(',
                        '[' => ']',
                        ']' => '[',
                        '{' => '}',
                        '}' => '{',
                        other => other,
                    }));
                }
            }
        }
    }
    Cow::Owned(output)
}

pub(super) const PAGE_SCROLL_GUTTER: f32 = 15.;

/// Page scrollbars share the window edge and the 15-point gutter of the watch reference.
pub fn page_scroll(ui: &mut Ui) -> egui::ScrollArea {
    let style = &mut ui.spacing_mut().scroll;
    style.bar_outer_margin = 0.;
    style.floating_allocated_width = PAGE_SCROLL_GUTTER;
    egui::ScrollArea::vertical()
        .content_margin(0)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .auto_shrink([false, false])
}

pub fn layer(fill: Color32, x: i8, y: i8) -> Frame {
    Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(x, y))
}

pub fn cut(
    painter: &egui::Painter,
    pos: Pos2,
    width: f32,
    label: &str,
    font: FontId,
    color: Color32,
) {
    let galley = cut_galley(painter, width, label, font, color);
    painter.galley(pos, galley, color);
}

fn cut_galley(
    painter: &egui::Painter,
    width: f32,
    label: &str,
    font: FontId,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut job =
        egui::text::LayoutJob::simple(visual(label).into_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    painter.fonts_mut(|fonts| fonts.layout_job(job))
}

#[derive(Clone, Copy)]
pub enum Icon {
    Film,
    Clock,
    Plus,
    Search,
    Folder,
    Disk,
    Grid,
    List,
    Filter,
    Play,
    Close,
    Refresh,
    Copy,
    Move,
    Trash,
    Pause,
    Collection,
    Info,
    External,
    Share,
    Back,
    Volume,
    Mute,
    Fullscreen,
    Previous,
    Next,
    Rewind,
    FastForward,
    More,
    Camera,
    Music,
    Theater,
    ChevronDown,
    ExpandMini,
    ClassicTransport(u8),
    Mini,
    Settings,
    Sidebar,
    Sun,
    Moon,
    System,
    Calendar,
}

fn playback_easing(x: f32) -> f32 {
    let mut low = 0.;
    let mut high = 1.;
    for _ in 0..12 {
        let t = (low + high) * 0.5;
        let u = 1. - t;
        if 1.2 * u * u * t + 0.6 * u * t * t + t * t * t < x {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) * 0.5;
    t * t * (3. - 2. * t)
}

impl Icon {
    pub(super) fn playback(ui: &Ui, paused: bool, reduce_motion: bool) -> Self {
        if interface_theme(ui.ctx()) != InterfaceTheme::Classic {
            return if paused { Self::Play } else { Self::Pause };
        }
        // YouTube desktop interpolates the two parts of the play glyph over 200ms
        // using cubic-bezier(.4, 0, .2, 1).
        let fraction = ui.ctx().animate_bool_with_time_and_easing(
            ui.id().with("play_pause_morph"),
            !paused,
            if reduce_motion { 0. } else { 0.2 },
            playback_easing,
        );
        Self::ClassicTransport((fraction * 32.).round() as u8)
    }

    pub fn paint(self, painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
        let point = |x: f32, y: f32| center + vec2(x, y) * (size / 24.0);
        let stroke = Stroke::new((size / 14.0).max(1.15), color);
        let line = |x, y, x2, y2| painter.line_segment([point(x, y), point(x2, y2)], stroke);
        let outline = |x, y, x2, y2, radius| {
            painter.rect_stroke(
                Rect::from_min_max(point(x, y), point(x2, y2)),
                radius,
                stroke,
                StrokeKind::Middle,
            );
        };
        match self {
            Self::Sun => {
                painter.circle_stroke(center, size * 0.21, stroke);
                for ray in 0..8 {
                    let angle = ray as f32 * std::f32::consts::FRAC_PI_4;
                    let (y, x) = angle.sin_cos();
                    line(x * 8.0, y * 8.0, x * 11.0, y * 11.0);
                }
            }
            Self::Moon => {
                let arc = |x: f32, y: f32, radius: f32, start: f32, sweep: f32| {
                    (0..=16).map(move |step| {
                        let angle = (start + sweep * step as f32 / 16.0).to_radians();
                        point(x + radius * angle.cos(), y + radius * angle.sin())
                    })
                };
                let mut crescent: Vec<_> = arc(-0.5, 0.6, 9.0, 8.0, 257.0).collect();
                crescent.extend(arc(4.3, -3.3, 7.0, 220.0, -153.0));
                painter.add(Shape::closed_line(crescent, stroke));
            }
            Self::System => {
                outline(-10.0, -8.0, 10.0, 5.0, 2);
                line(0.0, 5.0, 0.0, 9.0);
                line(-5.0, 9.0, 5.0, 9.0);
            }
            Self::Calendar => {
                outline(-9.0, -7.0, 9.0, 9.0, 2);
                line(-9.0, -2.0, 9.0, -2.0);
                line(-4.0, -10.0, -4.0, -5.0);
                line(4.0, -10.0, 4.0, -5.0);
                painter.circle_filled(point(0.0, 3.5), size * 0.065, color);
            }
            Self::Settings => {
                let mut gear = Vec::with_capacity(32);
                for tooth in 0..8 {
                    for (offset, radius) in [(-0.38, 8.1), (-0.24, 10.5), (0.24, 10.5), (0.38, 8.1)]
                    {
                        let angle = (tooth as f32 + offset) * std::f32::consts::TAU / 8.0;
                        gear.push(point(radius * angle.cos(), radius * angle.sin()));
                    }
                }
                painter.add(Shape::closed_line(gear, stroke));
                painter.circle_stroke(center, size * 0.145, stroke);
            }
            Self::Sidebar => {
                outline(-10.0, -8.0, 10.0, 8.0, 3);
                line(-3.0, -8.0, -3.0, 8.0);
            }
            Self::Film => {
                outline(-10.0, -8.0, 10.0, 8.0, 2);
                for x in [-5.5, 5.5] {
                    line(x, -8.0, x, 8.0);
                    let edge = x.signum() * 10.0;
                    for y in [-3.0, 3.0] {
                        line(x, y, edge, y);
                    }
                }
            }
            Self::Clock => {
                painter.circle_stroke(center, size * 0.38, stroke);
                line(0.0, -5.0, 0.0, 0.0);
                line(0.0, 0.0, 4.0, 2.0);
            }
            Self::Plus => {
                line(-7.0, 0.0, 7.0, 0.0);
                line(0.0, -7.0, 0.0, 7.0);
            }
            Self::Search => {
                painter.circle_stroke(point(-2.0, -2.0), size * 0.26, stroke);
                line(2.5, 2.5, 8.0, 8.0);
            }
            Self::Folder => {
                painter.add(Shape::line(
                    vec![
                        point(-10.0, 6.5),
                        point(-10.0, -6.5),
                        point(-2.5, -6.5),
                        point(0.0, -4.0),
                        point(10.0, -4.0),
                        point(10.0, 7.0),
                        point(-10.0, 7.0),
                    ],
                    stroke,
                ));
            }
            Self::Disk => {
                outline(-9.0, -7.5, 9.0, 7.5, 2);
                line(-9.0, 1.0, 9.0, 1.0);
                painter.circle_filled(point(5.0, 4.0), size / 17.0, color);
            }
            Self::Grid => {
                for (x, y) in [(-8.0, -8.0), (2.0, -8.0), (-8.0, 2.0), (2.0, 2.0)] {
                    outline(x, y, x + 6.0, y + 6.0, 1);
                }
            }
            Self::List => {
                for y in [-6.0, 0.0, 6.0] {
                    line(-3.0, y, 8.0, y);
                    painter.circle_filled(point(-7.0, y), size / 16.0, color);
                }
            }
            Self::Filter => {
                line(-9.0, -6.0, 9.0, -6.0);
                line(-9.0, 0.0, 9.0, 0.0);
                line(-9.0, 6.0, 9.0, 6.0);
                for (x, y) in [(-3.0, -6.0), (4.0, 0.0), (-1.0, 6.0)] {
                    painter.circle_filled(point(x, y), size * 0.10, color);
                }
            }
            Self::Play => {
                painter.add(Shape::convex_polygon(
                    vec![point(-5.0, -8.0), point(8.0, 0.0), point(-5.0, 8.0)],
                    color,
                    Stroke::NONE,
                ));
            }
            Self::Close => {
                line(-6.0, -6.0, 6.0, 6.0);
                line(6.0, -6.0, -6.0, 6.0);
            }
            Self::Refresh => {
                let arc = (0..=16)
                    .map(|i| {
                        let angle = (35.0 + i as f32 * 17.0).to_radians();
                        point(angle.cos() * 8.0, angle.sin() * 8.0)
                    })
                    .collect::<Vec<_>>();
                let end = arc[16];
                painter.add(Shape::line(arc, stroke));
                painter.line_segment([end, point(5.0, -8.5)], stroke);
                painter.line_segment([end, point(1.5, -1.5)], stroke);
            }
            Self::Copy => {
                outline(-4.0, -4.0, 8.0, 8.0, 2);
                line(-8.0, 3.0, -8.0, -8.0);
                line(-8.0, -8.0, 3.0, -8.0);
            }
            Self::Move => {
                line(-8.0, 0.0, 7.0, 0.0);
                line(7.0, 0.0, 2.0, -5.0);
                line(7.0, 0.0, 2.0, 5.0);
            }
            Self::Trash => {
                line(-9.0, -5.0, 9.0, -5.0);
                outline(-6.0, -5.0, 6.0, 8.0, 1);
                line(-3.0, -8.0, 3.0, -8.0);
                for x in [-2.0, 2.0] {
                    line(x, -1.0, x, 4.0);
                }
            }
            Self::Pause => {
                line(-3.0, -7.0, -3.0, 7.0);
                line(3.0, -7.0, 3.0, 7.0);
            }
            Self::Collection => {
                outline(-8.0, -4.0, 8.0, 7.0, 2);
                line(-6.0, -7.0, 6.0, -7.0);
                line(-3.0, 1.0, 3.0, 1.0);
            }
            Self::Info => {
                painter.circle_stroke(center, size * 0.38, stroke);
                painter.circle_filled(point(0.0, -3.5), size / 18.0, color);
                line(0.0, 0.0, 0.0, 5.0);
            }
            Self::Share => {
                line(-7.0, -2.0, -7.0, 9.0);
                line(-7.0, 9.0, 7.0, 9.0);
                line(7.0, 9.0, 7.0, -2.0);
                line(-7.0, -2.0, -4.0, -2.0);
                line(4.0, -2.0, 7.0, -2.0);
                line(0.0, 4.0, 0.0, -10.0);
                line(-4.0, -6.0, 0.0, -10.0);
                line(0.0, -10.0, 4.0, -6.0);
            }
            Self::Back => {
                line(-8.0, 0.0, 8.0, 0.0);
                line(-8.0, 0.0, -2.0, -6.0);
                line(-8.0, 0.0, -2.0, 6.0);
            }
            Self::Volume | Self::Mute => {
                painter.add(Shape::line(
                    vec![
                        point(-9.0, -3.0),
                        point(-5.0, -3.0),
                        point(0.0, -7.0),
                        point(0.0, 7.0),
                        point(-5.0, 3.0),
                        point(-9.0, 3.0),
                        point(-9.0, -3.0),
                    ],
                    stroke,
                ));
                if matches!(self, Self::Mute) {
                    line(4.0, -3.0, 10.0, 3.0);
                    line(10.0, -3.0, 4.0, 3.0);
                } else {
                    painter.add(Shape::line(
                        vec![
                            point(5.0, -6.0),
                            point(8.0, -3.0),
                            point(8.0, 3.0),
                            point(5.0, 6.0),
                        ],
                        stroke,
                    ));
                }
            }
            Self::More => {
                for x in [-7., 0., 7.] {
                    painter.circle_filled(point(x, 0.), size / 12., color);
                }
            }
            Self::Camera => {
                outline(-10., -6., 10., 8., 2);
                painter.circle_stroke(point(0., 1.), size * 0.18, stroke);
                line(-4., -8., 4., -8.);
            }
            Self::Music => {
                line(-3., 5., -3., -7.);
                line(-3., -7., 7., -9.);
                line(7., -9., 7., 3.);
                painter.circle_filled(point(-6., 6.), size * 0.13, color);
                painter.circle_filled(point(4., 4.), size * 0.13, color);
            }
            Self::Rewind | Self::FastForward => {
                let direction = if matches!(self, Self::FastForward) {
                    1.0
                } else {
                    -1.0
                };
                for start in [-9.0, -1.0] {
                    painter.add(Shape::convex_polygon(
                        vec![
                            point(start * direction, -6.0),
                            point((start + 8.0) * direction, 0.0),
                            point(start * direction, 6.0),
                        ],
                        color,
                        Stroke::NONE,
                    ));
                }
            }
            Self::Previous | Self::Next => {
                let direction = if matches!(self, Self::Next) {
                    1.0
                } else {
                    -1.0
                };
                painter.add(Shape::convex_polygon(
                    vec![
                        point(-7.0 * direction, -6.0),
                        point(3.0 * direction, 0.0),
                        point(-7.0 * direction, 6.0),
                    ],
                    color,
                    Stroke::NONE,
                ));
                line(6.0 * direction, -6.0, 6.0 * direction, 6.0);
            }
            Self::Theater => {
                outline(-10.0, -6.0, 10.0, 6.0, 1);
            }
            Self::ClassicTransport(frame) => {
                // 33 play-to-pause frames, followed by expand and close.
                let key = egui::Id::new("classic_transport_texture");
                let texture: egui::TextureHandle =
                    painter.ctx().data(|d| d.get_temp(key)).unwrap_or_else(|| {
                        let image = image::load_from_memory(include_bytes!(
                            "../../assets/classic-transport.png"
                        ))
                        .expect("bundled transport icons")
                        .into_rgba8();
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [image.width() as usize, image.height() as usize],
                            image.as_raw(),
                        );
                        let texture = painter.ctx().load_texture(
                            "Classic transport icons",
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        painter
                            .ctx()
                            .data_mut(|d| d.insert_temp(key, texture.clone()));
                        texture
                    });
                let uv = pos2(f32::from(frame % 8) / 8., f32::from(frame / 8) / 5.);
                painter.image(
                    texture.id(),
                    Rect::from_center_size(center, egui::Vec2::splat(size)),
                    Rect::from_min_size(uv, vec2(1. / 8., 1. / 5.)),
                    color,
                );
            }
            Self::ChevronDown => {
                painter.add(Shape::line(
                    vec![point(-6., -3.), point(0., 3.), point(6., -3.)],
                    stroke,
                ));
            }
            Self::ExpandMini => {
                line(-9., -9., -2., -9.);
                line(-9., -9., -9., -2.);
                line(-9., -9., 5., 5.);
                painter.add(Shape::line(
                    vec![
                        point(1., -9.),
                        point(9., -9.),
                        point(9., 9.),
                        point(-9., 9.),
                        point(-9., 1.),
                    ],
                    stroke,
                ));
            }
            Self::Mini => {
                outline(-10.0, -7.5, 10.0, 7.5, 2);
                painter.rect_filled(
                    Rect::from_min_max(point(1.0, 0.0), point(7.0, 5.0)),
                    0,
                    color,
                );
            }
            Self::Fullscreen => {
                for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    line(x * 8.0, y * 8.0, x * 8.0, y * 3.0);
                    line(x * 8.0, y * 8.0, x * 3.0, y * 8.0);
                }
            }
            Self::External => {
                line(1.0, -8.0, 8.0, -8.0);
                line(8.0, -8.0, 8.0, -1.0);
                line(8.0, -8.0, 0.0, 0.0);
                painter.add(Shape::line(
                    vec![
                        point(-2.0, -7.0),
                        point(-8.0, -7.0),
                        point(-8.0, 8.0),
                        point(7.0, 8.0),
                        point(7.0, 2.0),
                    ],
                    stroke,
                ));
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Selected,
    Quiet,
    OnVideo,
    OnVideoSelected,
    Danger,
    Nav,
    NavActive,
}

pub fn button(ui: &mut Ui, label: &str, icon: Option<Icon>, kind: ButtonKind) -> Response {
    control(ui, label, icon, kind, false, None)
}

pub(super) fn quiet_button(ui: &mut Ui, label: &str, width: f32) -> Response {
    control(ui, label, None, ButtonKind::Quiet, false, Some(width))
}

pub fn icon_button(ui: &mut Ui, label: &str, icon: Icon, kind: ButtonKind) -> Response {
    let label = super::player_keys::label(ui.ctx(), label);
    control(ui, &label, Some(icon), kind, true, None).on_hover_text(label.as_ref())
}

pub fn nav(ui: &mut Ui, label: &str, icon: Icon, selected: bool, icon_only: bool) -> Response {
    control(
        ui,
        label,
        Some(icon),
        if selected {
            ButtonKind::NavActive
        } else {
            ButtonKind::Nav
        },
        icon_only,
        Some(ui.available_width()),
    )
}

fn control(
    ui: &mut Ui,
    label: &str,
    icon: Option<Icon>,
    kind: ButtonKind,
    icon_only: bool,
    width: Option<f32>,
) -> Response {
    let colors = palette(ui);
    let size = 13.0;
    let is_nav = matches!(kind, ButtonKind::Nav | ButtonKind::NavActive);
    let on_video = matches!(kind, ButtonKind::OnVideo | ButtonKind::OnVideoSelected);
    let is_quiet = kind == ButtonKind::Quiet || is_nav;
    let font = if kind == ButtonKind::Primary || kind == ButtonKind::NavActive {
        font(size)
    } else {
        FontId::proportional(size)
    };
    let measured = if icon_only || width.is_some() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(visual(label).into_owned(), font.clone(), colors.text)
            .size()
            .x
    };
    let text_x = if icon.is_some() { 34.0 } else { 11.0 };
    let width = width.unwrap_or({
        if icon_only {
            34.0
        } else {
            text_x + measured + 11.0
        }
    });
    let (rect, response) = ui.allocate_exact_size(
        vec2(width, if is_nav { 36.0 } else { 34.0 }),
        Sense::click(),
    );
    response.widget_info(|| {
        if is_nav {
            WidgetInfo::selected(
                WidgetType::Button,
                ui.is_enabled(),
                kind == ButtonKind::NavActive,
                label,
            )
        } else {
            WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label)
        }
    });
    let hot = response.hovered() || response.has_focus();
    let (fill, color) = match kind {
        ButtonKind::Primary => (
            if hot {
                colors.primary_hover
            } else {
                colors.accent
            },
            colors.on_primary,
        ),
        ButtonKind::Danger => (
            if hot {
                colors.danger_hover
            } else {
                colors.danger_fill
            },
            colors.text,
        ),
        ButtonKind::NavActive => (if hot { colors.hover } else { colors.active }, colors.text),
        ButtonKind::Selected if !ui.visuals().dark_mode => (colors.accent, colors.on_primary),
        ButtonKind::Selected => (
            if hot {
                colors.active.lerp_to_gamma(colors.accent, 0.11)
            } else {
                colors.active
            },
            colors.accent,
        ),
        ButtonKind::OnVideo | ButtonKind::OnVideoSelected => (
            if response.has_focus() || kind == ButtonKind::OnVideoSelected {
                Color32::from_gray(72)
            } else if hot {
                if solid_glass(ui.ctx()) {
                    Color32::from_gray(48)
                } else {
                    Color32::from_rgba_unmultiplied(36, 36, 36, 175)
                }
            } else {
                video_button_fill(ui.ctx())
            },
            Color32::WHITE,
        ),
        _ => (
            if hot {
                colors.hover
            } else if is_quiet {
                Color32::TRANSPARENT
            } else {
                colors.well
            },
            if is_nav && !hot {
                colors.muted
            } else {
                colors.text
            },
        ),
    };
    let fill = if matches!(kind, ButtonKind::Secondary | ButtonKind::NavActive)
        || (kind == ButtonKind::Selected && ui.visuals().dark_mode)
    {
        control_fill(ui, fill)
    } else {
        fill
    };
    let fill = if is_nav && !ui.visuals().dark_mode {
        match (kind == ButtonKind::NavActive, hot) {
            (true, true) => Color32::from_black_alpha(37),
            (true, false) => Color32::from_black_alpha(26),
            (false, true) => Color32::from_black_alpha(16),
            (false, false) => Color32::TRANSPARENT,
        }
    } else {
        fill
    };
    let painter = ui.painter();
    let face = if on_video { rect.shrink(3.0) } else { rect };
    let paint_rect = if response.is_pointer_button_down_on() {
        Rect::from_center_size(face.center(), face.size() * 0.97)
    } else {
        face
    };
    if on_video {
        painter.circle_filled(paint_rect.center(), paint_rect.height() / 2.0, fill);
    } else {
        let classic_action = interface_theme(ui.ctx()) == InterfaceTheme::Classic
            && matches!(kind, ButtonKind::Primary | ButtonKind::Secondary);
        let border = if response.has_focus() {
            if classic_action {
                focus_color(ui)
            } else {
                colors.accent
            }
        } else if kind == ButtonKind::Danger {
            colors.muted
        } else if is_quiet || kind == ButtonKind::Primary || classic_action {
            Color32::TRANSPARENT
        } else {
            colors.line
        };
        painter.rect(
            paint_rect,
            if is_nav {
                9
            } else if classic_action {
                10
            } else {
                16
            },
            fill,
            Stroke::new(
                if classic_action && response.has_focus() {
                    2.
                } else {
                    1.
                },
                border,
            ),
            StrokeKind::Inside,
        );
    }
    if let Some(icon) = icon {
        let center = if icon_only {
            rect.center()
        } else {
            pos2(rect.min.x + 17.0, rect.center().y)
        };
        icon.paint(painter, center, 17.0, color);
    }
    if !icon_only {
        let pos = pos2(rect.min.x + text_x, rect.center().y - size * 0.63);
        let width = rect.width() - text_x - 8.0;
        cut(painter, pos, width, label, font, color);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn video_button_fill(ctx: &egui::Context) -> Color32 {
    if solid_glass(ctx) {
        Color32::from_gray(32)
    } else {
        VIDEO_BUTTON_FILL
    }
}
pub(super) const VIDEO_BUTTON_FILL: Color32 = Color32::from_black_alpha(51);

pub fn centered_text(
    painter: &egui::Painter,
    center: Pos2,
    label: &str,
    size: f32,
    color: Color32,
) {
    painter.text(
        center,
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(size),
        color,
    );
}
