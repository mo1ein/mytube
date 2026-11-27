//! Both presentations run the same preference edits and playback commands.
use crate::player::{MAX_SPEED, MIN_SPEED, SPEED_STEP};
use eframe::egui::{self, Context, Ui, emath::Numeric};
#[cfg(target_os = "macos")]
use serde_json::{Value, json};
#[cfg(target_os = "macos")]
use std::collections::HashMap;
use std::{hash::Hash, ops::RangeInclusive};

use super::{classic_settings, theme};

const SPEEDS: [(f64, &str); 12] = [
    (0.25, "0.25×"),
    (0.5, "0.5×"),
    (0.75, "0.75×"),
    (1., "Normal"),
    (1.25, "1.25×"),
    (1.5, "1.5×"),
    (1.75, "1.75×"),
    (2., "2×"),
    (2.5, "2.5×"),
    (3., "3×"),
    (3.5, "3.5×"),
    (4., "4×"),
];

pub(super) fn speed_slider(ui: &mut Ui, rate: &mut f64) -> egui::Response {
    let slider = egui::Slider::new(rate, MIN_SPEED..=MAX_SPEED)
        .step_by(SPEED_STEP)
        .fixed_decimals(2)
        .suffix("×");
    super::theme::slider(ui, slider)
}

fn speed_presets(ui: &mut Ui, current: f64, classic: bool) -> Option<f64> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4., 4.);
        for (rate, title) in SPEEDS {
            let selected = (rate - current).abs() < 0.001;
            let response = if classic {
                classic_settings::pill(ui, title, selected, false)
            } else {
                ui.selectable_label(selected, title)
            };
            if !classic && response.hovered() {
                let tint = ui.visuals().text_color().gamma_multiply(0.07);
                ui.painter().rect_filled(
                    response.rect,
                    ui.style().interact(&response).corner_radius,
                    tint,
                );
            }
            if response.clicked() {
                chosen = Some(rate);
            }
        }
    });
    chosen
}

pub(super) enum Form<'a> {
    Portable(&'a mut Ui),
    #[cfg(target_os = "macos")]
    Native(&'a mut NativeForm),
}

pub(super) enum Response {
    Portable(egui::Response),
    #[cfg(target_os = "macos")]
    Native(bool),
}
impl Response {
    pub fn clicked(&self) -> bool {
        match self {
            Self::Portable(r) => r.clicked(),
            #[cfg(target_os = "macos")]
            Self::Native(hit) => *hit,
        }
    }
    pub fn changed(&self) -> bool {
        match self {
            Self::Portable(r) => r.changed(),
            #[cfg(target_os = "macos")]
            Self::Native(hit) => *hit,
        }
    }
    pub fn on_hover_text(self, text: impl ToString) -> Self {
        match self {
            Self::Portable(r) => Self::Portable(r.on_hover_text(text.to_string())),
            #[cfg(target_os = "macos")]
            r => r,
        }
    }
}

pub(super) enum Choices<'a> {
    Portable(&'a mut Ui, Option<&'a mut theme::MenuRows>),
    #[cfg(target_os = "macos")]
    Native(&'a mut NativeForm),
}

impl Choices<'_> {
    pub fn selectable_value<T: PartialEq>(
        &mut self,
        value: &mut T,
        choice: T,
        label: impl ToString,
    ) -> Response {
        match self {
            Self::Portable(ui, Some(rows)) => {
                let label = label.to_string();
                let selected = *value == choice;
                let mut response = rows.add(classic_settings::choice_item(ui, &label, selected));
                if selected {
                    rows.prefer(&response);
                }
                if response.clicked() {
                    if !selected {
                        *value = choice;
                        response.mark_changed();
                    }
                    ui.close();
                }
                Response::Portable(response)
            }
            Self::Portable(ui, None) => {
                Response::Portable(ui.selectable_value(value, choice, label.to_string()))
            }
            #[cfg(target_os = "macos")]
            Self::Native(form) => Form::Native(form).selectable_value(value, choice, label),
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) struct NativeForm {
    pub ctx: Context,
    pub rows: Vec<Value>,
    pub action: Option<(String, Value)>,

    scope: egui::Id,
    row_counts: HashMap<egui::Id, usize>,
    depth: usize,
    enabled: bool,
    combo: Option<(String, Vec<String>)>,
}
#[cfg(target_os = "macos")]
impl NativeForm {
    pub fn new(ctx: &Context, section: &str, action: Option<(String, Value)>) -> Self {
        Self {
            ctx: ctx.clone(),
            rows: vec![],
            action,
            scope: egui::Id::new(section),
            row_counts: HashMap::new(),
            depth: 0,
            enabled: true,
            combo: None,
        }
    }
    fn id(&self, kind: &str, label: &str) -> String {
        let key = self.scope.with((kind, label));
        let occurrence = self.row_counts.get(&key).copied().unwrap_or_default();
        format!("{:x}", key.with(occurrence).value())
    }
    fn row(&mut self, kind: &str, label: &str, value: Value, options: Value) -> Option<Value> {
        let id = self.id(kind, label);
        *self
            .row_counts
            .entry(self.scope.with((kind, label)))
            .or_default() += 1;
        self.rows.push(json!({"id":id,"kind":kind,"label":label,"value":value,"options":options,"enabled":self.enabled,"depth":self.depth}));
        if self.enabled && self.action.as_ref().is_some_and(|a| a.0 == id) {
            self.action.take().map(|a| a.1)
        } else {
            None
        }
    }
}
impl Form<'_> {
    pub fn is_classic(&self) -> bool {
        matches!(self, Self::Portable(ui) if classic_settings::active(ui))
    }

    pub fn group(&mut self, title: &str, body: impl FnOnce(&mut Form)) {
        match self {
            Self::Portable(ui) if classic_settings::active(ui) => {
                classic_settings::group(ui, title, |ui| body(&mut Form::Portable(ui)))
            }
            Self::Portable(ui) => {
                super::theme::form_group(ui, title, |ui| body(&mut Form::Portable(ui)))
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                n.row("heading", title, Value::Null, Value::Null);
                self.push_id(title, body);
            }
        }
    }
    pub fn playback_speed(&mut self, current: f64) -> Option<f64> {
        let label = "Playback speed";
        let preset = match self {
            Self::Portable(ui) => {
                ui.strong(label);
                speed_presets(ui, current, classic_settings::active(ui))
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => n
                .row(
                    "speed",
                    label,
                    json!(
                        SPEEDS
                            .iter()
                            .position(|(rate, _)| (rate - current).abs() < 0.001)
                    ),
                    json!(SPEEDS.map(|(_, title)| title)),
                )
                .and_then(|v| usize::try_from(v.as_u64()?).ok())
                .and_then(|i| SPEEDS.get(i).map(|&(rate, _)| rate)),
        };
        let mut rate = preset.unwrap_or(current);
        let label = "Fine adjustment";
        match self {
            Self::Portable(ui) => {
                let response = if classic_settings::active(ui) {
                    classic_settings::row(ui, label, |ui, caption| {
                        speed_slider(ui, &mut rate).labelled_by(caption)
                    })
                } else {
                    let caption = ui.label(label);
                    speed_slider(ui, &mut rate).labelled_by(caption.id)
                };
                response.changed().then_some(rate)
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => n
                .row(
                    "fineSpeed",
                    label,
                    json!(rate),
                    json!([MIN_SPEED, MAX_SPEED, SPEED_STEP]),
                )
                .and_then(|v| v.as_f64())
                .map(crate::player::stepped_speed),
        }
        .or(preset)
    }
    pub fn ctx(&self) -> &Context {
        match self {
            Self::Portable(u) => u.ctx(),
            #[cfg(target_os = "macos")]
            Self::Native(n) => &n.ctx,
        }
    }
    pub fn horizontal_wrapped<R>(&mut self, f: impl FnOnce(&mut Form) -> R) -> R {
        match self {
            Self::Portable(u) => u.horizontal_wrapped(|ui| f(&mut Form::Portable(ui))).inner,
            #[cfg(target_os = "macos")]
            Self::Native(_) => f(self),
        }
    }
    pub fn push_id<R>(
        &mut self,
        id: impl Hash + std::fmt::Debug,
        f: impl FnOnce(&mut Form) -> R,
    ) -> R {
        match self {
            Self::Portable(u) => u.push_id(id, |ui| f(&mut Form::Portable(ui))).inner,
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let old = n.scope;
                n.scope = old.with(id);
                let result = f(&mut Form::Native(n));
                n.scope = old;
                result
            }
        }
    }
    pub fn enabled<R>(&mut self, on: bool, f: impl FnOnce(&mut Form) -> R) -> R {
        match self {
            Self::Portable(u) => u.add_enabled_ui(on, |ui| f(&mut Form::Portable(ui))).inner,
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let old = n.enabled;
                n.enabled &= on;
                let result = f(&mut Form::Native(n));
                n.enabled = old;
                result
            }
        }
    }
    pub fn collapsing(&mut self, label: &str, f: impl FnOnce(&mut Form)) {
        match self {
            Self::Portable(u) => {
                let id = u.make_persistent_id(label);
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    u.ctx(),
                    id,
                    false,
                );
                super::theme::disclosure_row(u, label, &mut state);
                state.store(u.ctx());
                if state.is_open() {
                    u.indent(id, |ui| f(&mut Form::Portable(ui)));
                }
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let id = n.scope.with(label);
                let mut open = n.ctx.data(|d| d.get_temp(id)).unwrap_or(false);
                if n.row("disclosure", label, json!(open), Value::Null)
                    .is_some()
                {
                    open = !open;
                    n.ctx.data_mut(|d| d.insert_temp(id, open));
                    n.rows.last_mut().unwrap()["value"] = json!(open);
                }
                if open {
                    let parent = n.scope;
                    n.scope = id;
                    n.depth += 1;
                    f(&mut Form::Native(n));
                    n.depth -= 1;
                    n.scope = parent;
                }
            }
        }
    }
    pub fn theme_picker(&mut self, theme: &mut crate::player::options::InterfaceTheme) {
        use crate::player::options::InterfaceTheme;
        let label = "Interface theme";
        if self.is_classic() {
            self.combo(label, theme.label(), |ui| {
                for choice in InterfaceTheme::ALL {
                    ui.selectable_value(theme, choice, choice.label());
                }
            });
            return;
        }
        match self {
            Self::Portable(ui) => {
                ui.strong(label);
                ui.horizontal_wrapped(|ui| {
                    for choice in InterfaceTheme::ALL {
                        ui.selectable_value(theme, choice, choice.label());
                    }
                });
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let choices = InterfaceTheme::ALL;
                let selected = choices.iter().position(|t| t == theme);
                if let Some(index) = n
                    .row(
                        "segments",
                        label,
                        json!(selected),
                        json!(choices.map(InterfaceTheme::label)),
                    )
                    .and_then(|v| v.as_u64())
                    && let Some(choice) = choices.get(index as usize)
                {
                    *theme = *choice;
                }
            }
        }
    }
    pub fn combo<R>(
        &mut self,
        label: &str,
        current: impl ToString,
        f: impl FnOnce(&mut Choices) -> R,
    ) -> Option<R> {
        let current = current.to_string();
        match self {
            Self::Portable(u) if classic_settings::active(u) => {
                classic_settings::row(u, label, |ui, caption| {
                    let combo = classic_settings::combo(ui, label, current, |menu, rows| {
                        f(&mut Choices::Portable(menu, Some(rows)))
                    });
                    combo.response.labelled_by(caption);
                    combo.inner
                })
            }
            Self::Portable(u) => {
                let caption = u.label(label);
                let combo = egui::ComboBox::from_id_salt(label)
                    .popup_style(theme::menu_style(u.ctx()))
                    .width(u.available_width().clamp(1., 340.))
                    .selected_text(current)
                    .truncate()
                    .show_ui(u, |ui| f(&mut Choices::Portable(ui, None)));
                combo.response.labelled_by(caption.id);
                combo.inner
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                n.combo = Some((n.id("choice", label), vec![]));
                let result = f(&mut Choices::Native(n));
                let (_, options) = n.combo.take().unwrap();
                n.row("choice", label, json!(current), json!(options));
                Some(result)
            }
        }
    }
    pub fn selectable_value<T: PartialEq>(
        &mut self,
        value: &mut T,
        choice: T,
        label: impl ToString,
    ) -> Response {
        let label = label.to_string();
        match self {
            Self::Portable(u) if classic_settings::active(u) => {
                let selected = *value == choice;
                let mut response = classic_settings::pill(u, &label, selected, false);
                if response.clicked() && !selected {
                    *value = choice;
                    response.mark_changed();
                }
                Response::Portable(response)
            }
            Self::Portable(u) => Response::Portable(u.selectable_value(value, choice, label)),
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let hit = if let Some((id, items)) = &mut n.combo {
                    let index = items.len();
                    items.push(label);
                    n.enabled
                        && n.action
                            .as_ref()
                            .is_some_and(|a| &a.0 == id && a.1.as_u64() == Some(index as u64))
                } else {
                    n.row("radio", &label, json!(*value == choice), Value::Null)
                        .is_some()
                };
                if hit {
                    *value = choice
                }
                Response::Native(hit)
            }
        }
    }
    pub fn selectable_label(&mut self, selected: bool, label: impl ToString) -> Response {
        let mut value = selected;
        self.selectable_value(&mut value, true, label)
    }
    pub fn label(&mut self, label: impl ToString) -> Response {
        let label = label.to_string();
        match self {
            Self::Portable(u) => Response::Portable(u.label(label)),
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                n.row("label", &label, Value::Null, Value::Null);
                Response::Native(false)
            }
        }
    }
    pub fn button(&mut self, label: impl ToString) -> Response {
        self.enabled_button(true, label)
    }
    pub fn enabled_button(&mut self, enabled: bool, label: impl ToString) -> Response {
        let label = label.to_string();
        self.enabled(enabled, |form| match form {
            Form::Portable(u) if classic_settings::active(u) => {
                Response::Portable(classic_settings::pill(u, &label, false, false))
            }
            Form::Portable(u) => Response::Portable(u.button(label)),
            #[cfg(target_os = "macos")]
            Form::Native(n) => {
                Response::Native(n.row("button", &label, Value::Null, Value::Null).is_some())
            }
        })
    }
    pub fn checkbox(&mut self, value: &mut bool, label: &str) -> Response {
        match self {
            Self::Portable(u) => Response::Portable(super::theme::setting_toggle(u, value, label)),
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let hit = n.row("toggle", label, json!(*value), Value::Null).is_some();
                if hit {
                    *value = !*value
                }
                Response::Native(hit)
            }
        }
    }
    pub fn number<T: Numeric>(
        &mut self,
        value: &mut T,
        range: RangeInclusive<T>,
        label: impl ToString,
        step: Option<f64>,
        log: bool,
    ) -> Response {
        let label = label.to_string();
        match self {
            Self::Portable(u) => {
                let mut slider = egui::Slider::new(value, range).logarithmic(log);
                if let Some(step) = step {
                    slider = slider.step_by(step)
                }
                let response = if classic_settings::active(u) {
                    classic_settings::row(u, &label, |ui, caption| {
                        super::theme::slider(ui, slider).labelled_by(caption)
                    })
                } else {
                    let caption = u.label(&label);
                    super::theme::slider(u, slider).labelled_by(caption.id)
                };
                Response::Portable(response)
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let min = range.start().to_f64();
                let max = range.end().to_f64();
                let change = n
                    .row(
                        "number",
                        &label,
                        json!(value.to_f64()),
                        json!([min, max, step]),
                    )
                    .and_then(|v| v.as_f64());
                if let Some(v) = change {
                    let v = step.map_or(v, |s| min + ((v - min) / s).round() * s);
                    *value = T::from_f64(v.clamp(min, max));
                }
                Response::Native(change.is_some())
            }
        }
    }
    pub fn text(&mut self, value: &mut String, label: &str, secret: bool) -> Response {
        match self {
            Self::Portable(u) => Response::Portable(
                u.add(
                    egui::TextEdit::singleline(value)
                        .password(secret)
                        .hint_text(label),
                ),
            ),
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let change = n.row(
                    if secret { "password" } else { "text" },
                    label,
                    json!(value),
                    Value::Null,
                );
                if let Some(v) = change.as_ref().and_then(Value::as_str) {
                    *value = v.into()
                }
                Response::Native(change.is_some())
            }
        }
    }
    pub fn separator(&mut self) {
        match self {
            Self::Portable(u) => {
                u.separator();
            }
            #[cfg(target_os = "macos")]
            Self::Native(_) => {}
        }
    }
    pub fn grid(&mut self, id: &str, f: impl FnOnce(&mut Form)) {
        match self {
            Self::Portable(u) => {
                egui::Grid::new(id).show(u, |ui| f(&mut Form::Portable(ui)));
            }
            #[cfg(target_os = "macos")]
            Self::Native(_) => f(self),
        }
    }
    pub fn end_row(&mut self) {
        match self {
            Self::Portable(u) => u.end_row(),
            #[cfg(target_os = "macos")]
            Self::Native(_) => {}
        }
    }
}

impl Form<'_> {
    pub fn labeled_text(&mut self, value: &mut String, label: &str) -> Response {
        match self {
            Self::Portable(u) => {
                u.horizontal(|ui| {
                    ui.label(label);
                    ui.text_edit_singleline(value).into()
                })
                .inner
            }
            #[cfg(target_os = "macos")]
            Self::Native(_) => self.text(value, label, false),
        }
    }

    pub fn color(&mut self, rgba: &mut [u8; 4], label: &str) -> Response {
        match self {
            Self::Portable(u) => {
                u.horizontal(|ui| {
                    let response = ui.color_edit_button_srgba_unmultiplied(rgba);
                    ui.label(label);
                    response.into()
                })
                .inner
            }
            #[cfg(target_os = "macos")]
            Self::Native(n) => {
                let change = n.row("color", label, json!(rgba), Value::Null);
                if let Some(color) = change.clone().and_then(|v| serde_json::from_value(v).ok()) {
                    *rgba = color
                }
                Response::Native(change.is_some())
            }
        }
    }
}
impl From<egui::Response> for Response {
    fn from(r: egui::Response) -> Self {
        Self::Portable(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disclosure_opens_from_the_trailing_edge_and_closes_with_space() {
        let ctx = Context::default();
        let frame = |events| {
            let mut open = false;
            let mut point = egui::Pos2::ZERO;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(240., 180.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    point = ui.available_rect_before_wrap().right_top() + egui::vec2(-15., 16.);
                    Form::Portable(ui).collapsing("Window behavior", |_| open = true);
                },
            );
            output.drop_without_applying_deltas();
            (open, point)
        };

        let (open, point) = frame(vec![]);
        assert!(!open);
        assert!(!frame(vec![egui::Event::PointerMoved(point)]).0);
        let button = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(!frame(vec![button(true)]).0);
        assert!(frame(vec![button(false)]).0);
        let press = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(frame(vec![press(egui::Key::Tab)]).0);
        assert!(!frame(vec![press(egui::Key::Space)]).0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_disclosure_updates_its_state_and_keeps_sibling_ids_stable() {
        let ctx = Context::default();
        let render = |action| {
            let mut form = NativeForm::new(&ctx, "General", action);
            let mut ui = Form::Native(&mut form);
            ui.collapsing("Feedback and previews", |ui| {
                ui.label("Show remaining time");
            });
            ui.collapsing("Window behavior", |_| {});
            form.rows
        };
        let closed = render(None);
        let click = || Some((closed[0]["id"].as_str().unwrap().to_owned(), json!(true)));
        assert_eq!(closed[0]["value"], false);
        let open = render(click());
        assert_eq!(open[0]["value"], true);
        assert_eq!(open[1]["depth"], 1);
        assert_eq!(open[2]["id"], closed[1]["id"]);
        assert_eq!(render(click()), closed);
    }

    #[test]
    fn speed_presets_require_a_click_and_slider_applies_fine_steps() {
        check_speed_controls(false);
        check_speed_controls(true);
    }

    fn check_speed_controls(classic: bool) {
        use crate::player::options::InterfaceTheme;
        let ctx = Context::default();
        theme::install(&ctx);
        theme::apply_style(
            &ctx,
            if classic {
                InterfaceTheme::Classic
            } else {
                InterfaceTheme::LiquidGlass
            },
            62,
        );
        let render = |events, rate| {
            let mut choice = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(238., 500.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if classic {
                        let mut section = crate::player::options::Section::Video;
                        classic_settings::page(ui, &mut section, true, |ui, _| {
                            choice = Form::Portable(ui).playback_speed(rate);
                        });
                    } else {
                        choice = Form::Portable(ui).playback_speed(rate);
                    }
                },
            );
            let labels = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.text().to_string(), t.pos + t.galley.size() * 0.5))
                    }
                    _ => None,
                })
                .collect::<std::collections::HashMap<_, _>>();
            let rail = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Rect(r)
                        if r.rect.width() > 35.
                            && if classic {
                                r.rect.height() == 4. && r.fill == egui::Color32::from_gray(80)
                            } else {
                                r.rect.height() < 10.
                            } =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .unwrap();
            output.drop_without_applying_deltas();
            (choice, labels, rail)
        };
        let (choice, labels, rail) = render(vec![], 1.);
        assert_eq!(choice, None);
        for (_, title) in SPEEDS {
            assert!(
                labels.contains_key(title),
                "{title} missing from {:?}",
                labels.keys()
            );
        }
        let click = |pos, rate| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let pressed = render(vec![egui::Event::PointerMoved(pos), button(true)], rate).0;
            render(vec![button(false)], pressed.unwrap_or(rate))
                .0
                .or(pressed)
        };
        for (_, title) in SPEEDS {
            assert_eq!(
                render(vec![egui::Event::PointerMoved(labels[title])], 1.).0,
                None
            );
        }
        assert_eq!(click(labels["1.75×"], 1.), Some(1.75));
        assert_eq!(click(labels["Normal"], 1.75), Some(1.));
        let rate = click(rail.lerp_inside(egui::vec2(0.6, 0.5)), 1.).unwrap_or_else(|| {
            panic!("Click did not adjust the slider (Classic: {classic}, rail: {rail:?})")
        });
        assert!((2.0..3.).contains(&rate));
        assert!((rate * 20. - (rate * 20.).round()).abs() < 1e-8);
        assert!(
            !SPEEDS
                .iter()
                .any(|&(preset, _)| (preset - rate).abs() < 1e-8)
        );
        if classic {
            let (adjusted, _, _) = render(
                vec![egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                rate,
            );
            assert!((adjusted.unwrap() - rate - SPEED_STEP).abs() < 1e-8);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_speed_rows_dispatch_presets_and_fine_steps() {
        let ctx = Context::default();
        let mut model = NativeForm::new(&ctx, "Video", None);
        assert_eq!(Form::Native(&mut model).playback_speed(1.), None);
        let apply = |row: usize, value: Value| {
            let action = (model.rows[row]["id"].as_str().unwrap().to_owned(), value);
            let mut form = NativeForm::new(&ctx, "Video", Some(action));
            Form::Native(&mut form).playback_speed(1.)
        };
        for (i, (rate, _)) in SPEEDS.into_iter().enumerate() {
            assert_eq!(apply(0, json!(i)), Some(rate));
        }
        assert!((apply(1, json!(1.471)).unwrap() - 1.45).abs() < 1e-8);
        assert_eq!(apply(1, json!(0.1)), Some(0.25));
        assert_eq!(apply(1, json!(8.0)), Some(4.0));
    }
}
