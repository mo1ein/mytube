use super::{dialogs::FileIntent, theme};
use crate::model::{Collection, ControlBarLayout, ProgressBarPosition};
use crate::player::Control;
use crossbeam_channel::Sender;
use eframe::egui::{self, Response, Ui};

pub(super) enum Item<A> {
    PlaybackSpeed {
        current: f64,
        commands: Sender<Control>,
    },
    Command {
        title: String,
        action: A,
        enabled: bool,
        checked: Option<bool>,
        destructive: bool,
    },
    Heading(String),
    Caption(String),
    Separator,
    Submenu {
        title: String,
        enabled: bool,
        items: Vec<Self>,
    },
}

impl<A> Item<A> {
    fn command(title: &str, action: A) -> Self {
        Self::choice(title, action, true, None, false)
    }

    fn choice(
        title: &str,
        action: A,
        enabled: bool,
        checked: Option<bool>,
        destructive: bool,
    ) -> Self {
        Self::Command {
            title: title.into(),
            action,
            enabled,
            checked,
            destructive,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlayerAction {
    Settings,
    Layout(ControlBarLayout),
    Position(ProgressBarPosition),
}

pub(super) fn player(
    layout: ControlBarLayout,
    position: ProgressBarPosition,
    speed: Option<(f64, Sender<Control>)>,
) -> Vec<Item<PlayerAction>> {
    let mut items = Vec::new();
    if let Some((current, commands)) = speed {
        items.push(Item::PlaybackSpeed { current, commands });
        items.push(Item::Separator);
    }
    items.extend([
        Item::Heading("Control bar".into()),
        Item::command("Player settings and tools…", PlayerAction::Settings),
    ]);
    items.extend(ControlBarLayout::ALL.map(|l| {
        Item::choice(
            l.label(),
            PlayerAction::Layout(l),
            true,
            Some(l == layout),
            false,
        )
    }));
    items.extend([
        Item::Separator,
        Item::Heading("Progress bar position".into()),
    ]);
    items.extend(ProgressBarPosition::ALL.map(|p| {
        Item::choice(
            p.label(),
            PlayerAction::Position(p),
            true,
            Some(p == position),
            false,
        )
    }));
    items
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LibraryAction {
    Play,
    Details,
    Browse,
    WatchLater,
    NewCollection,
    Collection(i64),
    ToggleSelection,
    ClearSelection,
    Reveal,
    CopyPaths,
    File(FileIntent),
}

pub(super) struct LibraryMenu<'a> {
    pub selected_here: bool,
    pub selection_count: usize,
    pub save_label: &'a str,
    pub range_ready: bool,
    pub files_idle: bool,
    pub collections: &'a [Collection],
}

impl LibraryMenu<'_> {
    pub fn items(&self) -> Vec<Item<LibraryAction>> {
        use LibraryAction::*;
        let group = self.selected_here && self.selection_count > 1;
        let mut items = if group {
            vec![
                Item::Caption(format!("{} selected", super::grouped(self.selection_count))),
                Item::Separator,
            ]
        } else {
            vec![
                Item::command("Play", Play),
                Item::command("Video details", Details),
                Item::command("Browse this folder", Browse),
                Item::Separator,
                Item::command(self.save_label, WatchLater),
            ]
        };
        let mut collections = vec![Item::command("New collection…", NewCollection)];
        if !self.collections.is_empty() {
            collections.push(Item::Separator);
            collections.extend(
                self.collections
                    .iter()
                    .map(|c| Item::command(&c.name, Collection(c.id))),
            );
        }
        items.push(Item::Submenu {
            title: "Add to collection".into(),
            enabled: self.range_ready,
            items: collections,
        });
        items.push(Item::command(
            if self.selected_here {
                "Deselect video"
            } else {
                "Select video"
            },
            ToggleSelection,
        ));
        if self.selection_count > 0 {
            items.push(Item::command("Clear selection", ClearSelection));
        }
        items.push(Item::Separator);
        if !group {
            items.push(Item::command("Open containing folder", Reveal));
        }
        items.push(Item::choice(
            if group {
                "Copy file paths"
            } else {
                "Copy file path"
            },
            CopyPaths,
            self.range_ready,
            None,
            false,
        ));
        let enabled = self.files_idle && self.range_ready;
        items.extend([
            Item::choice("Move to…", File(FileIntent::Move), enabled, None, false),
            Item::choice("Copy to…", File(FileIntent::Copy), enabled, None, false),
            Item::Separator,
            Item::choice("Delete…", File(FileIntent::Delete), enabled, None, true),
        ]);
        items
    }
}

pub(super) fn fallback<A: Copy>(
    response: &Response,
    width: f32,
    items: impl FnOnce() -> Vec<Item<A>>,
) -> Option<A> {
    let mut selected = None;
    theme::context_menu(response, width, |ui, rows| {
        render(ui, rows, &items(), &mut selected)
    });
    selected
}

pub(super) fn player_popup(
    surface: &Response,
    layout: ControlBarLayout,
    position: ProgressBarPosition,
    speed: impl FnOnce() -> Option<(f64, Sender<Control>)>,
) -> Option<egui::InnerResponse<Option<PlayerAction>>> {
    let width = 248_f32.min(surface.ctx.content_rect().width());
    theme::context_menu(surface, width, |ui, rows| {
        let mut selected = None;
        egui::ScrollArea::vertical()
            .max_height((ui.ctx().content_rect().height() - 24.).max(1.))
            .show(ui, |ui| {
                render(ui, rows, &player(layout, position, speed()), &mut selected);
            });
        selected
    })
}

fn playback_speed(
    ui: &mut Ui,
    rows: &mut theme::MenuRows,
    mut rate: f64,
    commands: &Sender<Control>,
) {
    use crate::player::{MAX_SPEED, MIN_SPEED, SPEED_STEP};

    ui.add_space(4.);
    let caption = ui.label(theme::strong(
        "Playback speed",
        13.,
        ui.visuals().text_color(),
    ));
    let right = egui::pos2(ui.max_rect().right(), caption.rect.center().y);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = ui.available_width();
            theme::slider(
                ui,
                egui::Slider::new(&mut rate, MIN_SPEED..=MAX_SPEED)
                    .step_by(SPEED_STEP)
                    .show_value(false),
            )
        })
        .inner
        .labelled_by(caption.id);
    if rows.add(response).changed() {
        let _ = commands.send(Control::Speed(rate));
    }
    ui.painter().text(
        right,
        egui::Align2::RIGHT_CENTER,
        format!("{rate:.2}×"),
        theme::timestamp_font(13.),
        ui.visuals().text_color(),
    );
    ui.add_space(4.);
}

fn render<A: Copy>(
    ui: &mut Ui,
    rows: &mut theme::MenuRows,
    items: &[Item<A>],
    selected: &mut Option<A>,
) {
    for item in items {
        match item {
            Item::PlaybackSpeed { current, commands } => {
                playback_speed(ui, rows, *current, commands)
            }
            Item::Command {
                title,
                action,
                enabled,
                checked,
                destructive,
            } => {
                let response = ui
                    .add_enabled_ui(*enabled, |ui| {
                        if let Some(on) = checked {
                            if !theme::interface_theme(ui.ctx()).glass_ui() {
                                return super::classic_settings::choice_item(ui, title, *on);
                            }
                            ui.selectable_label(*on, title)
                        } else if *destructive {
                            ui.button(theme::text(title, 14.0, theme::danger_text(ui)))
                        } else {
                            ui.button(title)
                        }
                    })
                    .inner;
                if rows.add(response).clicked() && *checked != Some(true) {
                    *selected = Some(*action);
                    ui.close();
                }
            }
            Item::Heading(title) => {
                ui.label(theme::strong(title, 12., theme::palette(ui).muted));
            }
            Item::Caption(title) => {
                ui.label(theme::text(title, 12., theme::palette(ui).muted));
            }
            Item::Separator => {
                ui.separator();
            }
            Item::Submenu {
                title,
                enabled,
                items,
            } => {
                ui.add_enabled_ui(*enabled, |ui| {
                    rows.submenu(ui, title, |ui, rows| {
                        // Keep the create action visible while scrolling a long list of collections.
                        if let Some(split) = items
                            .iter()
                            .position(|item| matches!(item, Item::Separator))
                        {
                            render(ui, rows, &items[..=split], selected);
                            egui::ScrollArea::vertical()
                                .max_height(240.)
                                .show(ui, |ui| render(ui, rows, &items[split + 1..], selected));
                        } else {
                            render(ui, rows, items, selected);
                        }
                    })
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_slider_adjusts_speed_without_closing_the_context_menu() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let (commands, received) = crossbeam_channel::unbounded();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 720.));
        let point = egui::pos2(40., 40.);
        let render = |events, rate| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let surface =
                        ui.interact(screen, ui.id().with("surface"), egui::Sense::click());
                    player_popup(
                        &surface,
                        ControlBarLayout::Classic,
                        ProgressBarPosition::Bottom,
                        || Some((rate, commands.clone())),
                    );
                },
            )
        };
        let button = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(vec![], 1.).drop_without_applying_deltas();
        render(
            vec![
                egui::Event::PointerMoved(point),
                button(point, egui::PointerButton::Secondary, true),
                button(point, egui::PointerButton::Secondary, false),
            ],
            1.,
        )
        .drop_without_applying_deltas();
        for style in crate::player::options::InterfaceTheme::ALL {
            theme::apply_style(&ctx, style, 62);
            render(vec![], 1.).drop_without_applying_deltas();
            let frame = render(vec![], 1.);
            let rail = frame.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(r)
                    if r.rect.width() > 60.
                        && r.rect.height() == 4.
                        && r.fill == egui::Color32::from_gray(80) =>
                {
                    Some(r.rect)
                }
                _ => None,
            });
            let filled = if style.glass_ui() {
                egui::Color32::from_rgb(10, 132, 255)
            } else {
                egui::Color32::from_gray(244)
            };
            let has_fill = frame.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(r) if r.rect.height() == 4. && r.fill == filled));
            if !style.glass_ui() {
                let rail = rail.expect("speed slider track");
                assert!(
                    !frame.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Rect(r) if r.stroke.width > 0. && r.rect.contains_rect(rail)
                            && r.rect.height() < 32.
                    )),
                    "the focused YouTube speed slider should be borderless"
                );
            }
            frame.drop_without_applying_deltas();
            assert!(has_fill, "{style:?} active track");
            let rail = rail.expect("speed slider track");
            assert!(
                rail.width() > 200.,
                "{style:?} rail width {:?}",
                rail.width()
            );
            let target = rail.lerp_inside(egui::vec2(0.82, 0.5));
            render(
                vec![
                    egui::Event::PointerMoved(target),
                    button(target, egui::PointerButton::Primary, true),
                ],
                1.,
            )
            .drop_without_applying_deltas();
            let Control::Speed(rate) = received.try_recv().expect("speed command from menu slider")
            else {
                panic!("expected speed change");
            };
            assert!((3.0..4.).contains(&rate));
            assert!((rate - crate::player::stepped_speed(rate)).abs() < f64::EPSILON * 4.);
            let end = rail.left_center();
            let dragging = render(vec![egui::Event::PointerMoved(end)], rate);
            assert!(dragging.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "0.25×"
                && text.pos.y < rail.top() && text.pos.x > rail.center().x)));
            dragging.drop_without_applying_deltas();
            assert!(
                matches!(received.try_recv(), Ok(Control::Speed(rate)) if rate == crate::player::MIN_SPEED)
            );
            render(
                vec![button(end, egui::PointerButton::Primary, false)],
                crate::player::MIN_SPEED,
            )
            .drop_without_applying_deltas();
            assert!(egui::Popup::is_any_open(&ctx));
        }
    }

    #[test]
    fn fallback_menu_cancels_with_escape_and_dispatches_the_chosen_row_once() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700., 500.));
        let point = egui::pos2(150., 140.);
        let click = |point, button| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: point,
                    button,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let mut chosen = Vec::new();
        let mut render = |events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let surface =
                        ui.interact(screen, ui.id().with("test_surface"), egui::Sense::click());
                    if let Some(action) = fallback(&surface, 225., || {
                        player(ControlBarLayout::Classic, ProgressBarPosition::Bottom, None)
                    }) {
                        chosen.push(action);
                    }
                },
            )
        };
        render(vec![]).drop_without_applying_deltas();
        render(click(point, egui::PointerButton::Secondary)).drop_without_applying_deltas();
        assert!(egui::Popup::is_any_open(&ctx));
        render(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }])
        .drop_without_applying_deltas();
        assert!(!egui::Popup::is_any_open(&ctx));
        render(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }])
        .drop_without_applying_deltas();
        render(click(egui::pos2(80., 80.), egui::PointerButton::Secondary))
            .drop_without_applying_deltas();
        assert!(egui::Popup::is_any_open(&ctx));
        let output = render(vec![]);
        let target = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(text) if text.galley.text() == "Floating" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        output.drop_without_applying_deltas();
        render(click(target, egui::PointerButton::Primary)).drop_without_applying_deltas();
        render(vec![]).drop_without_applying_deltas();
        assert_eq!(chosen, [PlayerAction::Layout(ControlBarLayout::Floating)]);
    }

    #[test]
    fn player_menu_checks_one_layout_and_one_progress_position() {
        for layout in ControlBarLayout::ALL {
            for position in ProgressBarPosition::ALL {
                let checked: Vec<_> = player(layout, position, None)
                    .into_iter()
                    .filter_map(|item| match item {
                        Item::Command {
                            action,
                            checked: Some(true),
                            ..
                        } => Some(action),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    checked,
                    [
                        PlayerAction::Layout(layout),
                        PlayerAction::Position(position)
                    ]
                );
            }
        }
    }

    #[test]
    fn group_menu_keeps_collection_ids_and_disables_busy_operations() {
        let collections = [
            Collection {
                id: 12,
                name: "Work".into(),
            },
            Collection {
                id: 91,
                name: "Work".into(),
            },
        ];
        for (range_ready, files_idle) in [(false, true), (true, false), (true, true)] {
            let items = LibraryMenu {
                selected_here: true,
                selection_count: 3,
                save_label: "Watch later",
                range_ready,
                files_idle,
                collections: &collections,
            }
            .items();
            assert!(!items.iter().any(|i| matches!(
                i,
                Item::Command {
                    action: LibraryAction::Play,
                    ..
                }
            )));
            let children = items
                .iter()
                .find_map(|i| match i {
                    Item::Submenu { enabled, items, .. } => {
                        assert_eq!(*enabled, range_ready);
                        Some(items)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(matches!(
                children[2],
                Item::Command {
                    action: LibraryAction::Collection(12),
                    ..
                }
            ));
            assert!(matches!(
                children[3],
                Item::Command {
                    action: LibraryAction::Collection(91),
                    ..
                }
            ));
            assert!(items.iter().any(|i| matches!(i, Item::Command {action: LibraryAction::File(FileIntent::Delete), destructive: true, enabled, ..} if *enabled == (range_ready && files_idle))));
        }
    }
}
