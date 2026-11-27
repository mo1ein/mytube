use super::{classic_settings, player_layout::SidebarStyle, settings_form::Form};
use crate::model::{Appearance, ControlBarLayout, ProgressBarPosition};
use eframe::egui::{self, Context, Ui};
use std::time::{Duration, Instant};

use crate::player::{
    Control, PlayerTool, Session, TrackKind,
    options::{
        Editor, EngineOption, InterfaceTheme, OPTIONS, PipParent, PlayerPreferences, Section,
        ToolbarItem, Transport,
    },
};

pub struct PlayerSettings {
    pub open: bool,
    #[cfg(target_os = "macos")]
    pub(super) native: super::macos::settings_glass::SettingsGlass,
    pub sidebar: Option<egui::Rect>,
    pub(super) reveal: f32,
    pub export_playlist: bool,
    pub music_request: bool,
    pub request_history: bool,
    pub clear_history: bool,
    confirm_history: bool,
    repeat: bool,
    pub history: Vec<crate::model::Video>,
    pub history_choice: Option<crate::model::Video>,
    pub section: Section,
    refresh: Option<Instant>,
    property: String,
    eq_name: String,
    jump_to: String,
    logo_region: [u32; 4],
    filter_name: String,
    filter_expression: String,
    audio_filter: bool,
    recorder: super::player_keys::Recorder,
    online: super::online_subtitles::OnlineSubtitles,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            open: false,
            #[cfg(target_os = "macos")]
            native: Default::default(),
            sidebar: None,
            reveal: 0.,
            export_playlist: false,
            music_request: false,
            request_history: false,
            clear_history: false,
            confirm_history: false,
            repeat: false,
            history: Vec::new(),
            history_choice: None,
            section: Section::Interface,
            refresh: None,
            property: String::new(),
            eq_name: String::new(),
            jump_to: String::new(),
            logo_region: [1, 1, 32, 16],
            filter_name: String::new(),
            filter_expression: String::new(),
            audio_filter: false,
            recorder: Default::default(),
            online: Default::default(),
        }
    }
}

impl PlayerSettings {
    pub(super) fn sidebar_geometry(&self) -> Option<(egui::Rect, egui::Rect)> {
        let panel = self.sidebar?;
        let travel = (1. - self.reveal) * panel.width();
        let sliding = panel.translate(egui::vec2(travel, 0.));
        Some((sliding, panel.intersect(sliding)))
    }

    pub(super) fn leading(&self, prefs: &PlayerPreferences) -> bool {
        if self.section == Section::Plugins {
            prefs.plugins_left
        } else {
            prefs.settings_left
        }
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        prefs: &mut PlayerPreferences,
        session: Option<&Session>,
        layout: &mut ControlBarLayout,
        position: &mut ProgressBarPosition,
        appearance: &mut Appearance,
    ) {
        self.online.update(session);
        let sidebar = self.sidebar_geometry();
        let visible = (self.open || (sidebar.is_some() && self.reveal > 0.))
            && sidebar.is_none_or(|(_, clip)| clip.is_positive());
        #[cfg(target_os = "macos")]
        let native = prefs.interface_theme.glass_ui() && self.native.available(ctx) && visible;
        #[cfg(target_os = "macos")]
        if !native {
            self.native.detach();
        }
        if !visible {
            return;
        }
        let glass = prefs.interface_theme.translucent() && prefs.glass_sidebar;
        let compact = prefs.compact_interface;
        let classic = prefs.interface_theme == InterfaceTheme::Classic;
        let title = if self.section == Section::Plugins {
            "Plugins"
        } else {
            "Player settings"
        };
        #[cfg(target_os = "macos")]
        if native {
            // Keep the platform host separate while the controls edit the settings model.
            let mut host = std::mem::take(&mut self.native);
            let keep_open = host.show(
                ctx,
                sidebar,
                self.section,
                compact,
                glass,
                |section, form| {
                    self.section = section;
                    self.section_contents(
                        &mut Form::Native(form),
                        prefs,
                        session,
                        layout,
                        position,
                        appearance,
                    );
                },
            );
            self.open &= keep_open;
            self.native = host;
            return;
        }
        let contents =
            |ui: &mut Ui| self.contents(ui, prefs, session, layout, position, appearance);
        let mut open = true;
        if let Some(geometry) = sidebar {
            open = super::player_layout::sidebar(
                ctx,
                egui::Id::new("player_settings_sidebar"),
                geometry,
                title,
                SidebarStyle {
                    glass,
                    compact,
                    radius: super::player_layout::settings_radius(ctx),
                    outlined: true,
                },
                contents,
            );
        } else if !ctx.embed_viewports() {
            let builder = egui::ViewportBuilder::default()
                .with_inner_size(if classic { [840., 640.] } else { [540., 620.] })
                .with_min_inner_size([360., 300.]);
            let builder = if classic {
                builder
                    .with_title("Settings")
                    .with_decorations(true)
                    .with_resizable(true)
                    // The native resize frame stays active under the Classic header on macOS.
                    .with_fullsize_content_view(true)
                    .with_title_shown(false)
                    .with_titlebar_shown(false)
                    .with_titlebar_buttons_shown(false)
            } else {
                builder
                    .with_title("Player settings and tools")
                    .with_fullsize_content_view(false)
                    .with_title_shown(false)
                    .with_titlebar_shown(false)
                    .with_titlebar_buttons_shown(false)
                    .with_decorations(false)
            };
            ctx.show_viewport_deferred(Self::window_id(), builder, super::settings_window);
        } else {
            let window = egui::Window::new("Player settings and tools")
                .id(egui::Id::new("player_preferences_window"))
                .open(&mut open)
                .collapsible(false)
                .default_width(if classic { 880. } else { 520. })
                .max_width(ctx.content_rect().width() - 24.);
            let window = if classic {
                window.max_height(ctx.content_rect().height() - 40.)
            } else {
                window
            };
            window.show(ctx, contents);
        }
        self.open &= open;
    }

    pub(super) fn window_id() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("player_preferences_window")
    }

    pub(super) fn window_contents(
        &mut self,
        ui: &mut Ui,
        prefs: &mut PlayerPreferences,
        session: Option<&Session>,
        layout: &mut ControlBarLayout,
        position: &mut ProgressBarPosition,
        appearance: &mut Appearance,
    ) {
        let open = !ui.input(|i| i.viewport().close_requested())
            && !ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::W)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)
            });
        if !open {
            self.open = false;
            return;
        }
        if prefs.interface_theme == InterfaceTheme::Classic {
            egui::CentralPanel::default()
                .frame(super::theme::layer(
                    classic_settings::window_surface(ui.visuals()),
                    16,
                    12,
                ))
                .show(ui, |ui| {
                    classic_settings::style(ui);
                    ui.horizontal(|ui| {
                        let heading = ui.label(super::theme::strong(
                            "Settings",
                            15.,
                            super::theme::palette(ui).text,
                        ));
                        let close = ui
                            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                classic_settings::close_button(ui, "Close settings")
                            })
                            .inner;
                        let drag_area = egui::Rect::from_min_max(
                            egui::pos2(heading.rect.left(), close.rect.top()),
                            egui::pos2(
                                close.rect.left() - ui.spacing().item_spacing.x,
                                close.rect.bottom(),
                            ),
                        );
                        super::window_handle(ui, "classic_settings_header", drag_area);
                        if close.clicked() {
                            self.open = false;
                        }
                    });
                    ui.separator();
                    self.contents(ui, prefs, session, layout, position, appearance)
                });
            return;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let heading = ui.add(egui::Label::new("Settings").sense(egui::Sense::drag()));
                if heading.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("×").on_hover_text("Close settings").clicked() {
                        self.open = false;
                    }
                });
            });
            self.contents(ui, prefs, session, layout, position, appearance);
        });
    }

    fn contents(
        &mut self,
        ui: &mut Ui,
        prefs: &mut PlayerPreferences,
        session: Option<&Session>,
        layout: &mut ControlBarLayout,
        position: &mut ProgressBarPosition,
        appearance: &mut Appearance,
    ) {
        if prefs.interface_theme == InterfaceTheme::Classic {
            let mut section = self.section;
            let in_player = self.sidebar.is_some();
            classic_settings::page(ui, &mut section, in_player, |ui, section| {
                self.section = section;
                self.section_contents(
                    &mut Form::Portable(ui),
                    prefs,
                    session,
                    layout,
                    position,
                    appearance,
                );
            });
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            for tab in Section::ALL {
                ui.selectable_value(&mut self.section, tab, tab.title());
            }
        });
        ui.separator();
        let height = if self.sidebar.is_some() {
            ui.available_height()
        } else {
            ui.available_height()
                .max(80.)
                .min((ui.ctx().content_rect().height() - 100.).max(80.))
        };
        egui::ScrollArea::vertical()
            .id_salt(self.section.title())
            .max_height(height.max(1.))
            .min_scrolled_height(height.clamp(1., 80.))
            .show(ui, |ui| {
                self.section_contents(
                    &mut Form::Portable(ui),
                    prefs,
                    session,
                    layout,
                    position,
                    appearance,
                )
            });
    }

    fn section_contents(
        &mut self,
        ui: &mut Form,
        prefs: &mut PlayerPreferences,
        session: Option<&Session>,
        layout: &mut ControlBarLayout,
        position: &mut ProgressBarPosition,
        appearance: &mut Appearance,
    ) {
        match self.section {
            Section::Plugins => self.plugins(ui, session),
            Section::Controls => self.controls(ui, prefs, layout),
            Section::Filters => self.filters(ui, prefs, session),
            Section::Interface => Self::interface(ui, prefs, position, appearance),
            Section::Playback => Self::playback(ui, prefs),
            Section::Inspector => self.inspector(ui, prefs, session),
            Section::Tools => self.tools(ui, prefs, session),
            section => self.media_options(ui, prefs, session, section),
        }
    }

    fn plugins(&mut self, ui: &mut Form, session: Option<&Session>) {
        ui.label("Local mpv scripts");
        let Some(session) = session else {
            ui.label("Open a video to load a script.");
            return;
        };
        let state = session.state();
        for path in state.scripts.iter() {
            ui.label(path.file_name().unwrap_or_default().to_string_lossy())
                .on_hover_text(path.display().to_string());
        }
        if ui
            .enabled_button(state.ready && !state.precise_seeking, "Load Lua script…")
            .clicked()
        {
            session.pick_script();
        }
    }

    fn controls(
        &mut self,
        ui: &mut Form,
        prefs: &mut PlayerPreferences,
        layout: &mut ControlBarLayout,
    ) {
        use crate::player::options::ClickAction;
        ui.group("Mouse", |ui| {
            for (label, current) in [
                ("Single click", &mut prefs.click),
                ("Double click", &mut prefs.double_click),
                ("Middle click", &mut prefs.middle_click),
            ] {
                ui.combo(label, current.title(), |ui| {
                    for action in ClickAction::ALL {
                        ui.selectable_value(current, action, action.title());
                    }
                });
            }
        });

        ui.group("Control bar", |ui| {
            ui.checkbox(&mut prefs.auto_hide, "Automatically hide the control bar");
            Self::control_hide_delay(ui, prefs);
        });
        ui.group("Player layout", |ui| {
            Self::layout_options(ui, prefs, layout)
        });
        ui.group("Keyboard shortcuts", |ui| self.recorder.show(ui, prefs));
    }

    fn filters(&mut self, ui: &mut Form, prefs: &mut PlayerPreferences, session: Option<&Session>) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.audio_filter, false, "Video filters");
            ui.selectable_value(&mut self.audio_filter, true, "Audio filters");
        });
        ui.text(&mut self.filter_name, "Filter name", false);
        ui.text(
            &mut self.filter_expression,
            if self.audio_filter {
                "mpv audio filter, e.g. lavfi=[highpass=f=100]"
            } else {
                "mpv video filter, e.g. hflip"
            },
            false,
        );
        if ui
            .enabled_button(
                !self.filter_name.trim().is_empty() && !self.filter_expression.trim().is_empty(),
                "Save filter",
            )
            .clicked()
        {
            prefs.filters.push(crate::player::options::FilterPreset {
                id: prefs.next_filter_id,
                name: self.filter_name.trim().into(),
                expression: self.filter_expression.trim().into(),
                audio: self.audio_filter,
            });
            prefs.next_filter_id += 1;
            self.filter_name.clear();
            self.filter_expression.clear();
        }
        let mut forget = None;
        for filter in prefs
            .filters
            .iter()
            .filter(|f| f.audio == self.audio_filter)
        {
            ui.push_id(filter.id, |ui| {
                ui.separator();
                ui.label(&filter.name).on_hover_text(&filter.expression);
                ui.horizontal_wrapped(|ui| {
                    if let Some(session) = session.filter(|s| s.state().ready) {
                        if ui.button("Apply").clicked() {
                            session.tool(PlayerTool::Filter(filter.clone(), true));
                        }
                        if ui.button("Remove from playback").clicked() {
                            session.tool(PlayerTool::Filter(filter.clone(), false));
                        }
                    }
                    if ui.button("Forget preset").clicked() {
                        forget = Some(filter.id);
                    }
                });
            });
        }
        prefs.filters.retain(|f| Some(f.id) != forget);
    }

    fn interface(
        ui: &mut Form,
        prefs: &mut PlayerPreferences,
        position: &mut ProgressBarPosition,
        appearance: &mut Appearance,
    ) {
        ui.group("Display", |ui| {
            ui.theme_picker(&mut prefs.interface_theme);
            ui.enabled(prefs.interface_theme.translucent(), |ui| {
                ui.number(
                    &mut prefs.glass_strength,
                    0..=100,
                    "Glass strength (%)",
                    Some(1.),
                    false,
                );
            });
            ui.combo("Appearance", appearance.label(), |ui| {
                for a in Appearance::ALL {
                    ui.selectable_value(appearance, a, a.label());
                }
            });

            ui.checkbox(&mut prefs.compact_interface, "Compact interface");
        });

        ui.group("Timeline", |ui| {
            ui.combo("Progress bar position", position.label(), |ui| {
                for option in ProgressBarPosition::ALL {
                    ui.selectable_value(position, option, option.label());
                }
            });
            ui.combo(
                "Progress bar style",
                prefs
                    .progress_bar_style
                    .map_or("Follow theme", |style| style.label()),
                |ui| {
                    ui.selectable_value(&mut prefs.progress_bar_style, None, "Follow theme");
                    for style in InterfaceTheme::ALL {
                        ui.selectable_value(
                            &mut prefs.progress_bar_style,
                            Some(style),
                            style.label(),
                        );
                    }
                },
            );

            ui.checkbox(&mut prefs.hover_previews, "Show timeline hover thumbnails");
            ui.checkbox(
                &mut prefs.show_chapters,
                "Show chapter markers on the timeline",
            );
        });

        ui.group("Window", |ui| {
            ui.checkbox(&mut prefs.always_on_top, "Keep window on top");
            ui.checkbox(&mut prefs.lock_aspect, "Lock window aspect ratio");
            Self::window_options(ui, prefs);
        });

        ui.collapsing("Feedback and previews", |ui| {
            ui.checkbox(&mut prefs.playback_feedback, "Enable on-screen feedback");
            ui.checkbox(
                &mut prefs.remote_thumbnails,
                "Prefetch player previews on network volumes",
            );
            ui.number(
                &mut prefs.thumbnail_cache_mb,
                1..=1024,
                "Timeline cache (MiB)",
                None,
                true,
            );
            ui.checkbox(&mut prefs.remaining_time, "Show remaining time");
            ui.enabled(prefs.remaining_time, |ui| {
                ui.checkbox(
                    &mut prefs.speed_adjusted_remaining,
                    "Adjust remaining time for playback speed",
                );
            });
            ui.enabled(prefs.hover_previews, |ui| {
                ui.number(
                    &mut prefs.thumbnail_width,
                    96..=384,
                    "Thumbnail width (pt)",
                    None,
                    false,
                );
            });
            ui.enabled(prefs.playback_feedback, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (flag, label) in [
                        (&mut prefs.feedback_on_open, "File start"),
                        (&mut prefs.feedback_on_pause, "Play / pause"),
                        (&mut prefs.feedback_on_seek, "Other seeks"),
                        (&mut prefs.feedback_on_speed, "Speed"),
                    ] {
                        ui.checkbox(flag, label);
                    }
                });
                ui.number(
                    &mut prefs.feedback_size,
                    12..=40,
                    "Feedback text size (pt)",
                    None,
                    false,
                );
                ui.number(
                    &mut prefs.feedback_duration_ms,
                    200..=5000,
                    "Feedback duration (ms)",
                    None,
                    false,
                );
            });
            ui.checkbox(
                &mut prefs.fullscreen_clock,
                "Show clock and battery in fullscreen",
            );
            ui.checkbox(&mut prefs.reduce_motion, "Disable player animations");
        });
    }

    fn layout_options(ui: &mut Form, prefs: &mut PlayerPreferences, layout: &mut ControlBarLayout) {
        ui.combo("Control bar layout", layout.label(), |ui| {
            for choice in ControlBarLayout::ALL {
                ui.selectable_value(layout, choice, choice.label());
            }
        });
        for (label, flag) in [
            ("Edge-to-edge video", &mut prefs.edge_to_edge),
            ("Settings on left in fullscreen", &mut prefs.settings_left),
            ("Playlist on left in fullscreen", &mut prefs.playlist_left),
            ("Plugins on left in fullscreen", &mut prefs.plugins_left),
        ] {
            ui.checkbox(flag, label);
        }
        ui.enabled(prefs.interface_theme.translucent(), |ui| {
            ui.checkbox(&mut prefs.glass, "Glass controller");
            ui.checkbox(&mut prefs.glass_osd, "Glass playback feedback");
            ui.checkbox(&mut prefs.glass_sidebar, "Glass sidebar");
        });
        ui.combo("Transport arrows", prefs.transport.title(), |ui| {
            for choice in [Transport::Playlist, Transport::Seek, Transport::Speed] {
                ui.selectable_value(&mut prefs.transport, choice, choice.title());
            }
        });
        if *layout == ControlBarLayout::Floating {
            ui.checkbox(
                &mut prefs.snap_controller,
                "Snap floating controls to center",
            );
        }

        ui.number(
            &mut prefs.sidebar_width,
            220..=450,
            "Preferred sidebar width",
            None,
            false,
        );
        ui.collapsing("Control bar buttons", |ui| toolbar(ui, &mut prefs.toolbar));
    }

    fn window_options(ui: &mut Form, prefs: &mut PlayerPreferences) {
        ui.enabled(!prefs.always_on_top, |ui| {
            ui.checkbox(&mut prefs.float_playing, "Float on top while playing");
        });
        ui.checkbox(
            &mut prefs.show_float_status,
            "Show float status in the window title",
        );
        super::player_window::settings(ui, &mut prefs.window);
    }

    fn control_hide_delay(ui: &mut Form, prefs: &mut PlayerPreferences) {
        ui.enabled(prefs.auto_hide, |ui| {
            ui.number(
                &mut prefs.hide_delay_ms,
                100..=10000,
                "Hide delay (ms)",
                None,
                false,
            );
        });
    }

    fn playback(ui: &mut Form, prefs: &mut PlayerPreferences) {
        ui.group("Behavior", |ui| {
            ui.checkbox(&mut prefs.resume, "Resume from last position");
            ui.checkbox(&mut prefs.autoplay, "Autoplay the next video");

            ui.checkbox(
                &mut prefs.pause_on_inactive,
                "Pause when MyTube loses focus",
            );
            ui.checkbox(
                &mut prefs.play_entering_fullscreen,
                "Play when entering fullscreen",
            );
            ui.checkbox(
                &mut prefs.pause_leaving_fullscreen,
                "Pause when leaving fullscreen",
            );
            ui.checkbox(&mut prefs.pause_on_open, "Pause when opening a video");

            ui.checkbox(&mut prefs.loop_playlist, "Repeat the playlist");
            ui.checkbox(&mut prefs.record_history, "Remember recently played videos");
        });
        ui.group("Picture-in-picture", |ui| {
            ui.checkbox(
                &mut prefs.pause_on_minimize,
                "Pause when entering the mini player",
            );
            ui.checkbox(
                &mut prefs.detached_mini,
                "Picture-in-picture in a separate floating window",
            );
            if prefs.detached_mini {
                ui.checkbox(
                    &mut prefs.pip_on_os_minimize,
                    "Enter picture-in-picture when the window is minimized",
                );
                ui.enabled(prefs.pip_on_os_minimize, |ui| {
                    ui.checkbox(
                        &mut prefs.pip_video_only,
                        "Only when a video track is playing",
                    );
                });
                ui.combo(
                    "When entering picture-in-picture",
                    prefs.pip_parent.title(),
                    |ui| {
                        for choice in [PipParent::Keep, PipParent::Minimize, PipParent::Hide] {
                            ui.selectable_value(&mut prefs.pip_parent, choice, choice.title());
                        }
                    },
                );
            }
        });
        ui.group("Seeking and volume", |ui| {
            ui.checkbox(
                &mut prefs.wheel_volume,
                "Adjust volume with the scroll wheel over playback controls",
            );
            ui.checkbox(
                &mut prefs.wheel_seek,
                "Scroll horizontally over the player to seek",
            );
            ui.number(
                &mut prefs.arrow_seek_seconds,
                1..=120,
                "Arrow key seek (seconds)",
                None,
                false,
            );
        });
    }

    fn inspector(
        &mut self,
        ui: &mut Form,
        prefs: &mut PlayerPreferences,
        session: Option<&Session>,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.text(&mut self.property, "mpv property to watch", false);
            if ui.button("Watch").clicked() {
                let name = self.property.trim();
                if !name.is_empty() && !prefs.watched_properties.iter().any(|item| item == name) {
                    prefs.watched_properties.push(name.to_owned());
                    self.property.clear();
                    self.refresh = None;
                }
            }
        });
        if let Some(session) = session.filter(|s| s.state().ready) {
            let due = self
                .refresh
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
            if due {
                session.inspect(&prefs.watched_properties);
                self.refresh = Some(Instant::now());
            }
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            let snapshot = session.state();
            ui.grid("player_info", |ui| {
                for (key, value) in snapshot.inspection.iter() {
                    ui.label(key);
                    ui.label(value);
                    if prefs.watched_properties.contains(key) && ui.button("Remove").clicked() {
                        prefs.watched_properties.retain(|k| k != key);
                        self.refresh = None;
                    }
                    ui.end_row();
                }
            });
        } else {
            prefs.watched_properties.retain(|name| {
                ui.push_id(name, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name);
                        !ui.button("Remove").clicked()
                    })
                })
            });
            ui.label("Open a video to inspect playback.");
        }
    }

    fn tools(&mut self, ui: &mut Form, prefs: &mut PlayerPreferences, session: Option<&Session>) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Save current playlist…").clicked() {
                self.export_playlist = true;
            }
            if ui.button("Show playback history").clicked() {
                self.request_history = true;
            }
        });
        if ui
            .enabled_button(session.is_some(), "Toggle music mode")
            .clicked()
        {
            self.music_request = true;
        }
        if !self.history.is_empty() {
            if self.confirm_history {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Clear the saved playback history?");
                    if ui.button("Clear history").clicked() {
                        self.clear_history = true;
                        self.confirm_history = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_history = false;
                    }
                });
            } else if ui.button("Clear history…").clicked() {
                self.confirm_history = true;
            }

            for video in &self.history {
                if ui
                    .button(&video.title)
                    .on_hover_text(video.path.display().to_string())
                    .clicked()
                {
                    self.history_choice = Some(video.clone());
                }
            }
        }
        ui.group("Screenshots", |ui| {
            ui.combo(
                "Screenshot format",
                prefs.screenshot_format.to_uppercase(),
                |ui| {
                    for ext in ["png", "jpg", "webp", "jxl", "avif"] {
                        ui.selectable_value(
                            &mut prefs.screenshot_format,
                            ext.to_owned(),
                            ext.to_uppercase(),
                        );
                    }
                },
            );
            ui.checkbox(
                &mut prefs.screenshot_subtitles,
                "Include subtitles in screenshots",
            );
        });
        if let Some(session) = session.filter(|s| s.state().ready) {
            let state = session.state();
            #[cfg(target_os = "macos")]
            ui.collapsing("Text in video", |ui| {
                if ui
                    .enabled_button(
                        !state.recognizing,
                        if state.recognizing {
                            "Reading frame…"
                        } else {
                            "Read text from current frame"
                        },
                    )
                    .clicked()
                {
                    session.tool(PlayerTool::ReadText);
                }
                if let Some(text) = &state.recognized_text {
                    if text.is_empty() {
                        ui.label("No text recognized in this frame.");
                    } else {
                        ui.label(text.as_ref());
                        if ui.button("Copy recognized text").clicked() {
                            ui.ctx().copy_text(text.to_string());
                        }
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Jump to start").clicked() {
                    session.send(Control::Seek(0.));
                }
                ui.text(&mut self.jump_to, "hh:mm:ss", false);
                let seconds = parse_time(&self.jump_to).filter(|&t| t <= state.duration);
                if ui
                    .enabled_button(seconds.is_some(), "Jump to time")
                    .clicked()
                    && let Some(at) = seconds
                {
                    session.send(Control::Seek(at));
                }
            });
            ui.enabled(!state.chapters.is_empty(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (next, label) in [(false, "Previous chapter"), (true, "Next chapter")] {
                        if ui.button(label).clicked() {
                            session.tool(PlayerTool::Chapter(next));
                        }
                    }
                });
            });
            ui.collapsing("Remove a logo", |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in self
                        .logo_region
                        .iter_mut()
                        .zip(["Left", "Top", "Width", "Height"])
                    {
                        ui.label(label);
                        ui.number(value, 1..=16384, label, None, false);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Apply rectangle").clicked() {
                        session.tool(PlayerTool::Delogo(Some(self.logo_region)));
                    }
                    if ui.button("Clear rectangle").clicked() {
                        session.tool(PlayerTool::Delogo(None));
                    }
                });
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label("Crop");
                for (label, aspect) in [
                    ("Original", None),
                    ("16:9", Some((16, 9))),
                    ("4:3", Some((4, 3))),
                    ("1:1", Some((1, 1))),
                    ("2.35:1", Some((235, 100))),
                ] {
                    if ui.button(label).clicked() {
                        session.tool(PlayerTool::Crop(aspect));
                    }
                }
            });
            if ui.button("Save screenshot…").clicked() {
                session.pick_tool_file(None, prefs.screenshot_subtitles, &prefs.screenshot_format);
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Set A / B loop point").clicked() {
                    session.tool(PlayerTool::LoopPoint);
                }
                if ui.button("Clear A–B loop").clicked() {
                    session.tool(PlayerTool::ClearLoop);
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Previous frame").clicked() {
                    session.send(Control::Frame(false));
                }
                if ui.button("Next frame").clicked() {
                    session.send(Control::Frame(true));
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Mirror horizontally").clicked() {
                    session.tool(PlayerTool::Flip(true));
                }
                if ui.button("Flip vertically").clicked() {
                    session.tool(PlayerTool::Flip(false));
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Previous subtitle").clicked() {
                    session.tool(PlayerTool::SubtitleStep(false));
                }
                if ui.button("Next subtitle").clicked() {
                    session.tool(PlayerTool::SubtitleStep(true));
                }
            });
        } else {
            ui.label("Open a video to use playback tools.");
        }
    }

    fn media_options(
        &mut self,
        ui: &mut Form,
        prefs: &mut PlayerPreferences,
        session: Option<&Session>,
        section: Section,
    ) {
        let state = session.map(Session::state).unwrap_or_default();
        let session = session.filter(|_| state.ready);
        ui.enabled(session.is_some(), |ui| {
            if section == Section::Video {
                ui.group("Playback", |ui| {
                    if ui.checkbox(&mut self.repeat, "Repeat video").changed()
                        && let Some(session) = session
                    {
                        session.send(Control::Repeat(self.repeat));
                    }
                    if let Some(speed) = ui.playback_speed(state.speed)
                        && let Some(session) = session
                    {
                        session.send(Control::Speed(speed));
                    }
                });
            }
            let (kind, selected, load) = match section {
                Section::Audio => (TrackKind::Audio, state.audio, "Load external audio…"),
                Section::Video => (TrackKind::Video, state.video_track, "Load external video…"),
                _ => (TrackKind::Subtitle, state.subtitle, "Load subtitle file…"),
            };
            ui.group("Tracks", |ui| {
                for (label, current) in [
                    ("Track", selected),
                    ("Secondary subtitle", state.secondary_subtitle),
                ] {
                    let secondary = label == "Secondary subtitle";
                    if secondary && kind != TrackKind::Subtitle {
                        continue;
                    }
                    let choices = state.tracks.iter().filter(|t| t.kind == kind);
                    let mut choice = current;
                    ui.combo(
                        label,
                        choices
                            .clone()
                            .find(|t| Some(t.id) == current)
                            .map_or("Off", |t| t.title.as_str()),
                        |ui| {
                            ui.selectable_value(&mut choice, None, "Off");
                            for track in choices {
                                ui.selectable_value(&mut choice, Some(track.id), &track.title);
                            }
                        },
                    );
                    if choice != current
                        && let Some(session) = session
                    {
                        if secondary {
                            session.tool(PlayerTool::SecondarySubtitle(choice));
                        } else {
                            session.send(Control::Track(kind, choice));
                        }
                    }
                }
                if section == Section::Subtitles {
                    self.online.show(ui);
                }
                if ui.button(load).clicked()
                    && let Some(session) = session
                {
                    session.pick_tool_file(Some(kind), false, "");
                }
            });
        });
        if section == Section::Audio {
            ui.collapsing("10-band equalizer", |ui| {
                for (index, frequency) in crate::player::options::EQ_FREQUENCIES
                    .into_iter()
                    .enumerate()
                {
                    ui.number(
                        &mut prefs.equalizer[index],
                        -12..=12,
                        format!("{frequency} Hz"),
                        None,
                        false,
                    );
                }
                if ui.button("Flat").clicked() {
                    prefs.equalizer = [0; 10];
                }
                ui.horizontal_wrapped(|ui| {
                    ui.text(&mut self.eq_name, "Preset name", false);
                    if ui
                        .enabled_button(!self.eq_name.trim().is_empty(), "Save preset")
                        .clicked()
                    {
                        prefs
                            .eq_presets
                            .insert(self.eq_name.trim().to_owned(), prefs.equalizer);
                        self.eq_name.clear();
                    }
                });
                let mut remove = None;
                for (name, gains) in &prefs.eq_presets {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .selectable_label(*gains == prefs.equalizer, name)
                            .clicked()
                        {
                            prefs.equalizer = *gains;
                        }
                        if ui.button("Remove").clicked() {
                            remove = Some(name.clone());
                        }
                    });
                }
                if let Some(name) = remove {
                    prefs.eq_presets.remove(&name);
                }
            });
        }
        let available = OPTIONS.iter().filter(|opt| opt.section == section);
        let common = |key: &str| {
            matches!(
                key,
                "brightness" | "contrast" | "audio-delay" | "sub-delay" | "sub-scale"
            )
        };
        let group = match section {
            Section::Video => "Picture",
            Section::Audio => "Timing",
            _ => "Appearance and timing",
        };
        ui.group(group, |ui| {
            if section == Section::Subtitles {
                ui.number(
                    &mut prefs.caption_position,
                    0..=100,
                    "Vertical position (%)",
                    None,
                    false,
                );
            }
            for option in available.clone().filter(|opt| common(opt.key)) {
                ui.enabled(
                    session.is_none_or(|s| s.supports_option(option.key)),
                    |ui| {
                        ui.push_id(option.key, |ui| edit_option(ui, option, prefs));
                    },
                );
            }
        });
        ui.collapsing(
            &format!("Advanced {} options", section.title().to_lowercase()),
            |ui| {
                for option in available.filter(|opt| !common(opt.key)) {
                    ui.enabled(
                        session.is_none_or(|s| s.supports_option(option.key)),
                        |ui| {
                            ui.push_id(option.key, |ui| edit_option(ui, option, prefs));
                        },
                    );
                }
                if ui
                    .button(format!("Reset {} settings", section.title().to_lowercase()))
                    .clicked()
                {
                    if section == Section::Audio {
                        prefs.equalizer = [0; 10];
                    }
                    if section == Section::Subtitles {
                        prefs.caption_position = 100;
                    }
                    for option in OPTIONS.iter().filter(|opt| opt.section == section) {
                        prefs.engine.remove(option.key);
                    }
                }
            },
        );
    }
}

fn parse_time(text: &str) -> Option<f64> {
    let parts: Vec<_> = text.trim().split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut seconds = 0.;
    for (i, part) in parts.iter().enumerate() {
        let value: f64 = part.parse().ok()?;
        if !value.is_finite()
            || value < 0.
            || (i != 0 && value >= 60.)
            || (i + 1 < parts.len() && value.fract() != 0.)
        {
            return None;
        }
        seconds = seconds * 60. + value;
    }
    Some(seconds)
}

fn edit_option(ui: &mut Form, option: &EngineOption, prefs: &mut PlayerPreferences) {
    let mut value = option.value(prefs).to_owned();
    let changed = match option.editor {
        Editor::Number(min, max, step) => {
            let mut number = value.parse().unwrap_or(min);
            let changed = ui
                .number(&mut number, min..=max, option.label, Some(step), false)
                .changed();
            if changed {
                value = number.to_string();
            }
            changed
        }
        Editor::Choice(choices) => {
            let current = choices
                .iter()
                .find(|(key, _)| *key == value)
                .map_or(value.as_str(), |(_, label)| *label)
                .to_owned();
            ui.combo(option.label, current, |ui| {
                let mut changed = false;
                for &(key, label) in choices {
                    changed |= ui
                        .selectable_value(&mut value, key.to_owned(), label)
                        .changed();
                }
                changed
            })
            .unwrap_or(false)
        }
        Editor::Color => {
            let argb = u32::from_str_radix(value.trim_start_matches('#'), 16)
                .unwrap_or_default()
                .to_be_bytes();
            let mut rgba = [argb[1], argb[2], argb[3], argb[0]];
            let changed = ui.color(&mut rgba, option.label).changed();
            if changed {
                value = format!(
                    "#{:08X}",
                    u32::from_be_bytes([rgba[3], rgba[0], rgba[1], rgba[2]])
                );
            }
            changed
        }
        Editor::Text => ui.labeled_text(&mut value, option.label).changed(),
    };
    if changed && option.accepts(&value) {
        if value == option.default {
            prefs.engine.remove(option.key);
        } else {
            prefs.engine.insert(option.key.into(), value);
        }
    }
}

fn toolbar(ui: &mut Form, items: &mut Vec<ToolbarItem>) {
    for index in 0..items.len() {
        let item = items[index];
        let mut edited = false;
        ui.horizontal_wrapped(|ui| {
            let mut visible = true;
            if ui.checkbox(&mut visible, item.title()).changed() {
                items.remove(index);
                edited = true;
            } else {
                if ui
                    .enabled_button(index > 0, "↑")
                    .on_hover_text("Move earlier")
                    .clicked()
                {
                    items.swap(index, index - 1);
                    edited = true;
                }
                if ui
                    .enabled_button(index + 1 < items.len(), "↓")
                    .on_hover_text("Move later")
                    .clicked()
                {
                    items.swap(index, index + 1);
                    edited = true;
                }
            }
        });
        if edited {
            return;
        }
    }
    for item in ToolbarItem::all() {
        if !items.contains(&item) && ui.checkbox(&mut false, item.title()).clicked() {
            items.push(item);
        }
    }
    if ui.button("Restore default buttons").clicked() {
        *items = ToolbarItem::DEFAULTS.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_controls_fit_after_the_sidebar_is_resized() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        crate::ui::theme::install(&ctx);
        let mut settings = PlayerSettings {
            open: true,
            reveal: 1.,
            ..Default::default()
        };
        for style in crate::player::options::InterfaceTheme::ALL {
            let mut prefs = PlayerPreferences {
                interface_theme: style,
                ..Default::default()
            };
            crate::ui::theme::apply_style(&ctx, style, prefs.glass_strength);
            for width in [358., 280., 238., 520.] {
                // Keep the controls under test inside the vertical scroll viewport.
                let rect =
                    egui::Rect::from_min_size(egui::pos2(20., 20.), egui::vec2(width, 1000.));
                settings.sidebar = Some(rect);
                let mut draw = || {
                    ctx.run_ui(Default::default(), |_| {
                        settings.show(
                            &ctx,
                            &mut prefs,
                            None,
                            &mut Default::default(),
                            &mut Default::default(),
                            &mut Default::default(),
                        );
                    })
                };
                draw().drop_without_applying_deltas();
                let output = draw();
                let navigation = ["General", "Playback", "Video"];
                for label in navigation
                    .iter()
                    .copied()
                    .chain(["Progress bar position", "Progress bar style"])
                    .chain(style.translucent().then_some("Glass strength (%)"))
                {
                    let shape = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::Shape::Text(t) if t.galley.text() == label => Some(t),
                            _ => None,
                        })
                        .unwrap_or_else(|| {
                            let labels: Vec<_> = output
                                .shapes
                                .iter()
                                .filter_map(|s| match &s.shape {
                                    egui::Shape::Text(t) => Some((t.galley.text(), t.pos)),
                                    _ => None,
                                })
                                .collect();
                            panic!("{style:?} at width {width} is missing {label}: {labels:?}")
                        });
                    assert!(
                        shape.pos.x + shape.galley.size().x <= rect.right(),
                        "{label} overflows at width {width}"
                    );
                }
                if style == InterfaceTheme::Classic {
                    for (_, node) in &output
                        .platform_output
                        .accesskit_update
                        .as_ref()
                        .unwrap()
                        .nodes
                    {
                        if matches!(
                            node.role(),
                            egui::accesskit::Role::CheckBox | egui::accesskit::Role::Slider
                        ) && let Some(bounds) = node.bounds()
                        {
                            assert!(
                                bounds.x0 >= f64::from(rect.left())
                                    && bounds.x1 <= f64::from(rect.right()),
                                "{:?} extends outside the {width}-point sidebar: {bounds:?}",
                                node.label()
                            );
                        }
                    }
                }
                output.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn classic_settings_use_the_same_controls_in_wide_and_player_layouts() {
        use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, vec2};

        for (width, sidebar) in [(840., false), (595., false), (380., false), (340., true)] {
            let ctx = Context::default();
            super::super::theme::install(&ctx);
            let mut settings = PlayerSettings {
                open: true,
                section: Section::Interface,
                ..Default::default()
            };
            let mut prefs = PlayerPreferences {
                interface_theme: InterfaceTheme::Classic,
                ..Default::default()
            };
            let mut appearance = Appearance::default();
            let viewport_width = std::cell::Cell::new(width);
            let viewport_height = std::cell::Cell::new(580.);
            let mut render = |events| {
                let frame = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            vec2(viewport_width.get(), viewport_height.get()),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        settings.sidebar = sidebar.then(|| ui.available_rect_before_wrap());
                        let draw = if sidebar {
                            PlayerSettings::contents
                        } else {
                            PlayerSettings::window_contents
                        };
                        draw(
                            &mut settings,
                            ui,
                            &mut prefs,
                            None,
                            &mut Default::default(),
                            &mut Default::default(),
                            &mut appearance,
                        );
                    },
                );
                let positions = frame
                    .shapes
                    .iter()
                    .filter_map(|item| match &item.shape {
                        Shape::Text(text) => Some((
                            text.galley.text().to_owned(),
                            text.visual_bounding_rect().center(),
                        )),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                frame.drop_without_applying_deltas();
                (
                    positions,
                    appearance,
                    prefs.compact_interface,
                    settings.section,
                )
            };
            let position = |labels: &[(String, Pos2)], label| {
                labels
                    .iter()
                    .find_map(|(name, point)| (name == label).then_some(*point))
                    .unwrap_or_else(|| panic!("Missing {label} at width {width}"))
            };
            let click = |pos| {
                let button = |pressed| Event::PointerButton {
                    pos,
                    pressed,
                    button: PointerButton::Primary,
                    modifiers: Modifiers::NONE,
                };
                vec![Event::PointerMoved(pos), button(true), button(false)]
            };
            let key = |key| Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            render(vec![]);
            let (labels, initial_appearance, compact, section) = render(vec![]);
            assert!(section == Section::Interface);
            if !sidebar {
                position(&labels, "Settings");
                assert!(!labels.iter().any(|(label, _)| label.contains("MyTube")));
                assert!(
                    !labels
                        .iter()
                        .any(|(label, _)| label == "Controls and layout")
                );
            }
            if width >= 635. {
                position(&labels, "Float on top while playing");
                assert!(
                    !labels
                        .iter()
                        .any(|(label, _)| label == "More window options")
                );
            }
            let general = position(&labels, "General");
            let next_category = position(&labels, "Playback");
            if width < 635. {
                assert!(next_category.x > general.x && (next_category.y - general.y).abs() < 2.);
            } else {
                assert!(next_category.y > general.y && (next_category.x - general.x).abs() < 12.);
                let heading = labels
                    .iter()
                    .find(|(label, pos)| label == "General" && pos.x > general.x)
                    .expect("General section heading");
                assert!(
                    (heading.1.y - general.y).abs() <= 3.,
                    "{heading:?} / {general:?}"
                );
            }
            let (_, _, toggled_compact, _) = render(click(position(&labels, "Compact interface")));
            assert_ne!(toggled_compact, compact);
            let auto = position(&labels, initial_appearance.label());
            render(click(auto));
            let (labels, _, _, _) = render(vec![]);
            let light_option = position(&labels, "Light");
            let dark_option = position(&labels, "Dark");
            assert!(
                (dark_option.y - light_option.y - 40.).abs() <= 1.5,
                "{width}: {light_option:?} {dark_option:?}"
            );
            assert!((dark_option.x - light_option.x).abs() < 5.);
            render(vec![key(Key::ArrowDown)]);
            let (_, appearance, compact_after, _) = render(vec![key(Key::Enter)]);
            assert_eq!(appearance, Appearance::Light);
            assert_eq!(compact_after, toggled_compact);
            if width >= 635. {
                let focused = ctx
                    .memory(|memory| memory.focused())
                    .expect("focused appearance dropdown");
                viewport_width.set(595.);
                let (narrow, _, _, _) = render(vec![]);
                assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused));
                let group = position(&narrow, "Display");
                let setting = position(&narrow, "Interface theme");
                assert!(setting.y > group.y && (setting.x - group.x).abs() < 25.);
                viewport_width.set(width);
                render(vec![]);
                assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused));
            }
            let (labels, _, _, _) = render(vec![]);
            render(click(position(&labels, "Light")));
            let (labels, _, _, _) = render(vec![]);
            let (_, appearance, _, _) = render(click(position(&labels, "Dark")));
            assert_eq!(appearance, Appearance::Dark);
            let (_, _, _, section) = render(click(next_category));
            assert!(section == Section::Playback);
            let (labels, _, _, _) = render(vec![]);
            if sidebar {
                let (_, _, _, section) = render(click(position(&labels, "Video")));
                assert!(section == Section::Video);
            } else {
                if width >= 635. {
                    for tab in Section::ALL {
                        position(&labels, tab.title());
                    }
                    render(click(position(&labels, "Controls")));
                } else {
                    // Navigate past the initially visible portion of the horizontal tab strip.
                    for expected in &Section::ALL[2..=5] {
                        let (_, _, _, selected) = render(vec![key(Key::ArrowRight)]);
                        assert_eq!(selected.title(), expected.title());
                    }
                }
                viewport_height.set(1400.);
                let (labels, _, _, _) = render(vec![]);
                position(&labels, "Player layout");
                position(&labels, "Control bar layout");
                viewport_height.set(1400.);
                let (labels, _, _, _) = render(vec![]);
                position(&labels, "Keyboard shortcuts");
                position(&labels, "Play / pause (alternate)");
                let close = position(&labels, "×");
                assert!(close.x > width - 48. && close.y < 50.);
                render(click(close));
                assert!(!settings.open);
            }
        }
    }

    #[test]
    fn edits_round_trip_between_main_and_playback_settings_in_each_theme() {
        use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};

        let ctx = Context::default();
        super::super::theme::install(&ctx);
        ctx.enable_accesskit();
        let mut settings = PlayerSettings {
            open: true,
            ..Default::default()
        };
        let mut prefs = PlayerPreferences::default();
        for theme in InterfaceTheme::ALL {
            prefs.interface_theme = theme;
            super::super::theme::apply_style(&ctx, theme, prefs.glass_strength);
            let mut draw = |sidebar, events| {
                let width = if sidebar { 360. } else { 840. };
                let frame = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 1200.))),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        settings.sidebar = sidebar.then(|| ui.available_rect_before_wrap());
                        let render = if sidebar {
                            PlayerSettings::contents
                        } else {
                            PlayerSettings::window_contents
                        };
                        render(
                            &mut settings,
                            ui,
                            &mut prefs,
                            None,
                            &mut Default::default(),
                            &mut Default::default(),
                            &mut Default::default(),
                        );
                    },
                );
                let checkbox = frame
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .find_map(|(_, node)| {
                        if node.label() == Some("Compact interface") {
                            Some((node.bounds()?, node.toggled()?))
                        } else {
                            None
                        }
                    });
                frame.drop_without_applying_deltas();
                checkbox.expect("compact interface checkbox")
            };
            for (source, destination) in [(false, true), (true, false)] {
                draw(source, vec![]);
                let (rect, before) = draw(source, vec![]);
                let pos = egui::pos2((rect.x0 + 5.) as f32, ((rect.y0 + rect.y1) / 2.) as f32);
                let button = |pressed| Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                };
                draw(source, vec![Event::PointerMoved(pos), button(true)]);
                let (_, edited) = draw(source, vec![button(false)]);
                assert_ne!(edited, before, "{theme:?} sidebar={source}");
                draw(destination, vec![]);
                assert_eq!(draw(destination, vec![]).1, edited);
            }
        }
    }

    #[test]
    fn jump_accepts_fractional_seconds_and_valid_clock_components() {
        for (input, expected) in [("1:02:03.5", 3723.5), ("04:03", 243.), ("90.25", 90.25)] {
            assert_eq!(parse_time(input), Some(expected));
        }
        for invalid in ["", "1:60", "1:2:3:4", "1.5:12", "-1", "inf", "NaN"] {
            assert_eq!(parse_time(invalid), None);
        }
    }
}
