use super::settings_form::Form;
use crate::player::options::{Binding, PlayerPreferences};
use eframe::egui::{self, Context, Event, Key, KeyboardShortcut, Modifiers};

const ACTIONS: &[(Key, &str)] = &[
    (Key::Space, "Play / pause"),
    (Key::K, "Play / pause (alternate)"),
    (Key::ArrowLeft, "Seek backward"),
    (Key::ArrowRight, "Seek forward"),
    (Key::J, "Back ten seconds"),
    (Key::L, "Forward ten seconds"),
    (Key::Comma, "Previous frame"),
    (Key::Period, "Next frame"),
    (Key::ArrowUp, "Previous video"),
    (Key::ArrowDown, "Next video"),
    (Key::F, "Fullscreen"),
    (Key::T, "Theater mode"),
    (Key::I, "Mini player"),
    (Key::M, "Mute"),
    (Key::C, "Captions"),
];

impl Binding {
    fn shortcut(&self) -> Option<KeyboardShortcut> {
        Some(KeyboardShortcut::new(
            Modifiers {
                alt: self.alt,
                shift: self.shift,
                command: self.command,
                ctrl: self.control,
                ..Modifiers::NONE
            },
            Key::from_name(&self.key)?,
        ))
    }
}

fn shortcut(prefs: &PlayerPreferences, action: Key) -> KeyboardShortcut {
    prefs
        .bindings
        .get(action.name())
        .and_then(Binding::shortcut)
        .unwrap_or(KeyboardShortcut::new(Modifiers::NONE, action))
}

pub(super) fn refresh_labels(ctx: &Context, prefs: &PlayerPreferences) {
    for &(action, _) in ACTIONS {
        let label = prefs
            .bindings
            .get(action.name())
            .and_then(Binding::shortcut)
            .map(|key| ctx.format_shortcut(&key));
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(("custom_key_label", action)), label));
    }
}

pub(super) fn label<'a>(ctx: &Context, text: &'a str) -> std::borrow::Cow<'a, str> {
    if let Some((name, suffix)) = text.strip_suffix(')').and_then(|s| s.rsplit_once(" (")) {
        let key = match suffix {
            "↑" => Some(Key::ArrowUp),
            "↓" => Some(Key::ArrowDown),
            s => Key::from_name(s),
        };
        if let Some(key) = key
            && let Some(Some(binding)) =
                ctx.data(|d| d.get_temp::<Option<String>>(egui::Id::new(("custom_key_label", key))))
        {
            return format!("{name} ({binding})").into();
        }
    }
    text.into()
}

pub(super) fn count(
    ctx: &Context,
    prefs: &PlayerPreferences,
    action: Key,
    repeated: bool,
) -> usize {
    let binding = shortcut(prefs, action);
    ctx.input_mut(|i| {
        let before = i.events.len();
        i.events.retain(|event| !matches!(event, Event::Key{key, modifiers, pressed:true, repeat, ..}
            if *key == binding.logical_key && modifiers.matches_exact(binding.modifiers) && (repeated || !repeat)));
        before - i.events.len()
    })
}

#[derive(Default)]
pub(super) struct Recorder {
    active: Option<Key>,
    error: Option<String>,
}
impl Recorder {
    pub fn show(&mut self, ui: &mut Form, prefs: &mut PlayerPreferences) {
        if let Some(action) = self.active {
            ui.label("Press the new shortcut, or Escape to cancel.");
            let key = ui.ctx().input_mut(|i| {
                let index = i.events.iter().position(|e| {
                    matches!(
                        e,
                        Event::Key {
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )
                })?;
                if let Event::Key { key, modifiers, .. } = i.events.remove(index) {
                    Some((key, modifiers))
                } else {
                    None
                }
            });
            if let Some((key, modifiers)) = key {
                if key == Key::Escape {
                    self.active = None;
                    self.error = None;
                } else if ACTIONS.iter().any(|(other, _)| {
                    let bound = shortcut(prefs, *other);
                    *other != action
                        && key == bound.logical_key
                        && modifiers.matches_exact(bound.modifiers)
                }) {
                    self.error = Some("Another player action already uses that shortcut.".into());
                } else if matches!(key, Key::Tab | Key::Enter)
                    || (modifiers.command
                        && matches!(key, Key::Comma | Key::Q | Key::W | Key::C | Key::F))
                {
                    self.error = Some("That shortcut is used by the window or library.".into());
                } else {
                    prefs.bindings.insert(
                        action.name().into(),
                        Binding {
                            key: key.name().into(),
                            alt: modifiers.alt,
                            shift: modifiers.shift,
                            control: modifiers.ctrl && !modifiers.command,
                            command: modifiers.command,
                        },
                    );
                    self.active = None;
                    self.error = None;
                }
            }
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        ui.grid("player_key_bindings", |ui| {
            for &(action, label) in ACTIONS {
                ui.label(label);
                let text = ui.ctx().format_shortcut(&shortcut(prefs, action));
                if ui
                    .selectable_label(self.active == Some(action), text)
                    .clicked()
                {
                    self.active = Some(action);
                    self.error = None;
                }
                ui.end_row();
            }
        });
        if ui.button("Restore default keyboard shortcuts").clicked() {
            prefs.bindings.clear();
            *self = Self::default();
        }
    }
}
