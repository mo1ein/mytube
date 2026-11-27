use std::time::{Duration, Instant};

use eframe::{
    egui::{
        self, Align, Align2, Color32, Context, Id, Layout, Pos2, Rect, Sense, Slider, Stroke, Ui,
        ViewportCommand, pos2, vec2,
    },
    glow,
};

use crate::{
    model::{
        Appearance, Collection, CollectionTarget, ControlBarLayout, ProgressBarPosition, Video,
    },
    player::{Chapter, Control, GlProc, Glass, Playback, Session, TrackKind, VideoMask},
    service::{
        Action, Service, Settings, TimelineKey, TimelineOrigin, TimelinePrefetch, TimelineRequest,
    },
};

use super::{
    control_bar::{self, Bar, DisplayMaterial},
    dialogs::video_metadata,
    library::{self, Thumbnails},
    player_settings::PlayerSettings,
    precise_seek::{
        self, Finish, Gesture, HeldScrub, MAX_FILMSTRIP_TILES, PreciseSeek, SCRUB_THRESHOLD,
    },
    preview::{Peek, PeekPlacement},
    theme::{self, ButtonKind::*, Icon, button, icon_button, strong, text},
};
use crate::player::options::{
    InterfaceTheme, OPTIONS, PipParent, PlayerPreferences, Section, ToolbarItem, Transport,
};

const WATCH_MARGIN: f32 = 16.;
const MIN_CONTROLS_WIDTH: f32 = 550.0;
const CONTROLS_EXIT: Duration = Duration::from_millis(300);
const CONTROLS_FADE: Duration = Duration::from_millis(140);
const CLASSIC_VOLUME_ID: &str = "classic_volume_slider";
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Checkpoint {
    saved: Option<f64>,
    paused: bool,
    ended: bool,
    last_write: Option<Instant>,
}

impl Checkpoint {
    fn update(&mut self, state: &Playback, now: Instant, flush: bool) -> Option<Option<f64>> {
        let since = *self.last_write.get_or_insert(now);
        let just_paused = state.paused && !self.paused;
        let just_ended = state.ended && !self.ended;
        self.paused = state.paused;
        self.ended = state.ended;
        let value = if state.ended {
            None
        } else if state.ready && (flush || state.paused || now - since >= CHECKPOINT_INTERVAL) {
            Some(state.confirmed_position.filter(|at| at.is_finite())?).filter(|at| *at > 0.0)
        } else {
            return None;
        };
        if self.saved == value && !just_paused && !just_ended {
            return None;
        }
        self.saved = value;
        self.last_write = Some(now);
        Some(value)
    }
}

struct RailDrag {
    rail: Id,
    origin: Pos2,
    position: f64,
}

struct SurfaceDrag {
    held: HeldScrub,
    touch_scroll: bool,
    position: Option<f64>,
}

#[derive(Default)]
struct SeekBar {
    drag: Option<RailDrag>,
    surface_drag: Option<SurfaceDrag>,
    pending: Option<(f64, Instant)>,
    precise: PreciseSeek,
}

impl SeekBar {
    fn position(&mut self, state: &Playback, ctx: &Context) -> f64 {
        if let Some((target, expires)) = self.pending {
            let remaining = expires.saturating_duration_since(Instant::now());
            if !remaining.is_zero()
                && state.error.is_none()
                && (state.position - target).abs() > 0.35
            {
                ctx.request_repaint_after(remaining);
                return target;
            }
            self.pending = None;
        }
        state.position
    }

    fn seek(&mut self, session: &Session, position: f64, duration: f64) -> f64 {
        let position = position.clamp(0.0, duration.max(0.0));
        self.mark_pending(position);
        session.send(Control::Seek(position));
        position
    }

    fn mark_pending(&mut self, position: f64) {
        self.pending = Some((position, Instant::now() + Duration::from_secs(1)));
    }

    fn mark_precise_pending(&mut self, command: &Control) {
        if let Control::FinishPrecise(_, finish) = command
            && let Some(at) = finish.position()
        {
            self.mark_pending(at);
        }
    }

    fn surface_motion(
        &mut self,
        pointer: &ScrubPointer,
        state: &Playback,
        end: f64,
    ) -> Option<Control> {
        let drag = self.surface_drag.as_mut()?;
        if pointer.cancelled || (!pointer.down && pointer.release.is_none()) {
            self.surface_drag = None;
            return None;
        }
        let point = pointer.release.or(pointer.point)?;
        let delta = point - drag.held.origin;
        let vertical = delta.y.abs() > delta.x.abs() * 1.5;
        if drag.position.is_none()
            && drag.touch_scroll
            && vertical
            && delta.y.abs() >= SCRUB_THRESHOLD
        {
            // A vertical swipe scrolls the watch page. A mouse can lift straight into the filmstrip.
            self.surface_drag = None;
            return None;
        }
        if precise_seek::pulled_up(drag.held.origin, point) {
            let position = drag.held.enter_precise(point, end, None);
            let held = drag.held;
            self.surface_drag = None;
            return Some(
                self.precise
                    .start(state, position, end, pointer.down.then_some(held)),
            );
        }
        if drag.position.is_none() && (delta.x.abs() < SCRUB_THRESHOLD || vertical) {
            if pointer.release.is_some() {
                self.surface_drag = None;
            }
            return None;
        }
        if pointer.release.is_some() {
            let position = drag.held.update(point, end);
            self.surface_drag = None;
            self.mark_pending(position);
            return Some(Control::Seek(position));
        }
        drag.position = Some(drag.held.update(point, end));
        None
    }

    fn surface_position(&self) -> Option<f64> {
        self.surface_drag.as_ref().and_then(|drag| drag.position)
    }

    fn reset(&mut self) {
        self.drag = None;
        self.surface_drag = None;
        self.pending = None;
        self.precise.finish(Finish::Cancel);
    }
}

#[derive(Default)]
pub struct Queue {
    pub first: Option<usize>,
    pub previous: Option<usize>,
    pub next: Option<usize>,
}

impl Queue {
    pub fn for_video(browser: &library::Browser, video: Option<&Video>) -> Self {
        let Some(index) = video.and_then(|v| browser.position(v.id)) else {
            return Self::default();
        };
        let total = browser.total().unwrap_or_default();
        Self {
            first: (total > 0).then_some(0),
            previous: index.checked_sub(1),
            next: index.checked_add(1).filter(|&next| next < total),
        }
    }
}

enum Feedback {
    Volume,
    Speed,
    Seek(f64),
    File,
}

pub struct Watch {
    ctx: Context,
    loader: Option<GlProc>,
    session: Option<Session>,
    video: Option<Video>,
    error: Option<String>,
    seeking: SeekBar,
    #[cfg(target_os = "macos")]
    pub(super) playlist_glass: super::macos::controller_glass::ControllerGlass,
    #[cfg(target_os = "macos")]
    pub(super) controller_glass: super::macos::controller_glass::ControllerGlass,
    #[cfg(target_os = "macos")]
    native_player_menu: Option<super::macos::context_menu::PlayerMenu>,
    #[cfg(target_os = "macos")]
    pub(super) menu_host: super::macos::context_menu::MenuHost,
    control_bar_layout: ControlBarLayout,
    progress_bar_position: ProgressBarPosition,
    floating_position: Option<egui::Vec2>,
    prefs: PlayerPreferences,
    saved_prefs: PlayerPreferences,
    pub settings_panel: PlayerSettings,
    sidebars: super::player_layout::Sidebars,
    playlist_open: bool,
    toggle_playlist: bool,
    chapters_tab: bool,
    reveal_playlist_item: bool,
    controls_until: Instant,
    pointer_inside: bool,
    window_state: Option<(bool, bool)>,
    caption_position: u8,
    fullscreen: bool,
    theater: bool,
    pub(super) music: bool,
    mini: bool,
    pip_visible: bool,
    auto_pip: bool,
    os_minimized: bool,
    pip_parent: PipParent,

    request: Option<usize>,
    end_handled: bool,
    last_paused: Option<bool>,
    playback_flash: Option<(bool, Instant)>,
    feedback: Option<(Feedback, Instant)>,
    feedback_speed: Option<f64>,
    feedback_seeks: u64,
    battery: super::power::Battery,
    volume_scroll: f32,
    peek: Peek,
    prefetched_video: Option<(i64, i64)>,
    new_collection: Option<i64>,
    checkpoint: Checkpoint,
    #[cfg(target_os = "macos")]
    now_playing: Option<super::macos::NowPlaying>,
}

impl Watch {
    pub fn new(ctx: Context, loader: Option<GlProc>) -> Self {
        Self {
            ctx,
            loader,
            session: None,
            video: None,
            error: None,
            seeking: SeekBar::default(),
            #[cfg(target_os = "macos")]
            playlist_glass: Default::default(),
            #[cfg(target_os = "macos")]
            controller_glass: Default::default(),
            #[cfg(target_os = "macos")]
            native_player_menu: None,
            #[cfg(target_os = "macos")]
            menu_host: Default::default(),
            control_bar_layout: ControlBarLayout::default(),
            progress_bar_position: ProgressBarPosition::default(),
            floating_position: None,
            prefs: PlayerPreferences::default(),
            saved_prefs: PlayerPreferences::default(),
            settings_panel: PlayerSettings::default(),
            sidebars: Default::default(),
            playlist_open: false,
            toggle_playlist: false,
            chapters_tab: false,
            reveal_playlist_item: true,
            controls_until: Instant::now(),
            pointer_inside: false,
            window_state: None,
            caption_position: 100,
            fullscreen: false,
            theater: false,
            music: false,
            mini: false,
            pip_visible: false,
            auto_pip: false,
            os_minimized: false,
            pip_parent: PipParent::Keep,

            request: None,
            end_handled: false,
            last_paused: None,
            playback_flash: None,
            feedback: None,
            feedback_speed: None,
            feedback_seeks: 0,
            battery: Default::default(),
            volume_scroll: 0.0,
            peek: Peek::default(),
            prefetched_video: None,
            new_collection: None,
            checkpoint: Checkpoint::default(),
            #[cfg(target_os = "macos")]
            now_playing: None,
        }
    }

    pub fn restore_preferences(&mut self, prefs: PlayerPreferences) {
        theme::apply_style(&self.ctx, prefs.interface_theme, prefs.glass_strength);
        self.ctx.data_mut(|data| {
            data.insert_temp(Id::new("progress_bar_style"), prefs.progress_bar_style);
            data.insert_temp(
                Id::new("opaque_player_controls"),
                !prefs.glass || !prefs.interface_theme.translucent(),
            );

            data.insert_temp(Id::new("player_chapter_markers"), prefs.show_chapters);
        });
        super::player_keys::refresh_labels(&self.ctx, &prefs);
        self.prefs = prefs.clone();
        self.saved_prefs = prefs;
        self.update_window_level();
    }

    fn update_window_level(&mut self) {
        let floating = self.prefs.always_on_top
            || (self.prefs.float_playing
                && self.active()
                && self.session.as_ref().is_some_and(|s| {
                    let state = s.state();
                    state.ready && !state.paused && !state.ended
                }));
        let show_status = self.prefs.show_float_status;
        if self.window_state == Some((floating, show_status)) {
            return;
        }
        self.window_state = Some((floating, show_status));
        self.ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            ViewportCommand::WindowLevel(if floating {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }),
        );
        let title = match (show_status, floating) {
            (true, true) => "MyTube — On top",
            (true, false) => "MyTube — Normal window",
            _ => "MyTube",
        };
        self.ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            ViewportCommand::Title(title.to_owned()),
        );
    }

    pub(super) fn settings_window(
        &mut self,
        ui: &mut Ui,
        service: &Service,
        appearance: &mut Appearance,
    ) {
        if !self.settings_panel.open || (self.active() && !self.mini()) {
            return;
        }
        ui.set_style(ui.ctx().global_style());
        let theme_before = self.prefs.interface_theme;
        let appearance_before = *appearance;
        let mut layout = self.layout();
        let mut position = self.progress_bar_position;
        self.settings_panel.window_contents(
            ui,
            &mut self.prefs,
            self.session.as_ref().filter(|_| self.video.is_some()),
            &mut layout,
            &mut position,
            appearance,
        );
        self.apply_settings_display(
            service,
            appearance_before,
            *appearance,
            theme_before,
            layout,
            position,
        );
        // Only live settings pages need periodic updates from the playback engine.
        if !matches!(
            self.settings_panel.section,
            Section::Interface | Section::Controls | Section::Inspector
        ) {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
    }

    pub fn preferences_frame(
        &mut self,
        service: &Service,
        appearance: &mut Appearance,
    ) -> Option<(String, bool)> {
        let lost_focus = self.ctx.input(|i| {
            i.events
                .iter()
                .any(|event| matches!(event, egui::Event::WindowFocused(false)))
        });
        #[cfg(target_os = "macos")]
        let lost_focus = self.settings_panel.native.lost_focus(lost_focus);
        if self.prefs.pause_on_inactive
            && lost_focus
            && let Some(session) = &self.session
        {
            session.send(Control::SetPaused(true));
        }
        let previous_theme = self.prefs.interface_theme;
        let mut layout = self.layout();
        let mut position = self.progress_bar_position;
        let previous_appearance = *appearance;
        if self.settings_panel.open
            && self.active()
            && !self.mini
            && self.settings_panel.sidebar.is_none()
        {
            // The gear can be clicked after sidebar layout has run in this frame.
            self.ctx.request_repaint();
        } else {
            self.settings_panel.show(
                &self.ctx,
                &mut self.prefs,
                self.session.as_ref().filter(|_| self.video.is_some()),
                &mut layout,
                &mut position,
                appearance,
            );
        }
        if std::mem::take(&mut self.settings_panel.music_request) {
            self.music = !self.music;
            self.restore();
            self.settings_panel.open = false;
        }
        self.apply_settings_display(
            service,
            previous_appearance,
            *appearance,
            previous_theme,
            layout,
            position,
        );
        if self.saved_prefs != self.prefs {
            if self
                .ctx
                .input(|i| i.raw.viewports.contains_key(&PlayerSettings::window_id()))
            {
                self.ctx.request_repaint_of(PlayerSettings::window_id());
            }
            if self.saved_prefs.bindings != self.prefs.bindings {
                super::player_keys::refresh_labels(&self.ctx, &self.prefs);
            }
            if !self.prefs.detached_mini {
                self.leave_pip();
            }
            theme::apply_style(
                &self.ctx,
                self.prefs.interface_theme,
                self.prefs.glass_strength,
            );
            self.ctx.data_mut(|data| {
                data.insert_temp(Id::new("progress_bar_style"), self.prefs.progress_bar_style);
                data.insert_temp(
                    Id::new("opaque_player_controls"),
                    !self.prefs.glass || !self.prefs.interface_theme.translucent(),
                );

                data.insert_temp(Id::new("player_chapter_markers"), self.prefs.show_chapters);
            });

            if (!self.prefs.hover_previews && self.saved_prefs.hover_previews)
                || self.prefs.remote_thumbnails != self.saved_prefs.remote_thumbnails
            {
                self.cancel_preview(service);
            }
            if let Some(session) = &self.session {
                if self.prefs.equalizer != self.saved_prefs.equalizer {
                    session.send(Control::Equalizer(self.prefs.equalizer));
                }
                for option in OPTIONS
                    .iter()
                    .filter(|option| session.supports_option(option.key))
                {
                    let value = option.value(&self.prefs);
                    if value != option.value(&self.saved_prefs) && option.accepts(value) {
                        session.send(Control::Configure(option.key, value.to_owned()));
                    }
                }
            }
            if service.send(Action::SavePlayerSettings(self.prefs.clone())) {
                self.saved_prefs = self.prefs.clone();
            }
        }
        self.update_window_level();
        self.session.as_ref().and_then(Session::take_tool_notice)
    }

    fn apply_settings_display(
        &mut self,
        service: &Service,
        previous_appearance: Appearance,
        appearance: Appearance,
        previous_theme: InterfaceTheme,
        mut layout: ControlBarLayout,
        position: ProgressBarPosition,
    ) {
        if previous_appearance != appearance {
            theme::apply(&self.ctx, appearance);
            service.send(Action::SaveAppearance(appearance));
            self.ctx.request_repaint();
        }
        if self.prefs.interface_theme != previous_theme {
            layout = self.layout();
            theme::apply_style(
                &self.ctx,
                self.prefs.interface_theme,
                self.prefs.glass_strength,
            );
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
            self.ctx.request_discard("Interface theme changed");
            self.reveal_controls();
        }
        self.save_control_preferences(service, layout, position);
    }

    pub fn active(&self) -> bool {
        self.video.is_some()
    }
    pub fn mini(&self) -> bool {
        self.mini && self.active()
    }
    pub fn fullscreen(&self) -> bool {
        self.fullscreen && self.active()
    }
    fn layout(&self) -> ControlBarLayout {
        if self.prefs.interface_theme.glass_ui() {
            self.control_bar_layout
        } else {
            self.prefs.classic_layout
        }
    }

    pub fn set_control_bar_layout(&mut self, layout: ControlBarLayout) {
        self.control_bar_layout = layout;
        self.reveal_controls();
    }
    pub fn set_progress_bar_position(&mut self, position: ProgressBarPosition) {
        self.progress_bar_position = position;
        self.peek.reset();
        self.reveal_controls();
    }
    fn pip_id() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("mytube_pip")
    }

    fn pip_title(&self) -> String {
        let title = self.video.as_ref().map_or("MyTube", |v| v.title.as_str());
        format!("{title} — Picture-in-picture")
    }

    fn pip_builder(&self) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.pip_title())
            .with_transparent(true)
            .with_inner_size(vec2(360., 220.))
            .with_min_inner_size(vec2(220., 124.))
            .with_always_on_top()
    }

    // A minimized root receives logic ticks but may skip painting. Prepare a hidden
    // child while visible so the OS can show it on the minimize event itself.
    pub fn prepare_pip_window(&self) {
        if self.active()
            && !self.mini()
            && self.prefs.detached_mini
            && self.prefs.pip_on_os_minimize
            && !self.ctx.embed_viewports()
        {
            self.ctx.show_viewport_deferred(
                Self::pip_id(),
                self.pip_builder().with_visible(false),
                |_, _| {},
            );
        }
    }

    pub fn sync_minimized(&mut self) {
        let Some(minimized) = self.ctx.input(|i| i.viewport().minimized) else {
            return;
        };
        if self.os_minimized == minimized {
            return;
        }
        self.os_minimized = minimized;
        if !minimized && self.auto_pip {
            self.restore();
        } else if minimized
            && self.active()
            && !self.mini()
            && self.prefs.detached_mini
            && self.prefs.pip_on_os_minimize
            && !self.ctx.embed_viewports()
            && (!self.prefs.pip_video_only
                || self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.state().video_track.is_some()))
        {
            self.minimize();
            self.auto_pip = true;
            self.ctx
                .send_viewport_cmd_to(Self::pip_id(), ViewportCommand::Visible(true));
            self.ctx.request_repaint_of(Self::pip_id());
        }
    }

    fn leave_pip(&mut self) {
        self.auto_pip = false;
        if std::mem::take(&mut self.pip_visible) {
            if self.pip_parent != PipParent::Keep {
                self.ctx
                    .send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Visible(true));
                self.ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    ViewportCommand::Minimized(false),
                );
            }
            self.ctx
                .send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Focus);
            self.pip_parent = PipParent::Keep;
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub fn restore(&mut self) {
        self.leave_pip();
        self.mini = false;
        self.reveal_controls();
    }

    pub fn minimize(&mut self) {
        self.finish_precise(Finish::Cancel);
        self.seeking.drag = None;
        self.seeking.surface_drag = None;
        if self.fullscreen {
            self.set_fullscreen(false);
        }
        self.playlist_open = false;
        if self.prefs.pause_on_minimize
            && let Some(session) = &self.session
        {
            session.send(Control::SetPaused(true));
        }
        if self.active() && !self.mini {
            self.settings_panel.open = false;
        }
        self.mini = self.active();
        self.ctx.request_repaint();
    }

    pub fn mini_shortcut(&mut self, enabled: bool) {
        if enabled
            && self.active()
            && super::player_keys::count(&self.ctx, &self.prefs, egui::Key::I, false) != 0
        {
            if self.mini() {
                self.restore();
            } else {
                self.minimize();
            }
        }
    }

    pub fn take_new_collection(&mut self) -> Option<i64> {
        self.new_collection.take()
    }

    pub fn video(&self) -> Option<&Video> {
        self.video.as_ref()
    }
    pub fn take_request(&mut self) -> Option<usize> {
        self.request.take()
    }

    pub fn open(&mut self, video: Video, mut start: Option<f64>, service: &Service) {
        #[cfg(target_os = "macos")]
        {
            self.native_player_menu = None;
        }
        let previous = self.save_checkpoint(service, true);
        if self
            .video
            .as_ref()
            .is_some_and(|current| current.thumbnail_key() == video.thumbnail_key())
            && let Some(at) = previous
        {
            start = at;
        }
        if !self.prefs.resume {
            start = None;
        }
        self.checkpoint = Checkpoint {
            saved: start,
            ..Checkpoint::default()
        };
        #[cfg(target_os = "macos")]
        {
            self.now_playing = None;
        }
        self.error = None;
        self.reveal_playlist_item = true;
        self.seeking.reset();
        self.peek.reset();
        self.pointer_inside = false;
        self.reveal_controls();
        self.end_handled = false;
        self.last_paused = None;
        self.playback_flash = None;
        self.feedback = None;
        self.feedback_speed = None;
        self.feedback_seeks = 0;
        self.volume_scroll = 0.0;
        if self.session.is_none() {
            match self
                .loader
                .clone()
                .ok_or_else(|| anyhow::anyhow!("The OpenGL renderer is unavailable"))
                .and_then(|loader| Session::open(&self.ctx, loader))
            {
                Ok(session) => {
                    if self.prefs.equalizer != [0; 10] {
                        session.send(Control::Equalizer(self.prefs.equalizer));
                    }
                    for option in OPTIONS
                        .iter()
                        .filter(|option| session.supports_option(option.key))
                    {
                        if let Some(value) = self.prefs.engine.get(option.key)
                            && option.accepts(value)
                        {
                            session.send(Control::Configure(option.key, value.clone()));
                        }
                    }
                    self.session = Some(session);
                }
                Err(error) => self.error = Some(format!("{error:#}")),
            }
        }
        if let Some(session) = &self.session {
            session.load(video.clone(), start, self.prefs.pause_on_open);
        }
        if self.prefs.record_history && self.session.is_some() {
            service.send(Action::RecordPlay(video.id));
        }
        self.video = Some(video);
    }

    pub fn close(&mut self, service: &Service) {
        self.leave_pip();
        self.save_checkpoint(service, true);
        #[cfg(target_os = "macos")]
        {
            self.now_playing = None;
            self.native_player_menu = None;
        }
        if self.video.take().is_some() {
            self.settings_panel.open = false;
            self.music = false;
            self.set_caption_position(100);
            if let Some(session) = &self.session {
                session.stop();
            }
            self.seeking.reset();
            self.peek.reset();
            self.mini = false;
            self.playlist_open = false;
            self.request = None;
            if self.fullscreen {
                self.set_fullscreen(false);
            }
        }
    }

    pub fn save_checkpoint(&mut self, service: &Service, flush: bool) -> Option<Option<f64>> {
        let video = self.video.as_ref()?;
        let state = self.session.as_ref()?.state();
        if let Some(position) = self.checkpoint.update(&state, Instant::now(), flush) {
            service.save_position(video.thumbnail_key(), position);
        }
        if state.ended {
            Some(None)
        } else {
            state.confirmed_position.map(Some)
        }
    }

    #[cfg(target_os = "macos")]
    pub fn update_now_playing(&mut self, queue: &Queue) -> Option<usize> {
        use super::macos::{NowPlaying, RemoteAction};

        let video = self.video.as_ref()?;
        let session = self.session.as_ref()?;
        let state = session.state();
        let Some(at) = state
            .confirmed_position
            .filter(|_| state.ready && state.error.is_none())
        else {
            self.now_playing = None;
            return None;
        };
        let now_playing = self
            .now_playing
            .get_or_insert_with(|| NowPlaying::new(&self.ctx));
        let end = playback_end(video, &state);
        let rate = if state.paused || state.ended {
            0.0
        } else {
            state.speed
        };
        now_playing.publish(&video.title, at.max(0.0), end, rate, state.ended, queue);
        let mut navigation = None;
        for action in now_playing.actions() {
            match action {
                RemoteAction::Play => session.send(Control::SetPaused(false)),
                RemoteAction::Pause => session.send(Control::SetPaused(true)),
                RemoteAction::Toggle => session.send(Control::Toggle),
                RemoteAction::Seek(at) if end > 0.0 => {
                    self.seeking.seek(session, at, end);
                }
                RemoteAction::Previous => navigation = queue.previous.or(navigation),
                RemoteAction::Next => navigation = queue.next.or(navigation),
                RemoteAction::Seek(_) => {}
            }
        }
        navigation
    }

    pub fn shutdown(&mut self, gl: Option<&glow::Context>, service: &Service) {
        self.save_checkpoint(service, true);
        if self.session.is_some() {
            service.flush_positions();
        }
        #[cfg(target_os = "macos")]
        {
            self.now_playing = None;
        }
        if let Some(session) = &self.session {
            session.shutdown(gl);
        }
    }

    pub fn load_preview(&mut self, key: TimelineKey, image: Option<egui::ColorImage>, exact: bool) {
        if self
            .video
            .as_ref()
            .is_some_and(|v| v.thumbnail_key() == (key.0, key.1))
        {
            self.peek.load(&self.ctx, key, image, exact);
        }
    }

    pub fn load_prefetched(&mut self, key: TimelineKey, image: egui::ColorImage) {
        if self.prefetched_video == Some((key.0, key.1)) {
            self.peek.receive_prefetched(&self.ctx, key, image);
        }
    }

    pub fn cancel_preview(&mut self, service: &Service) {
        self.peek.reset();
        if self.prefetched_video.take().is_some() {
            self.peek.clear_prefetched();
            service.prefetch_timeline(TimelineOrigin::Player, None);
        }
    }

    pub fn load_precise(
        &mut self,
        sequence: u64,
        key: TimelineKey,
        image: Option<egui::ColorImage>,
    ) {
        if self.seeking.precise.accept(sequence, key) {
            self.peek.load(&self.ctx, key, image, true);
        }
    }

    pub fn timeline_update(&mut self) -> Option<Option<TimelineRequest>> {
        let hover = self.peek.take_request();
        self.seeking
            .precise
            .request(self.video.as_ref(), |key| self.peek.cached(key))
            .or_else(|| {
                if self.seeking.precise.gesture.is_some() {
                    None
                } else {
                    hover
                }
            })
    }

    fn finish_precise(&mut self, finish: Finish) {
        if let Some(command) = self.seeking.precise.finish(finish) {
            self.seeking.mark_precise_pending(&command);
            if let Some(session) = &self.session {
                session.send(command);
            }
            self.reveal_controls();
        }
    }

    fn set_fullscreen(&mut self, fullscreen: bool) {
        if self.fullscreen != fullscreen
            && let Some(session) = &self.session
        {
            if fullscreen && self.prefs.play_entering_fullscreen {
                session.send(Control::SetPaused(false));
            }
            if !fullscreen && self.prefs.pause_leaving_fullscreen {
                session.send(Control::SetPaused(true));
            }
        }
        self.fullscreen = fullscreen;
        self.mini = false;
        self.reveal_controls();
        self.ctx
            .send_viewport_cmd(ViewportCommand::Fullscreen(fullscreen));
    }

    fn reveal_controls(&mut self) {
        self.controls_until =
            Instant::now() + Duration::from_millis(self.prefs.hide_delay_ms.into());
        self.ctx.request_repaint();
    }

    fn set_caption_position(&mut self, position: u8) {
        let position = position.min(self.prefs.caption_position);
        if self.caption_position != position
            && let Some(session) = &self.session
        {
            session.send(Control::SubtitlePosition(position));
            self.caption_position = position;
        }
    }

    fn prepare_timeline(&mut self, video: &Video, state: &Playback, service: &Service) {
        let key = video.thumbnail_key();
        let duration = if state.ready && state.duration > 0. {
            state.duration
        } else {
            video.media.duration.unwrap_or_default()
        };
        if self.prefs.hover_previews
            && duration > 0.0
            && state.error.is_none()
            && self.prefetched_video != Some(key)
        {
            self.prefetched_video = Some(key);
            self.peek.clear_prefetched();
            service.prefetch_timeline(
                TimelineOrigin::Player,
                Some(TimelinePrefetch {
                    remote: self.prefs.remote_thumbnails,
                    focus: (state.position / duration).clamp(0., 1.),
                    video: video.clone(),
                    duration,
                }),
            );
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        browser: &mut library::Browser,
        shortcuts: bool,
        thumbs: &mut Thumbnails,
        service: &Service,
        settings: &Settings,
    ) {
        let Some(video) = self.video.clone() else {
            return;
        };
        let queue = Queue::for_video(browser, Some(&video));
        self.peek.set_cover(&video, thumbs.find(&video));
        let collections = &settings.collections;
        let mut state = self
            .session
            .as_ref()
            .map(Session::state)
            .unwrap_or_default();
        state.position = self.seeking.position(&state, ui.ctx());
        self.update_playback_flash(&state);
        self.update_navigation(ui, &queue, &mut state, shortcuts);
        self.prepare_timeline(&video, &state, service);
        self.update_held_seek(ui);
        let theater = self.theater_visible();
        let show_playlist = !self.fullscreen
            && self.settings_panel.sidebar.is_none()
            && !self.playlist_open
            && !self.music;
        let top_margin = if self.prefs.compact_interface { 6 } else { 16 };
        egui::CentralPanel::default()
            .frame(if self.fullscreen {
                egui::Frame::new().fill(Color32::BLACK)
            } else if self.music {
                let outer = if self.prefs.edge_to_edge || theater {
                    0
                } else {
                    24
                };
                theme::layer(Color32::TRANSPARENT, outer, top_margin)
            } else {
                egui::Frame::NONE
            })
            .show(ui, |ui| {
                if self.music {
                    self.music_player(ui, &video, &state, &queue);
                } else if self.fullscreen {
                    let size = player_size(ui.available_size(), state.display_aspect(&video));
                    self.player_surface(ui, &state, &queue, service, collections, size);
                } else {
                    let window = ui.ctx().content_rect().size();
                    let aspect = state.display_aspect(&video);
                    let mut scroll_source = egui::scroll_area::ScrollSource::default();
                    if self.seeking.drag.is_some()
                        || self.seeking.surface_position().is_some()
                        || self.seeking.precise.gesture.is_some()
                    {
                        scroll_source.drag = egui::scroll_area::DragScroll::Never;
                    }
                    theme::page_scroll(ui)
                        .scroll_source(scroll_source)
                        .id_salt(("watch_details", video.id))
                        .show(ui, |ui| {
                            let page = ui.available_rect_before_wrap();
                            let layout = WatchPageLayout::new(page, window, aspect, &self.prefs);
                            let mut primary = layout.player;
                            if theater {
                                primary = page;
                                if let Some(panel) = self.sidebars.playlist {
                                    primary.max.x = panel.left() - WATCH_MARGIN;
                                }
                                primary.max.y =
                                    primary.top() + theater_height(window, page.width(), aspect);
                            }
                            if show_playlist && !theater && !layout.stacked {
                                ui.scope_builder(
                                    egui::UiBuilder::new().max_rect(layout.sidebar),
                                    |ui| {
                                        self.page_playlist(
                                            ui,
                                            browser,
                                            thumbs,
                                            video.id,
                                            layout.player.height(),
                                        );
                                    },
                                );
                            }
                            ui.scope_builder(egui::UiBuilder::new().max_rect(primary), |ui| {
                                self.player_surface(
                                    ui,
                                    &state,
                                    &queue,
                                    service,
                                    collections,
                                    primary.size(),
                                );
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::symmetric(
                                        if theater || layout.stacked { 23 } else { 0 },
                                        0,
                                    ))
                                    .show(ui, |ui| {
                                        show_details(
                                            ui,
                                            &video,
                                            service,
                                            settings.watch_later.contains(&video.id),
                                        );
                                    });
                            });
                            if show_playlist && (theater || layout.stacked) {
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::symmetric(23, 0))
                                    .show(ui, |ui| {
                                        self.page_playlist(
                                            ui,
                                            browser,
                                            thumbs,
                                            video.id,
                                            layout.player.height(),
                                        );
                                    });
                            }
                        });
                }
            });
        self.peek.finish_frame();
    }

    fn update_held_seek(&mut self, ui: &Ui) {
        let Some(gesture) = &mut self.seeking.precise.gesture else {
            return;
        };
        let Some(mut held) = gesture.held else {
            return;
        };
        let pointer = ui.input(ScrubPointer::read);
        if pointer.cancelled || (!pointer.down && pointer.release.is_none()) {
            self.finish_precise(Finish::Cancel);
            return;
        }
        if let Some(point) = pointer.release.or(pointer.point) {
            let before = gesture.position();
            gesture.select(held.update(point, gesture.duration));
            if gesture.position() != before
                && let Some(session) = &self.session
            {
                session.preview_precise(gesture.id, gesture.position());
            }
        }
        if pointer.release.is_some() {
            gesture.held = None;
            if held.commits_on_release() {
                self.finish_precise(Finish::Scrub);
            }
        } else {
            gesture.held = Some(held);
        }
    }

    fn seek_surface(&mut self, ui: &Ui, player: Rect, bar: Bar, state: &Playback) -> bool {
        let end = self
            .video
            .as_ref()
            .map_or(0.0, |video| playback_end(video, state));
        let pointer = ui.input(ScrubPointer::read);
        let Some(session) = self.session.as_ref().filter(|_| {
            state.ready && !state.precise_seeking && state.error.is_none() && end > 0.0
        }) else {
            self.seeking.surface_drag = None;
            return false;
        };
        if let Some(press) = pointer.press
            && player.contains(press)
            && !bar.rect.contains(press)
            && !bar
                .detached_timeline
                .is_some_and(|strip| strip.contains(press))
            && ui.ctx().layer_id_at(press) == Some(ui.layer_id())
            && !egui::Popup::is_any_open(ui.ctx())
        {
            let width = player.width();
            let zero = press.x - width * (state.position / end).clamp(0.0, 1.0) as f32;
            self.seeking.surface_drag = Some(SurfaceDrag {
                held: HeldScrub::new(press, zero, width),
                touch_scroll: !self.fullscreen && ui.input(|input| input.any_touches()),
                position: None,
            });
        }
        if let Some(command) = self.seeking.surface_motion(&pointer, state, end) {
            self.peek.reset();
            ui.memory_mut(|memory| memory.surrender_focus(Id::new(super::SEARCH_ID)));
            session.send(command);
            ui.ctx().request_repaint();
            return true;
        }
        self.seeking.surface_position().is_some()
    }

    fn music_player(&mut self, ui: &mut Ui, video: &Video, state: &Playback, queue: &Queue) {
        ui.horizontal(|ui| {
            // Keep the existing decoder and audio clock when switching presentation.
            let size = vec2(112., 112. / state.display_aspect(video).clamp(0.6, 3.));
            let (picture, _) = ui.allocate_exact_size(size, Sense::hover());
            self.paint_surface(
                ui,
                picture,
                self.video_mask(picture),
                state,
                [None, None],
                None,
            );
            ui.vertical(|ui| {
                ui.heading(&video.title);
                if let Some(audio) = &video.media.audio {
                    ui.label(audio);
                }
                if ui.button("Return to video").clicked() {
                    self.music = false;
                }
            });
        });
        ui.separator();
        if let Some(session) = &self.session {
            let mut at = state.position;
            if theme::slider(
                ui,
                egui::Slider::new(&mut at, 0.0..=state.duration.max(0.001))
                    .show_value(false)
                    .text("Playback position"),
            )
            .changed()
            {
                self.seeking.seek(session, at, state.duration);
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}",
                    clock(at),
                    end_time_label(state.duration, at, &self.prefs, state.speed)
                ))
                .font(theme::timestamp_font(13.)),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(queue.previous.is_some(), egui::Button::new("Previous"))
                    .clicked()
                {
                    self.request = queue.previous;
                }
                if ui
                    .button(if state.paused || state.ended {
                        "Play"
                    } else {
                        "Pause"
                    })
                    .clicked()
                {
                    session.send(Control::Toggle);
                }
                if ui
                    .add_enabled(queue.next.is_some(), egui::Button::new("Next"))
                    .clicked()
                {
                    self.request = queue.next;
                }
                volume_slider(
                    ui,
                    Some(session),
                    state.volume,
                    110.,
                    theme::slider_fill(ui),
                );
                if ui.button("Playlist").clicked() {
                    self.toggle_playlist = true;
                }
            });
        }
    }

    fn player_surface(
        &mut self,
        ui: &mut Ui,
        state: &Playback,
        queue: &Queue,
        service: &Service,
        collections: &[Collection],
        size: egui::Vec2,
    ) {
        let slot_height = if self.fullscreen {
            ui.available_height().max(size.y)
        } else {
            size.y.max(control_bar::MIN_PLAYER_HEIGHT)
        };
        let (slot, _) = ui.allocate_exact_size(
            vec2(ui.available_width().max(1.0), slot_height),
            Sense::hover(),
        );
        let sized = Rect::from_center_size(slot.center(), vec2(size.x.min(slot.width()), size.y));
        let player = if self.fullscreen {
            slot
        } else {
            Rect::from_center_size(slot.center(), vec2(sized.width(), slot.height()))
        };
        let aspect = self
            .video
            .as_ref()
            .map_or(16.0 / 9.0, |video| state.display_aspect(video));
        let picture = video_picture(sized, aspect);
        let mask = self.video_mask(player);
        let visible_picture = picture.intersect(ui.clip_rect());
        let rect = if self.fullscreen { sized } else { picture };
        let mut bar = Bar {
            player: player.intersect(ui.clip_rect()),
            ..Bar::new(
                player,
                self.layout(),
                self.progress_bar_position,
                self.fullscreen,
            )
        };
        if self.prefs.compact_interface {
            bar.compact_spacing();
        }
        let glass_ui = self.prefs.interface_theme.glass_ui();
        let mut items =
            ToolbarItem::controller_items(&self.prefs.toolbar, glass_ui, self.fullscreen);
        items.retain(|item| bar.shows(*item));
        bar.set_toolbar(&items);
        if bar.layout == ControlBarLayout::Floating
            && let Some(position) = self.floating_position
        {
            let mut shown = position;
            if self.prefs.snap_controller
                && ((position.x - 0.5) * bar.floating_travel().width()).abs() <= 10.0
            {
                shown.x = 0.5;
            }
            bar.place_floating(shown);
        }
        let surface_scrubbing = self.seeking.precise.gesture.is_none()
            && self.seek_surface(ui, visible_picture, bar, state);
        if self.seeking.precise.gesture.is_some() {
            // Use the picture rather than the window or the letterbox. If the
            // details view clips the picture, keep the controls within the playback pane.
            let clip = ui.clip_rect();
            let panel_bottom = picture
                .bottom()
                .clamp((clip.top() + 42.0).min(clip.bottom()), clip.bottom());
            let panel = PrecisePanel::new(
                visible_picture
                    .with_min_y(
                        visible_picture
                            .top()
                            .min(panel_bottom - 42.0)
                            .max(clip.top()),
                    )
                    .with_max_y(panel_bottom),
                aspect,
            );
            let glass_actions = panel.actions().map(|rect| {
                (glass_ui && DisplayMaterial::current(ui.ctx()) == DisplayMaterial::Glass)
                    .then_some(Glass {
                        rect,
                        radius: rect.height() * 0.5,
                        opacity: 1.,
                        frosted: false,
                        symbol: None,
                    })
            });
            self.paint_surface(ui, rect, mask, state, glass_actions, None);
            let above = rect.with_max_y(panel.rect.top().max(rect.top()));
            let play_surface = ui.interact(
                above,
                ui.make_persistent_id("precise_play_surface"),
                Sense::click(),
            );
            self.set_caption_position(
                (((panel.rect.top() - rect.top()) / rect.height()).clamp(0.0, 1.0) * 100.0).floor()
                    as u8,
            );
            if let (Some(gesture), Some(video)) = (&mut self.seeking.precise.gesture, &self.video) {
                let before = gesture.position();
                let held = gesture.held.is_some();
                let finish = egui::Area::new(Id::new("precise_seek_overlay"))
                    .fixed_pos(panel.rect.min)
                    .default_size(panel.rect.size())
                    .constrain(false)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |overlay| {
                        overlay.set_min_size(panel.rect.size());
                        #[cfg(target_os = "macos")]
                        self.controller_glass.begin(
                            overlay,
                            0,
                            &mut glass_actions.into_iter().flatten().collect(),
                        );
                        let action = panel.show(overlay, gesture, video, state, &self.peek);
                        #[cfg(target_os = "macos")]
                        self.controller_glass.finish(
                            overlay,
                            self.menu_host
                                .view()
                                .filter(|_| !overlay.ctx().embed_viewports()),
                            mask,
                        );
                        action
                    })
                    .inner
                    .or_else(|| (!held && play_surface.clicked()).then_some(Finish::Play));
                if let Some(finish) = finish {
                    self.finish_precise(finish);
                } else if gesture.position() != before
                    && let Some(session) = &self.session
                {
                    session.preview_precise(gesture.id, gesture.position());
                }
            }
            return;
        }
        let controls = bar.rect.intersect(player);
        let strip = bar.detached_timeline.map(|rect| rect.intersect(player));
        let panels = || std::iter::once(controls).chain(strip);
        let (focused, pointer_moved, control_pressed) = ui.input(|input| {
            let pointer = &input.pointer;
            let control_pressed = pointer.any_down()
                && pointer
                    .press_origin()
                    .is_some_and(|origin| panels().any(|panel| panel.contains(origin)));
            (
                input.focused,
                pointer.delta() != egui::Vec2::ZERO,
                control_pressed,
            )
        });
        let now = Instant::now();
        let pointer_inside = focused && ui.rect_contains_pointer(player);
        if self.pointer_inside && !pointer_inside {
            self.controls_until = self.controls_until.min(now + CONTROLS_EXIT);
        }
        let pointer_engaged = pointer_inside
            && (!self.pointer_inside
                || pointer_moved
                || panels().any(|panel| ui.rect_contains_pointer(panel)));
        self.pointer_inside = pointer_inside;
        let control_focused = ui
            .memory(|memory| memory.focused())
            .and_then(|id| ui.ctx().read_response(id))
            .is_some_and(|response| {
                response.has_focus() && panels().any(|panel| panel.contains_rect(response.rect))
            });
        let engaged = ui
            .ctx()
            .is_being_dragged(ui.make_persistent_id("watch_bar"))
            || self.seeking.drag.is_some()
            || surface_scrubbing
            || control_pressed
            || control_focused
            || pointer_engaged
            || egui::Popup::is_any_open(ui.ctx());
        if engaged {
            self.controls_until = now + Duration::from_millis(self.prefs.hide_delay_ms.into());
        }
        let opacity =
            if !self.prefs.auto_hide || engaged || state.paused || state.ended || !state.ready {
                1.0
            } else if now < self.controls_until {
                ui.ctx().request_repaint_after(self.controls_until - now);
                1.0
            } else if self.prefs.reduce_motion || theme::solid_glass(ui.ctx()) {
                0.0
            } else {
                let progress = ((now - self.controls_until).as_secs_f32()
                    / CONTROLS_FADE.as_secs_f32())
                .min(1.0);
                if progress < 1.0 {
                    ui.ctx().request_repaint_after(Duration::from_millis(16));
                }
                1.0 - progress * progress * (3.0 - 2.0 * progress)
            };
        let show_controls = opacity > 0.0 && ui.is_rect_visible(controls);
        let visible_strip = strip.filter(|rect| opacity > 0.0 && ui.is_rect_visible(*rect));
        if self.prefs.wheel_volume || self.prefs.wheel_seek {
            let volume = self.fullscreen || (show_controls && ui.rect_contains_pointer(controls));
            self.wheel_input(ui, player, state, volume);
        }
        let material = DisplayMaterial::current(ui.ctx());
        let top_overlay = [
            (show_controls && bar.layout == ControlBarLayout::Top).then_some(controls),
            visible_strip.filter(|_| self.progress_bar_position == ProgressBarPosition::Top),
        ]
        .into_iter()
        .flatten()
        .reduce(|first, second| first.union(second));
        self.paint_surface(
            ui,
            rect,
            mask,
            state,
            [
                if show_controls {
                    bar.glass(material, opacity)
                } else {
                    None
                },
                visible_strip.and_then(|_| bar.timeline_glass(material, opacity)),
            ],
            top_overlay,
        );
        let surface = ui.interact(
            player,
            ui.make_persistent_id("watch_surface"),
            Sense::click(),
        );
        if show_controls {
            let floating = bar.layout == ControlBarLayout::Floating;
            let response = ui.interact(
                controls,
                ui.make_persistent_id("watch_bar"),
                if floating {
                    Sense::click_and_drag()
                } else {
                    Sense::click()
                },
            );
            if floating
                && response.dragged()
                && self.seeking.drag.is_none()
                && !ui.input(|i| {
                    i.pointer.press_origin().is_some_and(|p| {
                        bar.detached_timeline.is_none() && bar.timeline().contains(p)
                    })
                })
            {
                let travel = bar.floating_travel();
                let extent = travel.size().max(vec2(1.0, 1.0));
                let origin = self
                    .floating_position
                    .unwrap_or((bar.rect.min - travel.min) / extent);
                let offset = origin + response.drag_delta() / extent;
                let fraction = offset.clamp(egui::Vec2::ZERO, vec2(1.0, 1.0));
                self.floating_position = Some(fraction);
                ui.ctx().request_repaint();
            }
        }
        if let Some(strip) = visible_strip {
            ui.interact(
                strip,
                ui.make_persistent_id("watch_seek_strip"),
                Sense::CLICK,
            );
        }
        if state.ready
            && !surface_scrubbing
            && !theme::control_click(&surface)
            && !surface.secondary_clicked()
            && !egui::Popup::is_any_open(ui.ctx())
        {
            use crate::player::options::ClickAction;
            let action = if surface.double_clicked() {
                self.prefs.double_click
            } else if surface.middle_clicked() {
                self.prefs.middle_click
            } else if surface.clicked() {
                self.prefs.click
            } else {
                ClickAction::None
            };
            if let Some(session) = &self.session {
                match action {
                    ClickAction::Toggle => session.send(Control::Toggle),
                    ClickAction::Fullscreen => self.set_fullscreen(!self.fullscreen),
                    ClickAction::Mute => session.send(Control::Mute),
                    ClickAction::Mini => self.minimize(),
                    ClickAction::Loop => session.tool(crate::player::PlayerTool::LoopPoint),
                    ClickAction::None => {}
                }
            }
        }
        let position = self.progress_bar_position;

        let caption_position = if state.captions && state.subtitle.is_some() {
            let bottom_strip = visible_strip.filter(|_| position == ProgressBarPosition::Bottom);
            let bottom = (show_controls && bar.layout != ControlBarLayout::Top)
                .then_some(controls)
                .into_iter()
                .chain(bottom_strip)
                .filter(|panel| panel.intersects(rect))
                .fold(rect.bottom(), |y, panel| y.min(panel.top()));
            (((bottom - rect.top()) / rect.height()).clamp(0.0, 1.0) * 100.0).floor() as u8
        } else {
            100
        };
        self.set_caption_position(caption_position);
        if show_controls {
            let mut overlay = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("control_bar")
                    .max_rect(bar.rect),
            );
            overlay.set_opacity(opacity);
            overlay.set_clip_rect(overlay.clip_rect().intersect(player));
            bar.paint(&overlay, mask);
            overlay.set_clip_rect(overlay.clip_rect().intersect(controls));
            *overlay.visuals_mut() = overlay.ctx().style_of(egui::Theme::Dark).visuals.clone();
            if bar.layout == ControlBarLayout::Classic {
                self.classic_controls(
                    &mut overlay,
                    state,
                    queue,
                    service,
                    collections,
                    visible_picture,
                );
            } else {
                self.controls(&mut overlay, bar, state, queue, visible_picture);
            }
            if glass_ui || bar.layout != ControlBarLayout::Classic {
                self.controller_actions(&mut overlay, bar, state, &items);
            }
        }
        if let Some(strip) = visible_strip {
            let mut overlay = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("progress_strip")
                    .max_rect(strip),
            );
            overlay.set_opacity(opacity);
            overlay.set_clip_rect(overlay.clip_rect().intersect(strip));
            self.timeline_strip(
                &mut overlay,
                state,
                strip,
                position == ProgressBarPosition::Top,
                visible_picture,
            );
        }
        #[cfg(target_os = "macos")]
        self.controller_glass
            .finish(ui, self.menu_host.view(), mask);
        self.control_preferences_menu(&surface, service);
    }

    pub(super) fn context_menu<A: Copy>(
        &self,
        response: &egui::Response,
        width: f32,
        items: impl FnOnce() -> Vec<super::context_menu::Item<A>>,
    ) -> Option<A> {
        #[cfg(target_os = "macos")]
        if theme::menu_material(&response.ctx) == DisplayMaterial::Glass
            && !egui::Popup::is_id_open(&response.ctx, egui::Popup::context_menu(response).get_id())
            && let Some(view) = self.menu_host.view()
        {
            return if (response.secondary_clicked() || theme::control_click(response))
                && self.menu_host.begin_tracking(&response.ctx)
            {
                super::macos::context_menu::choose(view, response, &items())
            } else {
                None
            };
        }
        super::context_menu::fallback(response, width, items)
    }

    fn control_preferences_menu(&mut self, surface: &egui::Response, service: &Service) {
        use super::context_menu::{self, PlayerAction};
        let layout = self.layout();
        let position = self.progress_bar_position;
        let inline = || {
            context_menu::player_popup(surface, layout, position, || {
                self.session.as_ref().map(Session::speed_menu_control)
            })
            .and_then(|popup| popup.inner)
        };
        #[cfg(not(target_os = "macos"))]
        let selected = inline();
        #[cfg(target_os = "macos")]
        let selected = if let Some(menu) = &self.native_player_menu {
            let std::task::Poll::Ready(action) = menu.result() else {
                return;
            };
            self.native_player_menu = None;
            action
        } else if self.menu_host.view().is_some()
            && !surface.ctx.embed_viewports()
            && theme::menu_material(&surface.ctx) == DisplayMaterial::Glass
        {
            if (surface.secondary_clicked() || theme::control_click(surface))
                && self.menu_host.begin_tracking(&surface.ctx)
            {
                let host = if surface.ctx.viewport_id() == Self::pip_id() {
                    self.menu_host.window_content(&self.pip_title())
                } else {
                    self.menu_host.view().map(objc2::rc::Retained::from)
                };
                if let Some(view) = host {
                    self.native_player_menu = Some(super::macos::context_menu::PlayerMenu::open(
                        &view,
                        surface,
                        &context_menu::player(
                            layout,
                            position,
                            self.session.as_ref().map(Session::speed_menu_control),
                        ),
                    ));
                }
            }
            None
        } else {
            inline()
        };
        match selected {
            Some(PlayerAction::Settings) => self.toggle_settings(),
            Some(PlayerAction::Layout(next)) => {
                self.save_control_preferences(service, next, position)
            }
            Some(PlayerAction::Position(next)) => {
                self.save_control_preferences(service, layout, next)
            }
            None => {}
        }
    }

    pub fn show_mini(
        &mut self,
        root: &Ui,
        queue: &Queue,
        thumbs: &mut Thumbnails,
        service: &Service,
        shortcuts: bool,
    ) {
        let ctx = root.ctx();

        let Some(video) = self.video.clone() else {
            return;
        };
        self.peek.set_cover(&video, thumbs.find(&video));
        let mut state = self
            .session
            .as_ref()
            .map(Session::state)
            .unwrap_or_default();
        state.position = self.seeking.position(&state, ctx);
        self.update_playback_flash(&state);
        self.prepare_timeline(&video, &state, service);
        if state.ended
            && !state.precise_seeking
            && !self.end_handled
            && (self.prefs.autoplay || self.prefs.loop_playlist)
        {
            self.request = queue
                .next
                .or(queue.first.filter(|_| self.prefs.loop_playlist));
        }
        self.end_handled = state.ended;
        let width = (ctx.content_rect().width() - 40.0).clamp(240.0, 340.0);
        if self.prefs.detached_mini && !ctx.embed_viewports() {
            let id = Self::pip_id();
            let builder = self.pip_builder().with_visible(true);
            ctx.show_viewport_immediate(id, builder, |ui, _| {
                if ui.input(|input| input.viewport().close_requested()) {
                    self.restore();
                } else {
                    self.mini_content(ui, ui.available_size(), &mut state, shortcuts, service);
                }
            });
            if self.mini() && !self.pip_visible {
                self.pip_visible = true;
                self.pip_parent = if self.auto_pip {
                    PipParent::Minimize
                } else {
                    self.prefs.pip_parent
                };
                match self.pip_parent {
                    PipParent::Keep => {}
                    PipParent::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                    PipParent::Hide => ctx.send_viewport_cmd(ViewportCommand::Visible(false)),
                }
            }
        } else {
            egui::Area::new(Id::new("mini_player"))
                .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -56.0))
                .order(egui::Order::Foreground)
                .default_width(width)
                .show(ctx, |ui| {
                    ui.set_style(root.style().clone());
                    self.mini_content(
                        ui,
                        vec2(width, width * 9.0 / 16.0),
                        &mut state,
                        shortcuts,
                        service,
                    );
                });
        }
        self.peek.finish_frame();
    }

    fn mini_content(
        &mut self,
        ui: &mut Ui,
        size: egui::Vec2,
        state: &mut Playback,
        shortcuts: bool,
        service: &Service,
    ) {
        let can_toggle = self.session.is_some()
            && state.ready
            && state.error.is_none()
            && !state.precise_seeking;
        let layout = self.layout();
        let position = self.progress_bar_position;
        let (rect, surface) = ui.allocate_exact_size(
            size,
            if can_toggle {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if shortcuts && ui.rect_contains_pointer(rect) {
            self.keyboard_seek(ui.ctx(), state);
        }
        if self.prefs.wheel_volume || self.prefs.wheel_seek {
            self.wheel_input(ui, rect, state, true);
        }
        let strip = control_bar::mini_timeline(rect, layout, position);
        let top = control_bar::mini_timeline_at_top(layout, position);
        let material = DisplayMaterial::current(ui.ctx());
        let show_controls = mini_controls_visible(
            ui,
            rect,
            self.prefs.auto_hide,
            self.prefs.hide_delay_ms,
            self.seeking.drag.is_some(),
        );
        let actions = mini_actions(rect);
        let over_picture = ui.rect_contains_pointer(rect)
            && !ui.rect_contains_pointer(strip)
            && !actions
                .iter()
                .any(|&action| ui.rect_contains_pointer(action));
        let show_indicator =
            can_toggle && show_controls && over_picture && self.seeking.drag.is_none();
        let glass_indicator = show_indicator
            && self.prefs.interface_theme.translucent()
            && self.prefs.glass_osd
            && DisplayMaterial::system(ui.ctx()) == DisplayMaterial::Glass;
        let media_glass = self.prefs.interface_theme.glass_ui();
        let diameter = if media_glass { 64. } else { 54. };
        let center = rect.center();
        let paused = state.paused || state.ended;
        let indicator = Rect::from_center_size(center, egui::Vec2::splat(diameter));
        let material_button = |bounds: Rect, glyph| Glass {
            rect: bounds,
            radius: bounds.height() / 2.,
            frosted: false,
            opacity: 1.,
            symbol: Some(glyph),
        };
        let mask = self.video_mask(rect);
        self.paint_surface(
            ui,
            rect,
            mask,
            state,
            [
                show_controls
                    .then(|| control_bar::timeline_glass(layout, material, strip, 1.))
                    .flatten(),
                glass_indicator.then(|| {
                    material_button(indicator, if paused { c"play.fill" } else { c"pause.fill" })
                }),
            ],
            (top && show_controls).then_some(strip),
        );
        let native_symbols = glass_indicator && self.native_media_symbols(ui);
        if can_toggle {
            let paused = state.paused || state.ended;
            let label = if paused { "Play video" } else { "Pause video" };
            surface
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            if over_picture && surface.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if show_indicator {
                let fill = if glass_indicator {
                    theme::glass_tint(ui.ctx(), Color32::from_gray(29))
                } else if !media_glass {
                    Color32::from_black_alpha(88)
                } else {
                    Color32::from_gray(29)
                };
                let outline = if surface.has_focus() {
                    Stroke::new(2.0, Color32::WHITE)
                } else {
                    Stroke::NONE
                };
                ui.painter()
                    .circle(rect.center(), diameter / 2., fill, outline);
                let icon = Icon::playback(ui, paused, self.reduce_motion());
                if !native_symbols {
                    icon.paint(ui.painter(), rect.center(), diameter * 0.63, Color32::WHITE);
                }
            }
        }
        self.set_caption_position(
            if show_controls && !top && state.captions && state.subtitle.is_some() {
                (((strip.top() - rect.top()) / rect.height()) * 100.0).floor() as u8
            } else {
                100
            },
        );
        let [open, close] = if show_controls {
            ui.interact(
                strip,
                ui.make_persistent_id("mini_seek_strip"),
                Sense::CLICK,
            );
            let mut overlay = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("mini_progress")
                    .max_rect(strip),
            );
            overlay.set_clip_rect(overlay.clip_rect().intersect(strip));
            self.timeline_strip(&mut overlay, state, strip, top, rect);
            let [open, close] = actions;
            [
                mini_corner_action(ui, open, "Open in player", Icon::ExpandMini),
                mini_corner_action(ui, close, "Close mini player", Icon::Close),
            ]
        } else {
            [false, false]
        };
        if close {
            self.close(service);
            self.cancel_preview(service);
        } else if open {
            self.restore();
        } else if can_toggle
            && surface.clicked()
            && !theme::control_click(&surface)
            && !surface.secondary_clicked()
            && !egui::Popup::is_any_open(ui.ctx())
            && let Some(session) = &self.session
        {
            session.send(Control::Toggle);
        }
        #[cfg(target_os = "macos")]
        {
            let pip = (ui.ctx().viewport_id() == Self::pip_id())
                .then(|| self.menu_host.window_content(&self.pip_title()))
                .flatten();
            let host = if ui.ctx().viewport_id() == Self::pip_id() {
                pip.as_deref()
            } else {
                self.menu_host.view()
            };
            self.controller_glass.finish(ui, host, mask);
        }
        self.control_preferences_menu(&surface, service);
    }

    fn wheel_input(&mut self, ui: &Ui, player: Rect, state: &Playback, volume: bool) {
        if !state.ready
            || state.error.is_some()
            || !ui.rect_contains_pointer(player)
            || egui::Popup::is_any_open(ui.ctx())
            || self.session.is_none()
        {
            self.volume_scroll = 0.0;
            return;
        }
        let delta = ui.input_mut(|input| {
            if input.modifiers.any() {
                return egui::Vec2::ZERO;
            }
            let mut movement = egui::Vec2::ZERO;
            for event in &input.events {
                if let egui::Event::MouseWheel { unit, delta, .. } = event {
                    movement += *delta
                        * match unit {
                            egui::MouseWheelUnit::Point => 0.1,
                            egui::MouseWheelUnit::Line => 5.0,
                            egui::MouseWheelUnit::Page => 15.0,
                        };
                }
            }
            if self.prefs.wheel_seek && movement.x.abs() > movement.y.abs() {
                input.smooth_scroll_delta.x = 0.;
                vec2(movement.x, 0.)
            } else if volume && self.prefs.wheel_volume {
                input.smooth_scroll_delta.y = 0.;
                vec2(0., movement.y)
            } else {
                egui::Vec2::ZERO
            }
        });
        if delta.x != 0.
            && let Some(session) = &self.session
        {
            let at = self.seeking.position(state, ui.ctx());
            self.seeking
                .seek(session, at - f64::from(delta.x), state.duration);
        }
        self.volume_scroll += delta.y;
        let steps = self.volume_scroll.trunc() as i32;
        if steps != 0
            && let Some(session) = &self.session
        {
            self.volume_scroll -= steps as f32;
            session.send(Control::AdjustVolume(steps));
            let feedback = Duration::from_millis(self.prefs.feedback_duration_ms.into());
            self.feedback = Some((Feedback::Volume, Instant::now() + feedback));
            ui.ctx().request_repaint_after(feedback);
        }
    }

    pub fn locked_aspect(&self) -> Option<f32> {
        if !self.prefs.lock_aspect || self.fullscreen || self.mini || self.music {
            return None;
        }
        self.video.as_ref().map(|v| {
            self.session
                .as_ref()
                .map(Session::state)
                .unwrap_or_default()
                .display_aspect(v)
        })
    }

    pub fn toggle_settings(&mut self) {
        self.settings_panel.open = !self.settings_panel.open;
        self.ctx.request_repaint();
    }

    pub(super) fn reduce_motion(&self) -> bool {
        #[cfg(target_os = "macos")]
        if objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
            return true;
        }
        self.prefs.reduce_motion
    }

    fn toggle_theater(&mut self) {
        self.theater = !self.theater_visible();
        if !self.fullscreen && self.theater {
            self.settings_panel.open = false;
        }
    }

    fn theater_visible(&self) -> bool {
        self.theater && (self.fullscreen || self.settings_panel.sidebar.is_none())
    }

    fn video_mask(&self, frame: Rect) -> VideoMask {
        let rounded = self.mini || (!self.fullscreen && !self.theater_visible());
        VideoMask {
            frame,
            radius: if rounded { 12 } else { 0 },
        }
    }

    pub fn prepare_sidebars(&mut self, ui: &Ui) -> Rect {
        let bounds = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let settings_left = self.settings_panel.leading(&self.prefs);
        if std::mem::take(&mut self.toggle_playlist) {
            self.playlist_open = self.sidebars.playlist.is_none();
            self.reveal_playlist_item = self.playlist_open;
            if self.playlist_open
                && self.settings_panel.open
                && (!self.fullscreen
                    || super::player_layout::Sidebars::fullscreen(
                        bounds,
                        &self.prefs,
                        Some(settings_left),
                        true,
                    )
                    .playlist
                    .is_none())
            {
                self.settings_panel.open = false;
            }
        }
        self.settings_panel.reveal = if !self.fullscreen {
            f32::from(self.settings_panel.open)
        } else {
            ui.ctx().animate_bool_with_time_and_easing(
                Id::new("playback_settings_slide"),
                self.settings_panel.open,
                if self.reduce_motion() { 0. } else { 0.26 },
                egui::emath::easing::cubic_in_out,
            )
        };
        let sliding = self.settings_panel.open || self.settings_panel.reveal > 0.;
        if !self.fullscreen {
            let page = bounds.with_max_x(bounds.right() - theme::PAGE_SCROLL_GUTTER);
            let aspect = self.video.as_ref().map_or(16. / 9., |video| {
                self.session
                    .as_ref()
                    .map(Session::state)
                    .unwrap_or_default()
                    .display_aspect(video)
            });
            let layout =
                WatchPageLayout::new(page, ui.ctx().content_rect().size(), aspect, &self.prefs);
            self.sidebars = super::player_layout::Sidebars {
                settings: sliding.then_some(layout.sidebar),
                playlist: (!sliding
                    && self.playlist_open
                    && self.seeking.precise.gesture.is_none())
                .then_some(layout.sidebar),
            };
            self.settings_panel.sidebar = self.sidebars.settings;
            return if sliding || self.sidebars.playlist.is_some() {
                bounds.with_max_x(layout.sidebar.left() - WATCH_MARGIN)
            } else {
                bounds
            };
        }
        self.sidebars = super::player_layout::Sidebars::fullscreen(
            bounds,
            &self.prefs,
            sliding.then_some(settings_left),
            self.playlist_open && self.seeking.precise.gesture.is_none(),
        );
        self.settings_panel.sidebar = self.sidebars.settings;
        if self.prefs.edge_to_edge {
            bounds
        } else {
            self.sidebars.video_area(bounds)
        }
    }

    pub fn show_playlist(
        &mut self,
        ui: &Ui,
        browser: &mut library::Browser,
        thumbs: &mut Thumbnails,
    ) {
        #[cfg(target_os = "macos")]
        if self.sidebars.playlist.is_none()
            || !self.playlist_open
            || self.seeking.precise.gesture.is_some()
        {
            self.playlist_glass.hide();
        }
        if !self.playlist_open {
            return;
        }
        if self.seeking.precise.gesture.is_some() {
            return;
        }
        let Some(current_id) = self.video.as_ref().map(|v| v.id) else {
            return;
        };
        let Some(bounds) = self.sidebars.playlist else {
            return;
        };
        let open = super::player_layout::sidebar(
            ui.ctx(),
            Id::new("playlist_sidebar"),
            (bounds, bounds),
            "Playlist and chapters",
            super::player_layout::SidebarStyle {
                glass: self.prefs.interface_theme.translucent() && self.prefs.glass_sidebar,
                compact: self.prefs.compact_interface,
                outlined: false,
                radius: if !self.prefs.interface_theme.glass_ui() && !self.fullscreen {
                    12
                } else {
                    0
                },
            },
            |ui| self.playlist_contents(ui, browser, thumbs, current_id),
        );
        #[cfg(target_os = "macos")]
        if open
            && self.prefs.glass_sidebar
            && theme::menu_material(ui.ctx()) == DisplayMaterial::Glass
            && !egui::Popup::is_any_open(ui.ctx())
        {
            self.playlist_glass.sidebar(
                ui.ctx(),
                egui::LayerId::new(egui::Order::Middle, Id::new("playlist_sidebar")),
                bounds,
                self.menu_host.view(),
            );
        } else {
            self.playlist_glass.hide();
        }
        self.playlist_open &= open;
    }

    fn page_playlist(
        &mut self,
        ui: &mut Ui,
        browser: &mut library::Browser,
        thumbs: &mut Thumbnails,
        current_id: i64,
        height: f32,
    ) {
        ui.scope(|ui| {
            ui.set_width(ui.available_width());
            ui.set_max_height(height);
            theme::style_form(ui);
            self.playlist_contents(ui, browser, thumbs, current_id);
        });
    }

    fn playlist_contents(
        &mut self,
        ui: &mut Ui,
        browser: &mut library::Browser,
        thumbs: &mut Thumbnails,
        current_id: i64,
    ) {
        ui.horizontal_wrapped(|ui| {
            for (chapters, label) in [(false, "Playlist"), (true, "Chapters")] {
                let selected = self.chapters_tab == chapters;
                let tab = if self.prefs.interface_theme.glass_ui() {
                    button(ui, label, None, if selected { Selected } else { Secondary })
                } else {
                    super::classic_settings::pill(ui, label, selected, false)
                };
                tab.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        selected,
                        label,
                    )
                });
                if tab.clicked() {
                    self.chapters_tab = chapters;
                }
            }
        });
        ui.add_space(4.);
        let list = egui::ScrollArea::vertical()
            .id_salt(self.chapters_tab)
            .max_height(ui.available_height().max(1.));
        if self.chapters_tab {
            let state = self
                .session
                .as_ref()
                .map(Session::state)
                .unwrap_or_default();
            if state.chapters.is_empty() {
                ui.label(text(
                    "No chapters in this video",
                    13.,
                    theme::palette(ui).muted,
                ));
            }
            let current = state
                .chapters
                .partition_point(|c| c.at <= state.position)
                .checked_sub(1);
            let height = if self.prefs.compact_interface {
                32.
            } else {
                40.
            };
            list.show_rows(ui, height, state.chapters.len(), |ui, visible| {
                for index in visible {
                    let chapter = &state.chapters[index];
                    if chapter_row(ui, chapter, height, current == Some(index)).clicked()
                        && let Some(session) = &self.session
                    {
                        self.seeking.seek(session, chapter.at, state.duration);
                        self.playlist_open = false;
                    }
                }
            });
        } else {
            // The thumbnail already has four points of space above and below it.
            ui.spacing_mut().item_spacing.y = 0.;
            let total = browser.total().unwrap_or_default();
            let row_height = queue_row_height(ui.available_width(), self.prefs.compact_interface);
            let list = if self.reveal_playlist_item
                && let Some(index) = browser.position(current_id)
            {
                self.reveal_playlist_item = false;
                list.vertical_scroll_offset(
                    index as f32 * (row_height + ui.spacing().item_spacing.y),
                )
            } else {
                list
            };
            list.show_rows(ui, row_height, total, |ui, visible| {
                browser.set_target(visible.clone());
                for index in visible {
                    if let Some(item) = browser.get(index) {
                        if queue_row(ui, thumbs, item, row_height, item.id == current_id).clicked()
                        {
                            self.request = Some(index);
                            self.playlist_open = false;
                        }
                    } else {
                        ui.allocate_space(vec2(1.0, row_height));
                    }
                }
            });
        }
    }

    fn native_media_symbols(&self, ui: &Ui) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.menu_host.view().is_some()
                && !ui.ctx().embed_viewports()
                && !egui::Popup::is_any_open(ui.ctx())
                && theme::glass_strength(ui.ctx()) > 0
                && self.seeking.precise.gesture.is_none()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = ui;
            false
        }
    }

    fn paint_surface<const N: usize>(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        mask: VideoMask,
        state: &Playback,
        glass: [Option<Glass>; N],
        top_overlay: Option<Rect>,
    ) {
        ui.painter()
            .rect_filled(mask.frame, mask.radius, Color32::BLACK);
        let mut materials: Vec<_> = glass.into_iter().flatten().collect();
        let system = DisplayMaterial::system(ui.ctx());
        #[cfg(not(target_os = "macos"))]
        if !self.mini
            && (self.prefs.interface_theme.translucent() && self.prefs.glass_sidebar)
            && system == DisplayMaterial::Glass
        {
            let settings = self
                .settings_panel
                .sidebar_geometry()
                .map(|(_, clip)| clip)
                .filter(|clip| clip.is_positive());
            for (rect, radius) in [
                (settings, super::player_layout::settings_radius(ui.ctx())),
                (self.sidebars.playlist, 0),
            ] {
                let Some(rect) = rect else { continue };
                materials.push(Glass {
                    rect,
                    radius: f32::from(radius),
                    frosted: true,
                    symbol: None,
                    opacity: 1.0,
                });
            }
        }
        let native_osd = self.prefs.interface_theme.translucent()
            && self.prefs.glass_osd
            && system == DisplayMaterial::Glass
            && self.native_media_symbols(ui);
        let mut osd = |rect: Rect, radius: u8, opacity: f32, symbol| {
            let glass = self.prefs.interface_theme.translucent()
                && self.prefs.glass_osd
                && system == DisplayMaterial::Glass;
            if glass {
                materials.push(Glass {
                    rect,
                    radius: radius.into(),
                    frosted: true,
                    symbol,
                    opacity,
                });
            }
            let fill = if glass {
                theme::glass_tint(ui.ctx(), Color32::from_gray(29))
            } else {
                Color32::from_gray(29)
            };
            ui.painter()
                .rect_filled(rect, radius, fill.gamma_multiply(opacity));
        };
        // Reserve the video layer before drawing OSD text. Its shader needs the
        // final badge geometry, which becomes available during text layout.
        let surface = ui.painter().add(egui::Shape::Noop);
        if let Some(error) = self.error.as_ref().or(state.error.as_ref()) {
            message(ui, rect, error);
        } else if self.session.is_some() {
            if !state.ready {
                message(ui, rect, "Opening video…");
            } else if self.prefs.playback_feedback
                && self.prefs.feedback_on_pause
                && !self.mini
                && let Some((paused, at)) = self.playback_flash
            {
                let age = at.elapsed().as_secs_f32();
                let duration = self.prefs.feedback_duration_ms as f32 / 1000.0;
                let classic = self.prefs.interface_theme == InterfaceTheme::Classic;
                let duration = if classic { duration.min(0.5) } else { duration };
                if age < duration {
                    let progress = age / duration;
                    let moving = classic && !self.reduce_motion();
                    let scale = if moving { 0.8 + 0.45 * progress } else { 1. };
                    let opacity = if self.prefs.reduce_motion || theme::solid_glass(ui.ctx()) {
                        1.0
                    } else {
                        ((1.0 - progress) * 1.5).clamp(0.0, 1.0)
                    };
                    let center = rect.center();
                    osd(
                        Rect::from_center_size(center, vec2(64., 64.) * scale),
                        (32. * scale).ceil() as u8,
                        opacity,
                        Some(if paused { c"pause.fill" } else { c"play.fill" }),
                    );
                    let icon = if classic {
                        Icon::ClassicTransport(if paused { 32 } else { 0 })
                    } else if paused {
                        Icon::Pause
                    } else {
                        Icon::Play
                    };
                    if !native_osd {
                        icon.paint(
                            ui.painter(),
                            center,
                            29.0 * scale,
                            Color32::WHITE.gamma_multiply(opacity),
                        );
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(16));
                }
            }
        }
        if self.fullscreen && self.prefs.fullscreen_clock {
            let mut label = chrono::Local::now().format("%H:%M").to_string();
            if let Some(percent) = self.battery.update(ui.ctx()) {
                label.push_str(&format!("   {percent}%"));
            }
            let top = top_overlay.map_or(rect.top(), |overlay| overlay.bottom().max(rect.top()));
            let painter = ui.painter().with_clip_rect(rect);
            let galley = painter.layout_no_wrap(label, theme::timestamp_font(16.0), Color32::WHITE);
            let badge = Rect::from_min_size(
                pos2(rect.left() + 12.0, top + 12.0),
                galley.size() + vec2(16.0, 8.0),
            );
            osd(badge, 5, 1.0, None);
            painter.galley(badge.center() - galley.size() * 0.5, galley, Color32::WHITE);
            ui.ctx().request_repaint_after(Duration::from_secs(30));
        }
        if self.prefs.playback_feedback
            && let Some((kind, until)) = &self.feedback
            && Instant::now() < *until
        {
            let label = match kind {
                Feedback::Volume if state.muted => "Muted".into(),
                Feedback::Volume => format!("{:.0}%", state.volume),
                Feedback::Speed => format!("{}×", state.speed),
                Feedback::Seek(position) => clock(*position),
                Feedback::File => self
                    .video
                    .as_ref()
                    .map(|v| v.title.clone())
                    .unwrap_or_default(),
            };
            ui.ctx()
                .request_repaint_after(until.saturating_duration_since(Instant::now()));
            let painter = ui.painter().with_clip_rect(rect);
            let galley = painter.layout_no_wrap(
                label,
                if matches!(kind, Feedback::File | Feedback::Volume) {
                    theme::font(self.prefs.feedback_size.into())
                } else {
                    theme::timestamp_font(self.prefs.feedback_size.into())
                },
                Color32::WHITE,
            );
            let size = galley.size() + vec2(20.0, 10.0);
            let top = top_overlay.map_or(rect.top(), |overlay| overlay.bottom().max(rect.top()));
            let badge = Rect::from_min_size(
                pos2(
                    rect.right() - size.x - 12.0,
                    (top + 12.0).min(rect.bottom() - size.y),
                ),
                size,
            );
            osd(badge, 6, 1.0, None);
            painter.galley(badge.center() - galley.size() * 0.5, galley, Color32::WHITE);
        }
        if self.error.is_none()
            && state.error.is_none()
            && let Some(session) = &self.session
        {
            #[cfg(not(target_os = "macos"))]
            for glass in &mut materials {
                glass.opacity *= theme::glass_amount(ui.ctx());
            }
            materials.retain(|glass| glass.opacity > 0. && theme::glass_amount(ui.ctx()) > 0.);
            #[cfg(target_os = "macos")]
            {
                if self.seeking.precise.gesture.is_some() {
                    // The filmstrip's foreground layer composites its own native controls.
                    materials.clear();
                } else if self.menu_host.view().is_some()
                    && !ui.ctx().embed_viewports()
                    && !egui::Popup::is_any_open(ui.ctx())
                {
                    self.controller_glass
                        .begin(ui, surface.0 + 1, &mut materials);
                } else {
                    self.controller_glass.hide();
                    materials.clear();
                }
            }
            ui.painter()
                .set(surface, session.video_surface(rect, materials, mask));
        }
    }

    fn update_playback_flash(&mut self, state: &Playback) {
        if state.precise_seeking || self.seeking.precise.gesture.is_some() {
            self.playback_flash = None;
            self.last_paused = Some(state.paused);
            return;
        }
        if state.ready {
            let previous_speed = self.feedback_speed.replace(state.speed);
            let seek_changed = self.feedback_seeks != state.seeks;
            self.feedback_seeks = state.seeks;
            let next = if previous_speed.is_none() && self.prefs.feedback_on_open {
                Some(Feedback::File)
            } else if previous_speed.is_some_and(|speed| (state.speed - speed).abs() > 0.001)
                && self.prefs.feedback_on_speed
            {
                Some(Feedback::Speed)
            } else if seek_changed && self.prefs.feedback_on_seek {
                Some(Feedback::Seek(state.position))
            } else {
                None
            };
            if let Some(kind) = next {
                self.feedback = Some((
                    kind,
                    Instant::now() + Duration::from_millis(self.prefs.feedback_duration_ms.into()),
                ));
            }
        }
        if state.ready
            && !state.ended
            && self
                .last_paused
                .replace(state.paused)
                .is_some_and(|last| last != state.paused)
        {
            self.playback_flash = Some((state.paused, Instant::now()));
            self.reveal_controls();
        }
    }

    fn timeline_strip(
        &mut self,
        ui: &mut Ui,
        state: &Playback,
        strip: Rect,
        below: bool,
        player: Rect,
    ) {
        *ui.visuals_mut() = ui.ctx().style_of(egui::Theme::Dark).visuals.clone();
        control_bar::paint_timeline(ui, self.layout(), strip);
        self.seek_track(
            ui,
            state,
            strip.shrink2(vec2(14.0, 0.0)),
            strip,
            below,
            player,
        );
    }

    fn seek_track(
        &mut self,
        ui: &mut Ui,
        state: &Playback,
        track: Rect,
        controller: Rect,
        below: bool,
        player: Rect,
    ) {
        let Some(video) = &self.video else {
            return;
        };
        let session = self.session.as_ref();
        let end = playback_end(video, state);
        let seek_rect = slider_bounds(track, end, self.mini);
        let show_time = seek_rect.width() < track.width();
        let classic =
            self.layout() == ControlBarLayout::Classic || !self.prefs.interface_theme.glass_ui();
        let seek_rect = if classic {
            Rect::from_center_size(seek_rect.center(), vec2(seek_rect.width(), 18.0))
        } else {
            seek_rect
        };
        let mut seek_ui = ui.new_child(egui::UiBuilder::new().id_salt("seek").max_rect(seek_rect));
        let seek = timeline(
            &mut seek_ui,
            session.map(|session| move |command| session.send(command)),
            state,
            end,
            &mut self.seeking,
            (!classic).then_some(seek_rect.height()),
            (!self.mini).then_some(player),
        );
        if show_time {
            control_bar::time(
                ui,
                track.left_center(),
                Align2::LEFT_CENTER,
                &clock(seek.position),
            );
            let rect = track.with_min_x(seek_rect.right());
            let toggle = ui.interact(rect, ui.id().with("time_display"), Sense::click());
            toggle_time_display(ui, toggle, &mut self.prefs.remaining_time);
            let time = end_time_label(end, seek.position, &self.prefs, state.speed);
            control_bar::time(ui, track.right_center(), Align2::RIGHT_CENTER, &time);
        }
        let hovered = seek.hovered.map(|(mut point, time)| {
            point.y = if below {
                controller.bottom()
            } else {
                controller.top()
            };
            (point, time)
        });
        if self.prefs.hover_previews {
            self.peek.show(
                ui,
                video,
                hovered,
                end,
                PeekPlacement {
                    width: self.prefs.thumbnail_width.into(),
                    player,
                    below,
                    hint: !self.mini,
                },
            );
        }
    }

    fn controls(&mut self, ui: &mut Ui, bar: Bar, state: &Playback, queue: &Queue, picture: Rect) {
        if bar.detached_timeline.is_none() {
            self.seek_track(
                ui,
                state,
                bar.timeline(),
                bar.rect,
                bar.layout == ControlBarLayout::Top,
                picture,
            );
        }

        let session = self.session.as_ref();
        let ready_session = session.filter(|_| state.ready && state.error.is_none());
        let ready = ready_session.is_some();
        let row = bar.row();
        let floating = bar.layout == ControlBarLayout::Floating;

        let transport_x = if floating {
            row.center().x - control_bar::TRANSPORT_WIDTH * 0.5
        } else {
            row.left()
        };
        let paused = state.paused || state.ended;
        let play_label = if paused { "Play (K)" } else { "Pause (K)" };
        let play_icon = Icon::playback(ui, paused, self.reduce_motion());
        let mut transport = control_bar::group(ui, "transport", row, transport_x);
        if let Some(index) = transport_arrow(
            &mut transport,
            false,
            queue.previous,
            &self.prefs,
            ready_session,
            state,
            false,
        ) {
            self.request = Some(index);
        }
        if control_bar::button(
            &mut transport,
            play_label,
            Some(play_icon),
            ready,
            None,
            true,
        )
        .clicked()
            && let Some(session) = session
        {
            session.send(Control::Toggle);
        }
        if let Some(index) = transport_arrow(
            &mut transport,
            true,
            queue.next,
            &self.prefs,
            ready_session,
            state,
            false,
        ) {
            self.request = Some(index);
        }

        let mut volume = control_bar::group(ui, "volume", row, bar.volume_x());
        let (volume_label, volume_icon) = if state.muted {
            ("Unmute (M)", Icon::Mute)
        } else {
            ("Mute (M)", Icon::Volume)
        };
        if glass_icon(
            &mut volume,
            volume_label,
            volume_icon,
            ready,
            Some(state.muted),
        )
        .clicked()
            && let Some(session) = session
        {
            session.send(Control::Mute);
        }
        let inline_volume = bar.inline_volume();
        if inline_volume {
            volume_slider(
                &mut volume,
                ready_session,
                state.volume,
                72.0,
                control_bar::ACCENT,
            );
        }
    }

    fn controller_actions(
        &mut self,
        ui: &mut Ui,
        bar: Bar,
        state: &Playback,
        items: &[ToolbarItem],
    ) {
        let row = bar.row();
        let session = self.session.as_ref();
        let ready = session.is_some() && state.ready && state.error.is_none();
        let actions_width = bar.actions_width();
        let mut actions = control_bar::group(ui, "actions", row, row.right() - actions_width);

        let mut fullscreen = false;
        let overflow = items.len() > bar.enabled_actions;
        let inline = if overflow {
            bar.enabled_actions.saturating_sub(1)
        } else {
            items.len()
        };
        let mut chosen = None;
        let autoplay = self.prefs.autoplay;
        let theater = self.theater_visible();
        let descriptor = |item: ToolbarItem| match item {
            ToolbarItem::Autoplay => ("Autoplay", None, true, Some(autoplay)),
            ToolbarItem::Theater => (
                theater_label(theater),
                Some(Icon::Theater),
                true,
                Some(theater),
            ),
            ToolbarItem::Mini => ("Mini player (I)", Some(Icon::Mini), true, None),
            ToolbarItem::Captions => (
                "Captions (C)",
                None,
                ready && state.tracks.iter().any(|t| t.kind == TrackKind::Subtitle),
                Some(state.captions && state.subtitle.is_some()),
            ),
            ToolbarItem::Playlist => (
                "Playlist and chapters",
                Some(Icon::List),
                true,
                Some(self.playlist_open),
            ),
            ToolbarItem::Settings => (
                "Playback settings",
                Some(Icon::Settings),
                true,
                Some(self.settings_panel.open),
            ),
            ToolbarItem::Fullscreen => (
                if self.fullscreen {
                    "Exit fullscreen (F)"
                } else {
                    "Fullscreen (F)"
                },
                Some(Icon::Fullscreen),
                true,
                Some(self.fullscreen),
            ),
            ToolbarItem::Music => (item.title(), Some(Icon::Music), ready, None),
            ToolbarItem::Screenshot => (item.title(), Some(Icon::Camera), ready, None),
            ToolbarItem::Inspector | ToolbarItem::Text => {
                (item.title(), Some(Icon::Info), ready, None)
            }
        };
        for &item in &items[..inline] {
            if item == ToolbarItem::Autoplay {
                autoplay_toggle(
                    &mut actions,
                    &mut self.prefs.autoplay,
                    control_bar::BUTTON_WIDTH,
                );
                continue;
            }
            let (label, icon, enabled, selected) = descriptor(item);
            let response = control_bar::button(&mut actions, label, icon, enabled, selected, false);
            if response.clicked() {
                chosen = Some(item);
            }
        }
        if overflow {
            let more = glass_icon(&mut actions, "More controls", Icon::More, true, None);
            egui::Popup::menu(&more)
                .style(theme::menu_style(&more.ctx))
                .show(|ui| {
                    for &item in &items[inline..] {
                        let (label, _, enabled, selected) = descriptor(item);
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(label).selected(selected == Some(true)),
                            )
                            .clicked()
                        {
                            chosen = Some(item);
                            ui.close();
                        }
                    }
                });
        }
        match chosen {
            Some(ToolbarItem::Autoplay) => self.prefs.autoplay = !self.prefs.autoplay,
            Some(ToolbarItem::Theater) => self.toggle_theater(),
            Some(ToolbarItem::Mini) => self.minimize(),
            Some(ToolbarItem::Music) => self.music = true,
            Some(ToolbarItem::Screenshot) => {
                if let Some(session) = session {
                    session.pick_tool_file(
                        None,
                        self.prefs.screenshot_subtitles,
                        &self.prefs.screenshot_format,
                    );
                }
            }
            Some(ToolbarItem::Captions) => {
                if let Some(session) = session {
                    toggle_captions(session, state);
                }
            }
            Some(ToolbarItem::Playlist) => self.toggle_playlist = true,
            Some(ToolbarItem::Fullscreen) => fullscreen = true,
            Some(ToolbarItem::Settings) => self.toggle_settings(),
            Some(item @ (ToolbarItem::Inspector | ToolbarItem::Text)) => {
                self.settings_panel.open = true;
                self.settings_panel.section = match item {
                    ToolbarItem::Inspector => Section::Inspector,
                    ToolbarItem::Text => Section::Tools,
                    _ => Section::Interface,
                };
                #[cfg(target_os = "macos")]
                if item == ToolbarItem::Text
                    && let Some(session) = session
                {
                    session.tool(crate::player::PlayerTool::ReadText);
                }
            }
            None => {}
        }
        if fullscreen {
            self.set_fullscreen(!self.fullscreen);
        }
    }

    fn classic_controls(
        &mut self,
        ui: &mut Ui,
        state: &Playback,
        queue: &Queue,
        service: &Service,
        collections: &[Collection],
        player: Rect,
    ) {
        let theater = self.theater_visible();
        let Some(video) = &self.video else {
            return;
        };
        let colors = theme::Palette::DARK;
        let mut minimize = false;
        let mut toggle_settings = false;
        let mut toggle_theater = false;
        let mut fullscreen = false;
        let reduce_motion = self.reduce_motion();

        let session = self.session.as_ref();
        let ready = session.is_some() && state.ready && state.error.is_none();
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 2))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.spacing_mut().item_spacing.x = 4.0;
                let end = playback_end(video, state);
                let wide = ui.available_width() >= 675.0;
                let seek =
                    (self.progress_bar_position == ProgressBarPosition::FollowLayout).then(|| {
                        let seek = timeline(
                            ui,
                            session.map(|session| move |command| session.send(command)),
                            state,
                            end,
                            &mut self.seeking,
                            None,
                            Some(player),
                        );
                        if self.prefs.hover_previews {
                            self.peek.show(
                                ui,
                                video,
                                seek.hovered,
                                end,
                                PeekPlacement {
                                    width: self.prefs.thumbnail_width.into(),
                                    player,
                                    below: false,
                                    hint: true,
                                },
                            );
                        }
                        seek
                    });
                ui.horizontal(|ui| {
                    if play_button(ui, state, reduce_motion)
                        && let Some(session) = session
                    {
                        session.send(Control::Toggle);
                    }
                    for (forward, target) in [(false, queue.previous), (true, queue.next)] {
                        if let Some(index) =
                            transport_arrow(ui, forward, target, &self.prefs, session, state, true)
                        {
                            self.request = Some(index);
                        }
                    }
                    let volume_open = classic_volume(ui, session, state, self.prefs.reduce_motion);
                    if let Some(seek) = seek.filter(|_| wide || !volume_open) {
                        let time = format!(
                            "{} / {}",
                            clock(seek.position),
                            end_time_label(end, seek.position, &self.prefs, state.speed)
                        );
                        let galley = ui.painter().layout_no_wrap(
                            time.clone(),
                            theme::timestamp_font(12.0),
                            Color32::PLACEHOLDER,
                        );
                        let (rect, response) =
                            ui.allocate_exact_size(galley.size(), Sense::click());
                        toggle_time_display(ui, response, &mut self.prefs.remaining_time);
                        ui.painter().galley(rect.min, galley, colors.text);
                    }
                    if self.prefs.interface_theme.glass_ui() {
                        if wide && collection_button(ui, video, service, collections, OnVideo) {
                            self.new_collection = Some(video.id);
                        }
                        return;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        fullscreen = icon_button(
                            ui,
                            if self.fullscreen {
                                "Exit fullscreen"
                            } else {
                                "Fullscreen (F)"
                            },
                            Icon::Fullscreen,
                            if self.fullscreen {
                                OnVideoSelected
                            } else {
                                OnVideo
                            },
                        )
                        .clicked();
                        if self.fullscreen
                            && icon_button(
                                ui,
                                "Playlist and chapters",
                                Icon::List,
                                if self.playlist_open {
                                    OnVideoSelected
                                } else {
                                    OnVideo
                                },
                            )
                            .clicked()
                        {
                            self.toggle_playlist = true;
                        }
                        if !self.fullscreen && ui.available_width() > 120.0 {
                            if icon_button(
                                ui,
                                if theater {
                                    "Exit theater mode"
                                } else {
                                    "Theater mode (T)"
                                },
                                Icon::Theater,
                                if theater { OnVideoSelected } else { OnVideo },
                            )
                            .clicked()
                            {
                                toggle_theater = true;
                            }
                            if icon_button(ui, "Mini player (I)", Icon::Mini, OnVideo).clicked() {
                                minimize = true;
                            }
                        }
                        let subtitle_available = state
                            .tracks
                            .iter()
                            .any(|track| track.kind == TrackKind::Subtitle);
                        if ui
                            .add_enabled_ui(subtitle_available && ready, |ui| {
                                button(
                                    ui,
                                    "CC",
                                    None,
                                    if state.captions && state.subtitle.is_some() {
                                        OnVideoSelected
                                    } else {
                                        OnVideo
                                    },
                                )
                                .on_hover_text("Captions (C)")
                            })
                            .inner
                            .clicked()
                            && let Some(session) = session
                        {
                            toggle_captions(session, state);
                        }
                        let settings = icon_button(
                            ui,
                            "Playback settings",
                            Icon::Settings,
                            if self.settings_panel.open {
                                OnVideoSelected
                            } else {
                                OnVideo
                            },
                        );
                        if settings.clicked() {
                            toggle_settings = true;
                        }
                        if collection_button(ui, video, service, collections, OnVideo) {
                            self.new_collection = Some(video.id);
                        }
                        autoplay_toggle(ui, &mut self.prefs.autoplay, 42.);
                    });
                });
            });

        if toggle_theater {
            self.toggle_theater();
        }
        if toggle_settings {
            self.toggle_settings();
        }
        if minimize {
            self.minimize();
        }
        if fullscreen {
            self.set_fullscreen(!self.fullscreen);
        }
    }

    fn save_control_preferences(
        &mut self,
        service: &Service,
        layout: ControlBarLayout,
        position: ProgressBarPosition,
    ) {
        if layout != self.layout() {
            if !self.prefs.interface_theme.glass_ui() {
                self.prefs.classic_layout = layout;
                self.reveal_controls();
            } else if service.send(Action::SaveControlBarLayout(layout)) {
                self.set_control_bar_layout(layout);
            }
        }
        if position != self.progress_bar_position
            && service.send(Action::SaveProgressBarPosition(position))
        {
            self.set_progress_bar_position(position);
        }
    }

    fn keyboard_seek(&mut self, ctx: &Context, state: &mut Playback) -> bool {
        use egui::Key;
        let count = |key| super::player_keys::count(ctx, &self.prefs, key, true);
        let consume = |key| count(key) > 0;
        let arrow = count(Key::ArrowRight) as f64 - count(Key::ArrowLeft) as f64;
        let jump = if arrow != 0.0 {
            arrow * f64::from(self.prefs.arrow_seek_seconds)
        } else if consume(Key::J) {
            -10.0
        } else if consume(Key::L) {
            10.0
        } else {
            0.0
        };
        if jump == 0.0 {
            return false;
        }
        if state.ready
            && state.error.is_none()
            && state.duration > 0.0
            && let Some(session) = &self.session
        {
            state.position = self
                .seeking
                .seek(session, state.position + jump, state.duration);
            if arrow != 0.0 {
                self.feedback = Some((
                    Feedback::Seek(state.position),
                    Instant::now() + Duration::from_millis(self.prefs.feedback_duration_ms.into()),
                ));
                ctx.request_repaint();
            } else {
                self.reveal_controls();
            }
        }
        true
    }

    fn update_navigation(&mut self, ui: &Ui, queue: &Queue, state: &mut Playback, shortcuts: bool) {
        if state.ended
            && !self.end_handled
            && (self.prefs.autoplay || self.prefs.loop_playlist)
            && !state.precise_seeking
            && self.seeking.precise.gesture.is_none()
        {
            self.request = queue
                .next
                .or(queue.first.filter(|_| self.prefs.loop_playlist));
        }
        self.end_handled = state.ended;
        if !shortcuts {
            return;
        }
        use egui::{Key, Modifiers};
        let consume = |key| {
            if matches!(key, Key::Escape | Key::Enter) {
                ui.input_mut(|input| input.consume_key(Modifiers::NONE, key))
            } else {
                super::player_keys::count(ui.ctx(), &self.prefs, key, true) > 0
            }
        };
        if self.seeking.precise.gesture.is_some() {
            if consume(Key::Escape) {
                self.finish_precise(Finish::Cancel);
            } else if consume(Key::Enter) {
                self.finish_precise(Finish::Play);
            } else if consume(Key::ArrowUp) {
                self.request = queue.previous;
            } else if consume(Key::ArrowDown) {
                self.request = queue.next;
            }
            return;
        }
        if self.playlist_open && consume(Key::Escape) {
            self.playlist_open = false;
            return;
        }
        if self.fullscreen && consume(Key::Escape) {
            self.set_fullscreen(false);
            return;
        }
        if self.layout() == ControlBarLayout::Classic
            && let Some(id) = ui
                .ctx()
                .data(|data| data.get_temp::<Id>(Id::new(CLASSIC_VOLUME_ID)))
            && ui.memory(|memory| memory.has_focus(id))
            && ui.input(|input| {
                [
                    Key::ArrowUp,
                    Key::ArrowDown,
                    Key::ArrowLeft,
                    Key::ArrowRight,
                ]
                .into_iter()
                .any(|key| input.key_pressed(key))
            })
        {
            return;
        }
        if self.keyboard_seek(ui.ctx(), state) {
            return;
        }
        let consume = |key| super::player_keys::count(ui.ctx(), &self.prefs, key, true) > 0;
        if consume(Key::ArrowUp) {
            self.request = queue.previous;
        } else if consume(Key::ArrowDown) {
            self.request = queue.next;
        } else if consume(Key::F) {
            self.set_fullscreen(!self.fullscreen);
        } else if !self.fullscreen && consume(Key::T) {
            self.toggle_theater();
        } else if let Some(session) = &self.session {
            if consume(Key::Space) || consume(Key::K) {
                session.send(Control::Toggle);
            } else if consume(Key::M) {
                session.send(Control::Mute);
            } else if consume(Key::C) {
                toggle_captions(session, state);
            } else if consume(Key::Comma) {
                session.send(Control::Frame(false));
            } else if consume(Key::Period) {
                session.send(Control::Frame(true));
            }
        }
    }
}

fn mini_controls_visible(
    ui: &Ui,
    video: Rect,
    auto_hide: bool,
    delay_ms: u32,
    dragging: bool,
) -> bool {
    let focused = ui
        .memory(|memory| memory.focused())
        .and_then(|id| ui.ctx().read_response(id))
        .is_some_and(|response| {
            response.has_focus()
                && response.layer_id == ui.layer_id()
                && video.contains_rect(response.rect)
        });
    if dragging || focused {
        return true;
    }
    if ui.ctx().pointer_hover_pos().is_none() || !ui.rect_contains_pointer(video) {
        return false;
    }
    let (idle, down) = ui.input(|input| {
        let pointer = &input.pointer;
        (
            pointer
                .time_since_last_movement()
                .min(pointer.time_since_last_click()),
            pointer.any_down(),
        )
    });
    if !auto_hide || down {
        return true;
    }
    let remaining = delay_ms as f32 / 1000. - idle;
    if remaining > 0. {
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f32(remaining));
        return true;
    }
    false
}

fn mini_actions(video: Rect) -> [Rect; 2] {
    const SIZE: f32 = 38.0;
    let y = video.top() + 7.0;
    let left = video.left() + 8.;
    let right = video.right() - 8.0 - SIZE;
    [left, right].map(|x| Rect::from_min_size(pos2(x, y), vec2(SIZE, SIZE)))
}

fn mini_corner_action(ui: &Ui, rect: Rect, label: &str, icon: Icon) -> bool {
    let response = ui.interact(rect, ui.make_persistent_id(label), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let glass = theme::interface_theme(ui.ctx()).glass_ui();
    let active = response.hovered() || response.has_focus();
    let pressed = response.is_pointer_button_down_on();
    let size = if glass { 17. } else { 24. } * if pressed { 0.93 } else { 1. };
    let color = Color32::WHITE;
    if glass || active {
        ui.painter().circle(
            rect.center(),
            16.,
            Color32::from_black_alpha(if active { 152 } else { 92 }),
            if glass {
                Stroke::new(1., Color32::from_white_alpha(70))
            } else {
                Stroke::NONE
            },
        );
    }
    #[cfg(target_os = "macos")]
    let symbol = glass
        .then(|| super::macos::media_buttons::mini_glyph(ui.ctx(), icon))
        .flatten();
    let icon = if glass {
        icon
    } else {
        Icon::ClassicTransport(if matches!(icon, Icon::ExpandMini) {
            33
        } else {
            34
        })
    };
    let paint = |center, size, tint| {
        #[cfg(target_os = "macos")]
        if let Some(image) = &symbol {
            ui.painter().image(
                image.id(),
                Rect::from_center_size(center, egui::Vec2::splat(size)),
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                tint,
            );
            return;
        }
        icon.paint(ui.painter(), center, size, tint);
    };
    paint(
        rect.center() + vec2(0., 1.),
        size + 1.,
        Color32::from_black_alpha(185),
    );
    paint(rect.center(), size, color);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(3.),
            5,
            Stroke::new(1.5, Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
        .clicked()
}

fn video_action(ui: &Ui, rect: Rect, label: &str, icon: Icon) -> bool {
    let response = ui.interact(rect, ui.make_persistent_id(label), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if theme::interface_theme(ui.ctx()).glass_ui() {
        let (fill, edge) = DisplayMaterial::current(ui.ctx()).surface(ui.ctx());
        ui.painter().rect(
            rect,
            (rect.height() * 0.5) as u8,
            fill,
            edge,
            egui::StrokeKind::Inside,
        );
    } else if !response.hovered() && !response.has_focus() {
        icon.paint(
            ui.painter(),
            rect.center() + vec2(0., 1.),
            19.,
            Color32::from_black_alpha(180),
        );
    }
    control_bar::paint_action(
        ui,
        &response,
        Some(icon),
        19.0_f32.min(rect.height() * 0.8),
        false,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
        .clicked()
}

struct WatchPageLayout {
    player: Rect,
    sidebar: Rect,
    stacked: bool,
}

impl WatchPageLayout {
    fn new(page: Rect, window: egui::Vec2, aspect: f32, prefs: &PlayerPreferences) -> Self {
        let stacked = page.width() < 1000.;
        let inset = if prefs.compact_interface { 6. } else { 12. };
        let sidebar_width = (prefs.sidebar_width as f32).min(page.width() * 0.42);
        if stacked {
            let player = page.with_max_y(page.top() + theater_height(window, page.width(), aspect));
            let right = page.right() - WATCH_MARGIN;
            return Self {
                player,
                sidebar: page
                    .shrink2(vec2(0., inset))
                    .with_min_x(right - sidebar_width)
                    .with_max_x(right),
                stacked,
            };
        }

        // In YouTube's standard layout the recommendation rail takes at least
        // 320 CSS pixels. The masthead, top padding and metadata reserve 116 vertically.
        let rail = ((page.width() - theme::PAGE_SCROLL_GUTTER) * 0.275).max(320.);
        let row_width = page.width() - 3. * WATCH_MARGIN;
        let stage_aspect = aspect.max(4. / 3.);
        let width = (row_width - rail)
            .max(640.)
            .min((window.y - 116.).max(360.) * stage_aspect)
            .min(1840.)
            .min(row_width - sidebar_width)
            .max(1.);
        let player = Rect::from_min_size(
            pos2(page.left() + WATCH_MARGIN, page.top() + inset),
            vec2(width, width / stage_aspect),
        );
        let sidebar = page
            .shrink2(vec2(WATCH_MARGIN, inset))
            .with_min_x(player.right() + WATCH_MARGIN);
        Self {
            player,
            sidebar,
            stacked,
        }
    }
}

fn theater_height(window: egui::Vec2, content_width: f32, aspect: f32) -> f32 {
    // YouTube sizes against the viewport (including the scrollbar), then applies
    // its height cap and the single- or two-column minimum, in that order.
    let minimum = if content_width < 1000. { 240. } else { 480. };
    (window.x / aspect).min(window.y - 169.).max(minimum)
}

fn video_picture(bounds: Rect, aspect: f32) -> Rect {
    let width = bounds.width().min(bounds.height() * aspect);
    Rect::from_center_size(bounds.center(), vec2(width, width / aspect))
}

fn player_size(available: egui::Vec2, aspect: f32) -> egui::Vec2 {
    let width = available.x.max(1.0);
    let height = available.y.max(1.0).min(width / aspect);
    vec2(width.min((height * aspect).max(MIN_CONTROLS_WIDTH)), height)
}

fn queue_row_height(width: f32, compact: bool) -> f32 {
    let max_thumb_width: f32 = if compact { 128. } else { 168. };
    let thumb_width = (width * 0.45).min(max_thumb_width);
    (thumb_width * 9. / 16. + 8.).max(68.)
}

fn sidebar_list_row(
    ui: &mut Ui,
    label: &str,
    height: f32,
    selected: bool,
    filled: bool,
) -> egui::Response {
    let colors = theme::palette(ui);
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    if filled && (selected || response.hovered() || response.has_focus()) {
        ui.painter().rect_filled(
            rect,
            6,
            if selected {
                colors.active
            } else {
                colors.hover
            },
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            6,
            Stroke::new(1., theme::focus_color(ui)),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn queue_row(
    ui: &mut Ui,
    thumbs: &mut Thumbnails,
    video: &Video,
    height: f32,
    selected: bool,
) -> egui::Response {
    let colors = theme::palette(ui);
    let response = sidebar_list_row(ui, &video.title, height, selected, false);
    let rect = response.rect;
    let art_width = ((height - 8.) * 16. / 9.).min(rect.width() * 0.45);
    let size = vec2(art_width, art_width * 9. / 16.);
    let image = Rect::from_center_size(rect.left_center() + vec2(art_width * 0.5, 0.), size);
    thumbs.preview_artwork(ui, &response, image, video);
    let x = image.right() + 10.;
    let width = (rect.right() - x - 5.).max(1.);
    let mut title = egui::text::LayoutJob::simple(
        theme::visual(&video.title).into_owned(),
        theme::font(15.),
        colors.text,
        width,
    );
    title.wrap.max_rows = 2;
    let title = ui.fonts_mut(|fonts| fonts.layout_job(title));
    let origin = pos2(x, rect.top() + 6.);
    let metadata_y = origin.y + title.size().y + 4.;
    ui.painter().galley(origin, title, colors.text);
    let mut metadata_x = x;
    if selected {
        Icon::Play.paint(
            ui.painter(),
            pos2(x + 6., metadata_y + 7.),
            12.,
            colors.text,
        );
        metadata_x += 16.;
    }
    if let Some(parent) = video.path.parent() {
        theme::cut(
            ui.painter(),
            pos2(metadata_x, metadata_y),
            rect.right() - metadata_x,
            &library::folder_label(parent),
            egui::FontId::proportional(12.),
            colors.muted,
        );
    }
    if super::preview::over_artwork(&response, image) {
        response
    } else {
        response.on_hover_text(&video.title)
    }
}

fn playback_end(video: &Video, state: &Playback) -> f64 {
    if state.duration > 0.0 {
        state.duration
    } else {
        video.media.duration.unwrap_or_default()
    }
}

fn glass_icon(
    ui: &mut Ui,
    label: &str,
    icon: Icon,
    enabled: bool,
    selected: Option<bool>,
) -> egui::Response {
    control_bar::button(ui, label, Some(icon), enabled, selected, false)
}

fn theater_label(theater: bool) -> &'static str {
    if theater {
        "Exit theater mode (T)"
    } else {
        "Theater mode (T)"
    }
}

fn classic_volume(
    ui: &mut Ui,
    session: Option<&Session>,
    state: &Playback,
    reduce_motion: bool,
) -> bool {
    let (label, icon) = if state.muted {
        ("Unmute (M)", Icon::Mute)
    } else {
        ("Mute (M)", Icon::Volume)
    };
    let speaker = ui
        .add_enabled_ui(session.is_some(), |ui| {
            icon_button(ui, label, icon, OnVideo)
        })
        .inner;
    let Some(session) = session else {
        return false;
    };
    if speaker.clicked() {
        session.send(Control::Mute);
        if speaker.interact_pointer_pos().is_some() {
            speaker.surrender_focus();
        }
    }

    let ctx = ui.ctx();
    let memory_id = Id::new(CLASSIC_VOLUME_ID);
    let previous = ctx.data(|data| data.get_temp::<Id>(memory_id));
    let slider = previous.and_then(|id| ctx.read_response(id));
    let active = slider
        .as_ref()
        .is_some_and(|slider| slider.has_focus() || slider.is_pointer_button_down_on());
    let hovering = match &slider {
        Some(slider) => ui.rect_contains_pointer(speaker.rect.union(slider.interact_rect)),
        None => speaker.hovered(),
    };
    let focused = speaker.has_focus() || active;
    let fraction = ctx.animate_bool_with_time_and_easing(
        speaker.id.with("volume_reveal"),
        hovering || focused,
        if focused || reduce_motion { 0.0 } else { 0.12 },
        egui::emath::easing::cubic_out,
    );
    const WIDTH: f32 = 72.0;
    let width = fraction * WIDTH;
    if width < 1.0 {
        ctx.data_mut(|data| data.remove::<Id>(memory_id));
        return false;
    }

    let (slot, _) = ui.allocate_exact_size(vec2(width, speaker.rect.height()), Sense::hover());
    let slider_area =
        Rect::from_min_size(pos2(slot.left(), slot.center().y - 10.), vec2(WIDTH, 20.0));
    let mut slider_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("classic_volume")
            .max_rect(slider_area)
            .layout(Layout::top_down(Align::Min)),
    );
    slider_ui.set_clip_rect(slider_ui.clip_rect().intersect(slot));
    slider_ui.spacing_mut().slider_width = WIDTH;
    let mut volume = state.volume.clamp(0.0, 100.0);
    let slider = theme::slider_with_fill(
        &mut slider_ui,
        Slider::new(&mut volume, 0.0..=100.0).show_value(false),
        control_bar::ACCENT,
    );
    slider.widget_info(|| egui::WidgetInfo::slider(true, volume, "Volume"));
    if slider.drag_stopped() {
        slider.surrender_focus();
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(memory_id, slider.id));
    if slider.changed() {
        session.send(Control::Volume(volume));
    }
    true
}

fn volume_slider(
    ui: &mut Ui,
    session: Option<&Session>,
    mut volume: f64,
    width: f32,
    fill: Color32,
) {
    ui.add_enabled_ui(session.is_some(), |ui| {
        ui.spacing_mut().slider_width = width;
        let response = theme::slider_with_fill(
            ui,
            Slider::new(&mut volume, 0.0..=100.0).show_value(false),
            fill,
        );
        response.widget_info(|| egui::WidgetInfo::slider(session.is_some(), volume, "Volume"));
        if response.on_hover_text("Volume").changed()
            && let Some(session) = session
        {
            session.send(Control::Volume(volume));
        }
    });
}

fn autoplay_toggle(ui: &mut Ui, enabled: &mut bool, width: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click());
    if response.clicked() {
        *enabled = !*enabled;
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *enabled, "Autoplay")
    });
    let track = Rect::from_center_size(rect.center(), vec2((width - 4.).min(34.), 17.));
    let fill = if theme::interface_theme(ui.ctx()).glass_ui() {
        Color32::from_white_alpha(if *enabled { 108 } else { 42 })
    } else {
        theme::video_button_fill(ui.ctx())
    };
    let border = if response.has_focus() {
        Stroke::new(1., Color32::WHITE)
    } else {
        Stroke::new(1.0, Color32::from_black_alpha(64))
    };
    ui.painter()
        .rect(track, 9, fill, border, egui::StrokeKind::Inside);
    let offset = (track.width() - track.height()) * 0.5;
    let center = track.center() + vec2(if *enabled { offset } else { -offset }, 0.0);
    ui.painter().circle_filled(center, 6.5, Color32::WHITE);
    let icon = if *enabled { Icon::Play } else { Icon::Pause };
    icon.paint(ui.painter(), center, 7.5, Color32::from_gray(32));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if *enabled {
            "Autoplay is on"
        } else {
            "Autoplay is off"
        });
}

fn collection_button(
    ui: &mut Ui,
    video: &Video,
    service: &Service,
    collections: &[Collection],
    kind: theme::ButtonKind,
) -> bool {
    let response = icon_button(ui, "Add to Collection", Icon::Collection, kind);
    egui::Popup::menu(&response)
        .style(theme::menu_style(&response.ctx))
        .show(|ui| collection_choices(ui, video, service, collections))
        .is_some_and(|popup| popup.inner)
}

fn collection_choices(
    ui: &mut Ui,
    video: &Video,
    service: &Service,
    collections: &[Collection],
) -> bool {
    if ui.button("New collection…").clicked() {
        ui.close();
        return true;
    }
    if !collections.is_empty() {
        ui.separator();
    }
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .show(ui, |ui| {
            for collection in collections {
                if ui.button(&collection.name).clicked() {
                    service.send(Action::AddToCollection(
                        CollectionTarget::Existing(collection.id),
                        vec![video.id],
                        Some(video.id),
                    ));
                    ui.close();
                }
            }
        });
    false
}

fn play_button(ui: &mut Ui, state: &Playback, reduce_motion: bool) -> bool {
    let paused = state.paused || state.ended;
    let label = if paused { "Play (K)" } else { "Pause (K)" };
    let icon = Icon::playback(ui, paused, reduce_motion);
    ui.add_enabled_ui(state.ready && state.error.is_none(), |ui| {
        icon_button(ui, label, icon, OnVideo)
    })
    .inner
    .clicked()
}

fn transport_arrow(
    ui: &mut Ui,
    forward: bool,
    target: Option<usize>,
    prefs: &PlayerPreferences,
    session: Option<&Session>,
    state: &Playback,
    classic: bool,
) -> Option<usize> {
    let (label, icon) = match (prefs.transport, forward) {
        (Transport::Playlist, false) => ("Previous video (↑)", Icon::Previous),
        (Transport::Playlist, true) => ("Next video (↓)", Icon::Next),
        (Transport::Seek, false) => ("Rewind", Icon::Rewind),
        (Transport::Seek, true) => ("Fast forward", Icon::FastForward),
        (Transport::Speed, false) => ("Decrease playback speed", Icon::Rewind),
        (Transport::Speed, true) => ("Increase playback speed", Icon::FastForward),
    };
    let enabled = if prefs.transport == Transport::Playlist {
        target.is_some()
    } else {
        state.ready && session.is_some() && state.error.is_none()
    };
    let response = if classic {
        ui.add_enabled_ui(enabled, |ui| icon_button(ui, label, icon, OnVideo))
            .inner
    } else {
        glass_icon(ui, label, icon, enabled, None)
    };
    if response.clicked() {
        let direction = if forward { 1.0 } else { -1.0 };
        match prefs.transport {
            Transport::Playlist => return target,
            Transport::Seek => session?.send(Control::Seek(
                (state.position + direction * f64::from(prefs.arrow_seek_seconds))
                    .clamp(0.0, state.duration.max(0.0)),
            )),
            Transport::Speed => session?.send(Control::Speed(crate::player::stepped_speed(
                state.speed + direction * crate::player::SPEED_STEP,
            ))),
        }
    }
    None
}

fn toggle_captions(session: &Session, state: &Playback) {
    if state.subtitle.is_none() {
        if let Some(track) = state
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Subtitle)
        {
            session.send(Control::Track(TrackKind::Subtitle, Some(track.id)));
        }
    } else {
        session.send(Control::Captions(!state.captions));
    }
}

fn chapter_row(ui: &mut Ui, chapter: &Chapter, height: f32, current: bool) -> egui::Response {
    let colors = theme::palette(ui);
    let time = clock(chapter.at);
    let label = format!("{} · {time}", chapter.title);
    let response = sidebar_list_row(ui, &label, height, current, true);
    let content = response.rect.shrink2(vec2(10., 0.));
    let painter = ui.painter_at(content);
    let stamp = painter.text(
        content.right_center(),
        Align2::RIGHT_CENTER,
        time,
        theme::timestamp_font(12.),
        colors.muted,
    );
    let font = if current {
        theme::font(13.)
    } else {
        egui::FontId::proportional(13.)
    };
    let title = content.with_max_x(stamp.left() - 10.);
    theme::cut(
        &painter.with_clip_rect(title),
        pos2(title.left(), title.center().y - 8.),
        title.width(),
        &chapter.title,
        font,
        colors.text,
    );
    response.on_hover_text(label)
}

fn show_details(ui: &mut Ui, video: &Video, service: &Service, saved: bool) {
    let colors = theme::palette(ui);
    ui.add_space(8.0);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 34.),
        Layout::right_to_left(Align::Min),
        |ui| {
            let share = icon_button(ui, "Share video", Icon::Share, Secondary);
            share_video(&share, video, service);
            super::watch_later_control(ui, video.id, saved, service);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.add(egui::Label::new(strong(&video.title, 22.0, colors.text)).wrap());
            });
        },
    );
    ui.add_space(4.0);
    theme::panel_frame(ui, 14, 12).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(strong("Video details", 15.0, colors.text));
        ui.add_space(7.0);
        video_metadata(ui, video);
        if button(ui, "Open containing folder", Some(Icon::Folder), Quiet).clicked() {
            service.send(Action::Reveal(video.path.clone()));
        }
    });
}

fn share_video(button: &egui::Response, video: &Video, service: &Service) {
    #[cfg(target_os = "macos")]
    if button.clicked()
        && super::macos::share_file(&video.path, button.rect, button.ctx.viewport_rect())
    {
        return;
    }
    egui::Popup::menu(button)
        .style(theme::menu_style(&button.ctx))
        .show(|ui| {
            if ui.button("Copy file path").clicked() {
                ui.ctx()
                    .copy_text(video.path.to_string_lossy().into_owned());
                ui.close();
            }
            if ui.button("Open containing folder").clicked() {
                service.send(Action::Reveal(video.path.clone()));
                ui.close();
            }
        });
}

struct PrecisePanel {
    rect: Rect,
    track: Rect,
    strip: Rect,
    tiles: usize,
}

impl PrecisePanel {
    fn new(picture: Rect, aspect: f32) -> Self {
        let width = picture.width();
        let header = (width * 0.025 + 17.0)
            .clamp(36.0, 44.0)
            .min(picture.height());
        let strip_height = (width * 0.045 + 25.0)
            .clamp(36.0, 88.0)
            .min(picture.height() - header);
        let rect = picture.with_min_y(picture.bottom() - strip_height - header);
        let strip = rect.with_min_y(rect.bottom() - strip_height);
        let ideal_width = (strip_height * aspect).clamp(60.0, 168.0);
        let tiles = (strip.width() / ideal_width).round() as usize;
        let track = Rect::from_min_max(
            pos2(rect.left(), strip.top() - 17.0),
            pos2(rect.right(), strip.top() - 1.0),
        );
        Self {
            rect,
            track,
            strip,
            tiles: tiles.clamp(1, MAX_FILMSTRIP_TILES - 1) | 1,
        }
    }

    fn tile_pitch(&self) -> f32 {
        self.strip.width() / self.tiles as f32
    }

    fn tile(&self, index: usize, selected: usize, offset: f32) -> Rect {
        let edge = |index: usize| {
            self.strip.center().x
                + (index as f32 - selected as f32 - offset - 0.5) * self.tile_pitch()
        };
        Rect::from_min_max(
            pos2(edge(index), self.strip.top()),
            pos2(edge(index + 1), self.strip.bottom()),
        )
    }

    fn actions(&self) -> [Rect; 2] {
        let action_size = vec2(
            32.0_f32.min(self.rect.width() / 3.0),
            (self.track.top() - self.rect.top()).max(1.0),
        );
        let play = Rect::from_min_size(self.rect.left_top() + vec2(4.0, 0.0), action_size);
        let cancel = Rect::from_min_size(
            self.rect.right_top() - vec2(action_size.x + 4.0, 0.0),
            action_size,
        );
        [play, cancel]
    }

    fn show(
        &self,
        ui: &mut Ui,
        gesture: &mut Gesture,
        video: &Video,
        state: &Playback,
        peek: &Peek,
    ) -> Option<Finish> {
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("precise_panel")
                .max_rect(self.rect),
        );
        ui.set_clip_rect(ui.clip_rect().intersect(self.rect));
        *ui.visuals_mut() = ui.ctx().style_of(egui::Theme::Dark).visuals.clone();
        ui.interact(self.rect, ui.id().with("background"), Sense::CLICK);

        gesture.configure_strip(self.tiles, video.media.fps);
        let rail = Rail {
            rect: self.track,
            end: gesture.duration,
        };
        let slide = ui.interact(rail.rect, ui.id().with("rail"), Sense::click_and_drag());
        let pointer = ui.input(ScrubPointer::read);
        let on_layer = |point| ui.ctx().layer_id_at(point) == Some(ui.layer_id());
        let point = pointer.release.or(pointer.point);
        if gesture.held.is_none()
            && let Some(press) = pointer.press
            && slide.interact_rect.contains(press)
            && on_layer(press)
        {
            gesture.rail_drag = Some(rail.rect);
        }
        if let Some(rect) = gesture.rail_drag {
            if !pointer.cancelled
                && let Some(point) = point.filter(|_| pointer.down || pointer.release.is_some())
            {
                gesture.select(Rail { rect, ..rail }.time(point));
            }
            if !pointer.down || pointer.cancelled {
                gesture.rail_drag = None;
            }
        }
        if gesture.held.is_none()
            && gesture.rail_drag.is_none()
            && let Some(press) = pointer.press
            && self.strip.intersect(ui.clip_rect()).contains(press)
            && on_layer(press)
        {
            gesture.start_drag(press);
        }
        let points_per_second = self.tile_pitch() / gesture.seconds_per_tile() as f32;
        let dragged = !pointer.cancelled
            && point.is_some_and(|point| gesture.drag_to(point.x, points_per_second));
        let click = pointer
            .release
            .zip(gesture.strip_press())
            .filter(|(released, pressed)| {
                !dragged && !pointer.cancelled && released.distance(*pressed) < 4.0
            });
        if !pointer.down || pointer.cancelled {
            gesture.stop_drag();
        }

        let filmstrip = gesture.filmstrip();
        let slots: Vec<_> = filmstrip
            .positions
            .iter()
            .copied()
            .enumerate()
            .map(|(index, at)| {
                let frame = self.tile(index, filmstrip.selected, filmstrip.offset);
                let visible = frame.intersect(self.strip);
                let response = ui.interact(visible, ui.id().with(index), Sense::click_and_drag());
                (frame, at, response)
            })
            .collect();
        if gesture.held.is_none()
            && let Some((_, at, _)) = slots.iter().find(|(_, _, slot)| {
                if let Some((released, pressed)) = click {
                    slot.interact_rect.contains(pressed) && slot.interact_rect.contains(released)
                } else {
                    pointer.release.is_none() && slot.clicked()
                }
            })
        {
            gesture.select(*at);
            ui.ctx().request_repaint();
        }

        let [play, cancel] = self.actions();
        let start = video_action(&ui, play, "Play", Icon::Play);
        let dismiss = video_action(&ui, cancel, "Cancel", Icon::Close);

        if cancel.left() - play.right() >= 64.0 {
            theme::paint_overlay_time(
                ui.painter(),
                pos2(
                    self.strip.center().x,
                    (self.rect.top() + self.track.top()) * 0.5,
                ),
                if gesture.is_short_clip() {
                    precise_clock(gesture.position())
                } else {
                    clock(gesture.position().floor())
                },
                12.0,
            );
        }

        slide.widget_info(|| {
            egui::WidgetInfo::slider(true, gesture.position(), "Precise playback position")
        });
        let hovered = slide.hover_pos().map(|point| {
            let at = rail.time(point);
            (pos2(rail.x(at), rail.rect.top()), at)
        });
        let rail_active = hovered.is_some()
            || slide.has_focus()
            || gesture.rail_drag.is_some()
            || gesture.held.is_some();
        slide.on_hover_text(format!(
            "Current {} · Seek to {}",
            clock(gesture.original),
            precise_clock(gesture.position())
        ));
        rail.paint(&ui, state, gesture.position(), hovered, true, rail_active);

        let painter = ui.painter().with_clip_rect(self.strip);
        for (frame, at, slot) in slots {
            let key = gesture.key(video, at);
            if let Some(texture) = peek.nearest_frame(key) {
                let scale = frame.size() / texture.size_vec2();
                let uv = Rect::from_center_size(pos2(0.5, 0.5), scale / scale.max_elem());
                painter.image(texture.id(), frame, uv, Color32::WHITE);
            }
            if slot.has_focus() {
                painter.rect_filled(frame, 0, Color32::from_white_alpha(28));
            }
            slot.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, precise_clock(at))
            });
            slot.on_hover_cursor(if dragged && pointer.down {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        let center = self.strip.center().x;
        let marker = [
            pos2(center, self.strip.top()),
            pos2(center, self.strip.bottom()),
        ];
        painter.line_segment(marker, Stroke::new(3.0, Color32::from_black_alpha(150)));
        painter.line_segment(marker, Stroke::new(1.5, Color32::WHITE));

        if start {
            Some(Finish::Play)
        } else if dismiss {
            Some(Finish::Cancel)
        } else {
            None
        }
    }
}

fn precise_clock(at: f64) -> String {
    let ms = (at * 1000.0).round() as u64;
    format!("{}.{:03}", clock((ms / 1000) as f64), ms % 1000)
}

struct ScrubPointer {
    press: Option<Pos2>,
    release: Option<Pos2>,
    point: Option<Pos2>,
    down: bool,
    cancelled: bool,
}

impl ScrubPointer {
    fn read(input: &egui::InputState) -> Self {
        let release = input.events.iter().rev().find_map(|event| match event {
            egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: false,
                pos,
                ..
            } => Some(*pos),
            _ => None,
        });
        Self {
            press: super::primary_press(input).map(|(pos, _)| pos),
            release,
            point: input.pointer.interact_pos(),
            down: input.pointer.primary_down(),
            cancelled: !input.focused
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Touch {
                            phase: egui::TouchPhase::Cancel,
                            ..
                        }
                    )
                }),
        }
    }
}

struct Timeline {
    position: f64,
    hovered: Option<(Pos2, f64)>,
}

fn timeline(
    ui: &mut Ui,
    send: Option<impl FnMut(Control)>,
    state: &Playback,
    end: f64,
    seeking: &mut SeekBar,
    row_height: Option<f32>,
    precise_track: Option<Rect>,
) -> Timeline {
    use egui::{Key, Modifiers};
    let enabled = send.is_some()
        && state.ready
        && !state.precise_seeking
        && state.error.is_none()
        && end > 0.0;
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(18.0), row_height.unwrap_or(18.0)),
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let mut position = seeking
        .drag
        .as_ref()
        .map(|drag| drag.position)
        .or_else(|| seeking.surface_position())
        .unwrap_or(state.position)
        .clamp(0.0, end.max(0.0));
    let rail = Rail { rect, end };
    if let Some(mut send) = send.filter(|_| enabled) {
        let pointer = ui.input(ScrubPointer::read);
        if let Some(press) = pointer.press
            && seeking.precise.gesture.is_none()
            && response.interact_rect.contains(press)
            && ui.ctx().layer_id_at(press) == Some(ui.layer_id())
        {
            seeking.drag = Some(RailDrag {
                rail: response.id,
                origin: press,
                position: rail.time(press),
            });
        }
        if let Some(drag) = seeking
            .drag
            .as_mut()
            .filter(|drag| drag.rail == response.id)
        {
            let point = pointer.release.or(pointer.point);
            if pointer.cancelled || (!pointer.down && pointer.release.is_none()) {
                seeking.drag = None;
            } else if let Some(point) = point {
                position = rail.time(point);
                if let Some(track) = precise_track
                    && precise_seek::pulled_up(drag.origin, point)
                {
                    let (left, right) = rail.ends();
                    let mut held = HeldScrub::new(drag.origin, left, right - left);
                    let (start, stop) = Rail {
                        rect: track,
                        ..rail
                    }
                    .ends();
                    position = held.enter_precise(point, end, Some(stop - start));
                    seeking.drag = None;
                    ui.memory_mut(|memory| memory.surrender_focus(Id::new(super::SEARCH_ID)));
                    send(
                        seeking
                            .precise
                            .start(state, position, end, pointer.down.then_some(held)),
                    );
                    ui.ctx().request_repaint();
                } else if pointer.release.is_some() {
                    seeking.drag = None;
                    seeking.mark_pending(position);
                    send(Control::Seek(position));
                } else {
                    drag.position = position;
                }
            }
        }
        if response.has_focus() && seeking.precise.gesture.is_none() {
            let target = ui.input_mut(|input| {
                if input.consume_key(Modifiers::NONE, Key::Home) {
                    Some(0.0)
                } else if input.consume_key(Modifiers::NONE, Key::End) {
                    Some(end)
                } else {
                    None
                }
            });
            if let Some(target) = target {
                position = target;
                seeking.mark_pending(target);
                send(Control::Seek(target));
            }
        }
    }
    response.widget_info(|| egui::WidgetInfo::slider(enabled, position, "Playback position"));
    let held_here = seeking
        .drag
        .as_ref()
        .is_some_and(|drag| drag.rail == response.id);
    let hovered = if end > 0.
        && state.error.is_none()
        && seeking.precise.gesture.is_none()
        && (ui.rect_contains_pointer(rect) || held_here)
    {
        ui.ctx().pointer_hover_pos().map(|point| {
            let at = rail.time(point);
            (pos2(rail.x(at), rect.top()), at)
        })
    } else {
        None
    };
    let hot = hovered.is_some() || held_here || response.has_focus();
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    rail.paint(ui, state, position, hovered, enabled, hot);
    Timeline { position, hovered }
}

fn slider_bounds(track: Rect, end: f64, mini: bool) -> Rect {
    let time_width = 15.0 + clock(end).len() as f32 * 7.0;
    if !mini && track.width() >= time_width * 2.0 + 64.0 {
        track.shrink2(vec2(time_width, 0.0))
    } else {
        track
    }
}

struct Rail {
    rect: Rect,
    end: f64,
}

impl Rail {
    fn ends(&self) -> (f32, f32) {
        let inset = 7.0_f32.min(self.rect.width() * 0.25);
        (self.rect.left() + inset, self.rect.right() - inset)
    }

    fn x(&self, at: f64) -> f32 {
        let (left, right) = self.ends();
        left + (right - left) * (at / self.end.max(0.001)).clamp(0.0, 1.0) as f32
    }

    fn time(&self, pointer: Pos2) -> f64 {
        let (left, right) = self.ends();
        f64::from(((pointer.x - left) / (right - left).max(0.001)).clamp(0.0, 1.0)) * self.end
    }

    fn paint(
        &self,
        ui: &Ui,
        state: &Playback,
        position: f64,
        hovered: Option<(Pos2, f64)>,
        enabled: bool,
        hot: bool,
    ) {
        let (left, right) = self.ends();
        let y = self.rect.center().y;
        let painter = ui.painter();
        let ctx = ui.ctx();
        let style = ctx
            .data(|data| data.get_temp::<Option<InterfaceTheme>>(Id::new("progress_bar_style")))
            .flatten()
            .unwrap_or_else(|| theme::interface_theme(ctx));
        let weight = if style.glass_ui() { 4.0 } else { 3.0 };
        let played = control_bar::ACCENT;
        let track = Rect::from_min_max(pos2(left, y - weight * 0.5), pos2(right, y + weight * 0.5));
        let high_contrast = DisplayMaterial::current(ui.ctx()) == DisplayMaterial::HighContrast;
        let rail = if high_contrast {
            Color32::from_gray(136)
        } else {
            match style {
                InterfaceTheme::LiquidGlass
                    if DisplayMaterial::system(ctx) == DisplayMaterial::Glass =>
                {
                    Color32::from_gray(134)
                        .lerp_to_gamma(Color32::from_white_alpha(115), theme::glass_amount(ctx))
                }
                _ => Color32::from_gray(134),
            }
        };
        let handle = self.x(position);
        let markers = ui.ctx().data(|data| {
            data.get_temp::<bool>(Id::new("player_chapter_markers"))
                .unwrap_or(true)
        });
        // Clip each paint pass to its chapter. The gaps reveal the actual video.
        let mut start = left;
        for boundary in state
            .chapters
            .iter()
            .filter(|c| markers && c.at > 0.0 && c.at < self.end)
            .map(|c| self.x(c.at))
            .chain(std::iter::once(right + 0.5))
        {
            let stop = (boundary - 0.5).min(right);
            if stop > start {
                let clip = self.rect.with_min_x(start).with_max_x(stop);
                let segment = painter.with_clip_rect(painter.clip_rect().intersect(clip));
                segment.rect_filled(track, 2, rail);
                if let Some((point, _)) = hovered {
                    segment.rect_filled(
                        track.with_max_x(point.x),
                        2,
                        Color32::from_white_alpha(164),
                    );
                }
                if handle > start {
                    segment.rect_filled(track.with_max_x(handle), 2, played);
                }
            }
            start = start.max(boundary + 0.5);
        }
        if enabled {
            painter.circle_filled(pos2(handle, y), if hot { 6.5 } else { 5.0 }, played);
        }
        if let Some((point, _)) = hovered
            && (point.x - handle).abs() > 7.0
        {
            painter.circle_filled(pos2(point.x, y), 3.5, played);
        }
    }
}

#[test]
fn remaining_time_can_follow_the_playback_rate() {
    let mut prefs = PlayerPreferences::default();
    assert_eq!(end_time_label(180., 100., &prefs, 2.), "3:00");
    prefs.remaining_time = true;
    assert_eq!(end_time_label(180., 100., &prefs, 2.), "−1:20");
    prefs.speed_adjusted_remaining = true;
    assert_eq!(end_time_label(180., 100., &prefs, 2.), "−0:40");
}

fn end_time_label(end: f64, position: f64, prefs: &PlayerPreferences, speed: f64) -> String {
    if prefs.remaining_time {
        let rate = if prefs.speed_adjusted_remaining && speed.is_finite() && speed > 0.0 {
            speed
        } else {
            1.0
        };
        format!("−{}", clock((end - position).max(0.0) / rate))
    } else {
        clock(end)
    }
}

fn toggle_time_display(ui: &Ui, response: egui::Response, remaining: &mut bool) {
    let label = if *remaining {
        "Show total duration"
    } else {
        "Show remaining time"
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            3,
            Stroke::new(1.0, Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }
    if response
        .on_hover_text(label)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *remaining = !*remaining;
    }
}

fn clock(seconds: f64) -> String {
    library::duration(Some(seconds))
}

#[cfg(test)]
#[test]
fn playlist_artwork_leaves_room_for_two_title_lines_and_the_folder() {
    let ctx = Context::default();
    theme::install(&ctx);
    let mut video = super::tests::catalog_video(1);
    video.title = "A long video title that wraps onto a second line in the playlist".into();
    for (width, compact, expected_thumb) in
        [(420., false, 168.), (420., true, 128.), (220., false, 99.)]
    {
        let mut row = Rect::NOTHING;
        let frame = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(width);
            row = queue_row(
                ui,
                &mut Thumbnails::default(),
                &video,
                queue_row_height(width, compact),
                false,
            )
            .rect;
        });
        let image = frame
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Rect(r) if (r.rect.width() - expected_thumb).abs() < 0.1 => {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("playlist thumbnail");
        assert!((image.aspect_ratio() - 16. / 9.).abs() < 0.01);
        let label = |text: &str| {
            frame
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == text => Some(t),
                    _ => None,
                })
                .unwrap()
        };
        let title = label(&video.title);
        let folder = label("videos").visual_bounding_rect();
        assert_eq!(title.galley.rows.len(), 2);
        let title = title.visual_bounding_rect();
        assert!(title.left() > image.right() && title.bottom() <= folder.top());
        assert!(row.contains_rect(title) && row.contains_rect(folder));
        frame.drop_without_applying_deltas();
    }
}

#[cfg(test)]
#[test]
fn chapter_rows_use_playlist_selection_and_keep_long_titles_clear_of_timestamps() {
    let ctx = Context::default();
    theme::install(&ctx);
    let chapter = Chapter {
        title: "A long chapter title that must fit inside a narrow sidebar".into(),
        at: 3723.,
    };
    for style in InterfaceTheme::ALL {
        theme::apply_style(&ctx, style, 62);
        for appearance in [egui::Theme::Light, egui::Theme::Dark] {
            ctx.set_theme(appearance);
            let mut row = Rect::NOTHING;
            let mut active = Color32::TRANSPARENT;
            let frame = ctx.run_ui(Default::default(), |ui| {
                ui.set_width(220.);
                active = theme::palette(ui).active;
                row = chapter_row(ui, &chapter, 32., true).rect;
            });
            assert_eq!(row.width(), 220.);
            assert!(frame.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Rect(r) if r.rect == row && r.fill == active)));
            let painted_text = |label: &str| {
                frame
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == label => Some((s.clip_rect, t)),
                        _ => None,
                    })
                    .unwrap()
            };
            let (title_clip, _) = painted_text(&chapter.title);
            let (_, time) = painted_text(&clock(chapter.at));
            assert!(title_clip.right() < time.pos.x);
            assert!(row.contains_rect(time.visual_bounding_rect()));
            frame.drop_without_applying_deltas();
        }
    }
}

#[cfg(test)]
#[test]
fn theater_matches_the_measured_chrome_viewports_in_every_theme() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated layout test"));
    let mut watch = Watch::new(ctx.clone(), None);
    let mut video = crate::ui::tests::catalog_video(1);
    video.media.width = Some(1920);
    video.media.height = Some(1080);
    watch.video = Some(video);
    watch.theater = true;
    for theme in InterfaceTheme::ALL {
        theme::apply_style(&ctx, theme, 62);
        watch.prefs.interface_theme = theme;
        for (width, height, expected_height) in [
            (1920., 873., 704.),
            (1440., 900., 731.),
            (1280., 600., 480.),
            (1015., 550., 480.),
            (1014., 550., 381.),
            (900., 540., 371.),
            (760., 900., 427.5),
            (420., 700., 240.),
        ] {
            let window = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| {
                    // The player sits below the app header. Its height limit uses the whole window.
                    ui.add_space(58.);
                    watch.show(
                        ui,
                        &mut library::Browser::default(),
                        false,
                        &mut Thumbnails::default(),
                        &service,
                        &Settings::default(),
                    );
                },
            );
            let has_rect = |test: &dyn Fn(&egui::epaint::RectShape) -> bool| {
                output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if test(r)))
            };
            assert!(
                has_rect(&|r| r.fill == Color32::BLACK
                    && r.rect.left() == 0.
                    && (r.rect.width() - (width - 15.)).abs() <= 0.5
                    && (r.rect.height() - expected_height).abs() <= 0.5),
                "{theme:?} {width} × {height}"
            );
            assert!(has_rect(
                &|r| r.rect.right() == width && (r.rect.width() - 7.).abs() < 0.1
            ));
            output.drop_without_applying_deltas();
        }
    }
}

#[cfg(test)]
#[test]
fn playlist_uses_the_page_background_and_eight_point_thumbnail_gaps() {
    let ctx = Context::default();
    theme::install(&ctx);
    let mut watch = Watch::new(ctx.clone(), None);
    let mut browser = library::Browser::default();
    browser.applied(
        0,
        0,
        Some(3),
        (1..=3).map(super::tests::catalog_video).collect(),
        false,
    );
    for style in InterfaceTheme::ALL {
        theme::apply_style(&ctx, style, 62);
        watch.prefs.interface_theme = style;
        for appearance in [egui::Theme::Light, egui::Theme::Dark] {
            ctx.set_theme(appearance);
            let draw = |ui: &mut Ui| {
                ui.set_width(420.);
                watch.page_playlist(ui, &mut browser, &mut Thumbnails::default(), 1, 500.);
            };
            let output = ctx.run_ui(Default::default(), draw);
            let mut artwork = Vec::new();
            for item in &output.shapes {
                if let egui::Shape::Rect(shape) = &item.shape {
                    if (shape.rect.width() - 168.).abs() < 0.01 {
                        artwork.push(shape.rect);
                    }
                    if shape.rect.width() >= 400. && shape.rect.height() > 60. {
                        assert!(
                            shape.fill == Color32::TRANSPARENT && shape.stroke.is_empty(),
                            "unexpected playlist frame or filled row: {shape:?}"
                        );
                    }
                }
            }
            assert_eq!(artwork.len(), 3);
            for pair in artwork.windows(2) {
                assert!((pair[1].top() - pair[0].bottom() - 8.).abs() < 0.01);
            }
            output.drop_without_applying_deltas();
        }
    }
}

#[cfg(test)]
#[test]
fn default_watch_playlist_switches_tabs_and_plays_from_the_catalog() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated playlist test"));
    let mut browser = library::Browser::default();
    let videos = (1..=24).map(super::tests::catalog_video).collect();
    browser.applied(0, 0, Some(24), videos, false);
    let mut watch = Watch::new(ctx.clone(), None);
    watch.video = browser.get(12).cloned();
    let click = |pos| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    for style in InterfaceTheme::ALL {
        theme::apply_style(&ctx, style, 62);
        watch.prefs.interface_theme = style;
        watch.reveal_playlist_item = true;
        let mut render = |events| {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1300., 800.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    watch.show(
                        ui,
                        &mut browser,
                        false,
                        &mut Thumbnails::default(),
                        &service,
                        &Settings::default(),
                    )
                },
            );
            let labels: std::collections::HashMap<_, _> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.text().to_owned(), t.pos + t.galley.size() * 0.5))
                    }
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            labels
        };
        render(vec![]);
        let playlist = render(vec![]);
        assert!(playlist.contains_key("Video 14"));
        let chapters = render(click(playlist["Chapters"]));
        assert!(chapters.contains_key("No chapters in this video"));
        let playlist = render(click(chapters["Playlist"]));
        render(vec![]);
        render(click(playlist["Video 14"]));
        assert_eq!(watch.take_request(), Some(13), "{style:?}");
    }
}

#[cfg(test)]
#[test]
fn standard_player_matches_chrome_and_keeps_its_frame_when_the_sidebar_changes() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated layout test"));
    let mut watch = Watch::new(ctx.clone(), None);
    let mut video = crate::ui::tests::catalog_video(1);
    video.media.width = Some(1920);
    video.media.height = Some(1080);
    watch.video = Some(video.clone());
    for theme in InterfaceTheme::ALL {
        theme::apply_style(&ctx, theme, 62);
        watch.prefs.interface_theme = theme;
        for (width, height, expected) in [
            (1920., 873., vec2(1337.25, 752.20)),
            (1512., 649., vec2(947.55, 532.98)),
            (1440., 900., vec2(989.25, 556.45)),
            (1370., 886., vec2(938.5, 527.91)),
            (1280., 600., vec2(860.44, 483.98)),
            (1060., 610., vec2(677., 380.81)),
            (1015., 550., vec2(640., 360.)),
            (1014., 550., vec2(999., 381.)),
            (760., 900., vec2(745., 427.5)),
            (760., 1400., vec2(745., 427.5)),
        ] {
            let mut original = None;
            let mut playlist = None;
            for (index, (list, settings)) in
                [(false, false), (true, false), (true, true), (true, false)]
                    .into_iter()
                    .enumerate()
            {
                watch.playlist_open = list;
                if watch.settings_panel.open != settings {
                    watch.toggle_settings();
                }
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, height))),
                        ..Default::default()
                    },
                    |ui| {
                        ui.add_space(58.);
                        watch.prepare_sidebars(ui);
                        watch.show(
                            ui,
                            &mut library::Browser::default(),
                            false,
                            &mut Thumbnails::default(),
                            &service,
                            &Settings::default(),
                        );
                    },
                );
                let frame = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(r)
                            if r.fill == Color32::BLACK
                                && (r.rect.size() - expected).abs().max_elem() < 0.5 =>
                        {
                            Some(r.rect)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        panic!("{theme:?} {width} × {height}, phase {index}: expected {expected:?}")
                    });
                assert_eq!(*original.get_or_insert(frame), frame);
                if index == 0 {
                    let queue = output.shapes.iter().find_map(|shape| match &shape.shape {
                        egui::Shape::Text(t) if t.galley.text() == "Playlist" => Some(t.pos),
                        _ => None,
                    });
                    if width >= 1015. {
                        let tab = queue.expect("playlist tabs beside the player");
                        assert!(tab.x > frame.right() + WATCH_MARGIN && tab.x < width);
                    } else if height == 1400. {
                        assert!(queue.expect("queue below the player").y > frame.bottom());
                    }
                } else if settings {
                    let (panel, clip) = watch.settings_panel.sidebar_geometry().unwrap();
                    assert_eq!(Some(panel), playlist);
                    assert_eq!(panel, clip);
                } else {
                    let panel = watch.sidebars.playlist.unwrap();
                    assert_eq!(*playlist.get_or_insert(panel), panel);
                    if width >= 1015. {
                        assert!((panel.left() - frame.right() - WATCH_MARGIN).abs() < 1.);
                        assert!((frame.left() - WATCH_MARGIN).abs() < 1.);
                        assert!(
                            (width - theme::PAGE_SCROLL_GUTTER - WATCH_MARGIN - panel.right())
                                .abs()
                                < 1.
                        );
                    }
                }
                output.drop_without_applying_deltas();
            }
        }
    }
}

#[cfg(test)]
#[test]
fn letterboxed_playback_has_one_rounded_outline_in_every_theme() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated layout test"));
    let mut watch = Watch::new(ctx.clone(), None);
    let mut video = crate::ui::tests::catalog_video(1);
    video.media.width = Some(240);
    video.media.height = Some(480);
    watch.video = Some(video);
    for theme in InterfaceTheme::ALL {
        watch.prefs.interface_theme = theme;
        theme::apply_style(&ctx, theme, 62);
        let size = vec2(520., 270.);
        let frame = Rect::from_min_size(Pos2::ZERO, size);
        let picture = video_picture(frame, 0.5);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(frame),
                ..Default::default()
            },
            |ui| {
                watch.player_surface(
                    ui,
                    &Playback::default(),
                    &Queue::default(),
                    &service,
                    &[],
                    size,
                );
            },
        );
        let backgrounds: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(r)
                    if r.fill == Color32::BLACK && (r.rect == frame || r.rect == picture) =>
                {
                    Some((r.rect, r.corner_radius))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            backgrounds,
            [(frame, egui::CornerRadius::same(12))],
            "{theme:?}"
        );
        let mask = watch.video_mask(frame);
        assert_eq!(mask.corners(picture), egui::CornerRadius::ZERO);
        let docked = mask.corners(frame.with_min_y(frame.bottom() - 48.));
        assert_eq!([docked.nw, docked.ne, docked.sw, docked.se], [0, 0, 12, 12]);
        output.drop_without_applying_deltas();
    }
}

#[cfg(test)]
#[test]
fn watch_metadata_height_follows_the_title_and_file_details() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated layout test"));
    let mut video = crate::ui::tests::catalog_video(1);
    video.title = "The.Invite.2026.1080p.WEB-DL.x264.6CH.Pahe.SoftSub.ZarFilm".into();
    video.path = std::path::Path::new("/media/movies").join(format!("{}.mkv", video.title));
    video.media.width = Some(1920);
    video.media.height = Some(1040);
    for width in [410., 790.] {
        for height in [360., 890.] {
            let mut contents = egui::Vec2::ZERO;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, height))),
                    ..Default::default()
                },
                |ui| {
                    contents = egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            show_details(ui, &video, &service, false);
                        })
                        .content_size;
                },
            );
            let title = output.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == video.title => Some(t.pos.y),
                _ => None,
            });
            output.drop_without_applying_deltas();
            assert!(
                title.is_some_and(|y| y < 36.),
                "{width} × {height}: title at {title:?}, content {contents:?}"
            );
            assert!(
                contents.y < 570.,
                "{width} × {height}: content {contents:?}"
            );
            assert!(contents.x <= width + 1.);
        }
    }
}

#[cfg(test)]
#[test]
fn docked_settings_keep_their_full_width_even_when_the_picture_is_inset() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated layout test"));
    let mut watch = Watch::new(ctx.clone(), None);
    watch.settings_panel.open = true;
    watch.prefs.reduce_motion = true;
    let window = Rect::from_min_size(Pos2::ZERO, vec2(1200., 700.));
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(window),
            ..Default::default()
        },
        |ui| {
            let video_slot = watch.prepare_sidebars(ui);
            assert_eq!(
                watch.settings_panel.sidebar.unwrap().left() - video_slot.right(),
                WATCH_MARGIN
            );
            let mut inset =
                ui.new_child(egui::UiBuilder::new().max_rect(window.shrink2(vec2(150., 50.))));
            watch.player_surface(
                &mut inset,
                &Playback::default(),
                &Queue::default(),
                &service,
                &[],
                vec2(640., 360.),
            );
            let (panel, clip) = watch.settings_panel.sidebar_geometry().unwrap();
            assert_eq!(clip, panel);
            assert_eq!(
                panel.right(),
                window.right() - theme::PAGE_SCROLL_GUTTER - WATCH_MARGIN
            );
            assert!(panel.width() > watch.prefs.sidebar_width as f32);
            assert_eq!(clip.top(), window.top() + 12.);
            assert_eq!(clip.bottom(), window.bottom() - 12.);
        },
    )
    .drop_without_applying_deltas();
}

#[cfg(test)]
#[test]
fn fullscreen_settings_expand_with_the_display_in_every_theme() {
    let ctx = Context::default();
    let mut watch = Watch::new(ctx.clone(), None);
    watch.fullscreen = true;
    watch.settings_panel.open = true;
    watch.playlist_open = true;
    for theme in InterfaceTheme::ALL {
        for (width, preferred, expected) in [
            (800., 280, 336.),
            (1280., 280, 386.667),
            (1440., 280, 400.),
            (1920., 280, 420.),
            (2560., 450, 590.),
        ] {
            let window = Rect::from_min_size(Pos2::ZERO, vec2(width, 900.));
            for left in [false, true] {
                watch.restore_preferences(PlayerPreferences {
                    settings_left: left,
                    playlist_left: !left,
                    sidebar_width: preferred,
                    interface_theme: theme,
                    reduce_motion: true,
                    ..Default::default()
                });
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(window),
                        ..Default::default()
                    },
                    |ui| {
                        watch.prepare_sidebars(ui);
                        let (panel, clip) = watch.settings_panel.sidebar_geometry().unwrap();
                        assert_eq!(panel, clip);
                        assert!((panel.width() - expected).abs() < 0.01);
                        assert_eq!(
                            if left { panel.left() } else { panel.right() },
                            if left { window.left() } else { window.right() }
                        );
                        let playlist = watch.sidebars.playlist.unwrap();
                        assert_eq!(playlist.width(), preferred as f32);
                        assert!(!panel.intersects(playlist));
                    },
                )
                .drop_without_applying_deltas();
            }
        }
    }
}

#[cfg(test)]
#[test]
fn settings_reverses_in_place_and_restores_each_player_layout() {
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(1200., 720.));
    for fullscreen in [false, true] {
        for theater in [false, true] {
            for playlist in [false, true] {
                let ctx = Context::default();
                let mut watch = Watch::new(ctx.clone(), None);
                watch.fullscreen = fullscreen;
                watch.theater = theater;
                watch.playlist_open = playlist;
                watch.prefs.edge_to_edge = false;
                watch.prefs.settings_left = false;
                let mut time = 0.;
                let mut frame = |watch: &mut Watch, open| {
                    time += 1. / 60.;
                    watch.settings_panel.open = open;
                    let mut video = bounds;
                    ctx.run_ui(
                        egui::RawInput {
                            time: Some(time),
                            screen_rect: Some(bounds),
                            ..Default::default()
                        },
                        |ui| video = watch.prepare_sidebars(ui),
                    )
                    .drop_without_applying_deltas();
                    (video, watch.settings_panel.reveal)
                };
                let (original, _) = frame(&mut watch, false);
                let original_playlist = watch.sidebars.playlist;
                for _ in 0..5 {
                    frame(&mut watch, true);
                }
                if fullscreen {
                    let (panel, _) = watch.settings_panel.sidebar_geometry().unwrap();
                    let before = watch.settings_panel.reveal;
                    assert!((0.01..0.9).contains(&before));
                    let (_, closing) = frame(&mut watch, false);
                    assert!(closing < before && before - closing < 0.2);
                    assert!(
                        watch.settings_panel.sidebar_geometry().unwrap().0.left() > panel.left()
                    );
                    let (_, reopened) = frame(&mut watch, true);
                    assert!((reopened - before).abs() < 0.01);
                    for _ in 0..20 {
                        frame(&mut watch, true);
                    }
                }
                let (docked, reveal) = frame(&mut watch, true);
                assert_eq!(reveal, 1.);
                if fullscreen {
                    assert_eq!(docked, original);
                } else {
                    assert!(watch.sidebars.playlist.is_none());
                    assert!(!watch.theater_visible());
                    let settings = watch.settings_panel.sidebar.unwrap();
                    assert!(docked.right() <= settings.left());
                    if playlist {
                        assert_eq!(Some(settings), original_playlist);
                    }
                }
                for _ in 0..20 {
                    frame(&mut watch, false);
                }
                assert_eq!(frame(&mut watch, false), (original, 0.));
                assert!(watch.settings_panel.sidebar.is_none());
                assert_eq!(watch.sidebars.playlist, original_playlist);
                assert_eq!(watch.theater_visible(), theater);
            }
        }
    }
}

#[cfg(test)]
#[test]
fn classic_restores_the_inline_controller_and_closes_settings_on_return_to_library() {
    use crate::player::options::InterfaceTheme;
    let ctx = Context::default();
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated player test"));
    let mut watch = Watch::new(ctx, None);
    watch.set_control_bar_layout(ControlBarLayout::Compact);
    watch.restore_preferences(PlayerPreferences {
        interface_theme: InterfaceTheme::Classic,
        ..Default::default()
    });
    assert_eq!(watch.layout(), ControlBarLayout::Classic);
    watch.open(super::tests::catalog_video(1), None, &service);
    watch.settings_panel.open = true;
    watch.minimize();
    assert!(!watch.settings_panel.open);
    watch.restore();
    watch.settings_panel.open = true;
    watch.close(&service);
    assert!(!watch.settings_panel.open);
    watch.restore_preferences(PlayerPreferences {
        interface_theme: InterfaceTheme::LiquidGlass,
        ..Default::default()
    });
    assert_eq!(watch.layout(), ControlBarLayout::Compact);
}

#[test]
fn playback_rail_pulls_up_in_the_main_player_and_scrubs_in_the_mini_player() {
    let player = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 480.0));
    let mut playback = Playback::default();
    playback.ready = true;
    for (fullscreen, mini, lift, precise) in [
        (false, false, 23.0, false),
        (false, false, 24.0, true),
        (true, false, 24.0, true),
        (false, true, 60.0, false),
    ] {
        let ctx = Context::default();
        let mut seeking = SeekBar::default();
        let mut commands = Vec::new();
        let bar = Bar::new(
            player,
            ControlBarLayout::Floating,
            ProgressBarPosition::FollowLayout,
            fullscreen,
        );
        let rect = bar.timeline();
        let start = rect.center();
        let end = start - vec2(0.0, lift);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut frame = |events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(player),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if seeking.precise.gesture.is_none() {
                        let mut rail = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                        timeline(
                            &mut rail,
                            Some(|command| commands.push(command)),
                            &playback,
                            45.0,
                            &mut seeking,
                            Some(rect.height()),
                            (!mini).then_some(rect),
                        );
                    }
                },
            )
            .drop_without_applying_deltas();
        };
        frame(vec![]);
        frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
        frame(vec![egui::Event::PointerMoved(end)]);
        frame(vec![button(end, false)]);
        assert_eq!(seeking.precise.gesture.is_some(), precise);
        if precise {
            assert!(
                matches!(commands.as_slice(), [Control::BeginPrecise(_, at)] if (*at - 22.5).abs() < 0.01)
            );
        } else {
            assert!(
                matches!(commands.as_slice(), [Control::Seek(at)] if (*at - 22.5).abs() < 0.01)
            );
        }
    }
}

#[cfg(test)]
#[test]
fn a_diagonal_rail_lift_stays_in_precise_mode_when_released() {
    let ctx = Context::default();
    let track = Rect::from_min_size(pos2(0., 96.), vec2(640., 20.));
    let mut state = Playback::default();
    state.ready = true;
    let start = track.center();
    let event = |pos, pressed| egui::Event::PointerButton {
        pos,
        pressed,
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    };
    for coalesced in [false, true] {
        let mut watch = Watch::new(ctx.clone(), None);
        let mut commands = Vec::new();
        let mut frame = |events| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    watch.update_held_seek(ui);
                    if watch.seeking.precise.gesture.is_none() {
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(track));
                        timeline(
                            &mut child,
                            Some(|command| commands.push(command)),
                            &state,
                            60.,
                            &mut watch.seeking,
                            Some(track.height()),
                            Some(track),
                        );
                    }
                },
            )
            .drop_without_applying_deltas();
        };
        frame(vec![]);
        frame(vec![event(start, true)]);
        if !coalesced {
            frame(vec![egui::Event::PointerMoved(start + vec2(25., -25.))]);
            frame(vec![egui::Event::PointerMoved(start + vec2(32., -32.))]);
        }
        let lifted = start + vec2(40., -40.);
        frame(vec![
            egui::Event::PointerMoved(lifted),
            event(lifted, false),
        ]);
        assert!(matches!(commands.as_slice(), [Control::BeginPrecise(..)]));
        assert!(
            watch
                .seeking
                .precise
                .gesture
                .as_ref()
                .is_some_and(|gesture| gesture.held.is_none())
        );
    }
}

#[cfg(test)]
#[test]
fn timeline_drags_scrub_normally_until_a_mouse_or_touch_pull_up() {
    for (touch, pull_up) in [(false, false), (false, true), (true, false), (true, true)] {
        let ctx = Context::default();
        let bounds = Rect::from_min_size(Pos2::ZERO, vec2(830.0, 440.0));
        let rail_rect = Rect::from_min_size(pos2(0.0, 98.0), vec2(bounds.width(), 22.0));
        let mut watch = Watch::new(ctx.clone(), None);
        let mut state = Playback::default();
        state.ready = true;
        state.position = 12.0;
        let end = 60.0;
        let mut commands = Vec::new();
        let mut frame = |watch: &mut Watch, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(bounds),
                    events,
                    ..Default::default()
                },
                |ui| {
                    watch.update_held_seek(ui);
                    let mut rail = ui.new_child(egui::UiBuilder::new().max_rect(rail_rect));
                    timeline(
                        &mut rail,
                        Some(|command| commands.push(command)),
                        &state,
                        end,
                        &mut watch.seeking,
                        Some(rail_rect.height()),
                        Some(rail_rect),
                    );
                },
            )
            .drop_without_applying_deltas();
        };
        let start = pos2(160.0, rail_rect.center().y);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let input = |pos, phase| egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase,
            pos,
            force: None,
        };
        let moved = |pos, phase| {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if touch {
                events.insert(0, input(pos, phase));
            }
            events
        };
        frame(&mut watch, vec![]);
        let mut press = moved(start, egui::TouchPhase::Start);
        press.push(button(start, true));
        frame(&mut watch, press);
        frame(
            &mut watch,
            moved(pos2(295.0, start.y), egui::TouchPhase::Move),
        );
        assert!(watch.seeking.precise.gesture.is_none());
        let ordinary = watch.seeking.drag.as_ref().unwrap().position;
        frame(
            &mut watch,
            moved(pos2(400.0, start.y - 23.0), egui::TouchPhase::Move),
        );
        assert!(watch.seeking.precise.gesture.is_none());
        assert!(watch.seeking.drag.as_ref().unwrap().position > ordinary);
        let y = if pull_up {
            start.y - 24.0
        } else {
            start.y + 4.0
        };
        frame(&mut watch, moved(pos2(500.0, y), egui::TouchPhase::Move));
        assert_eq!(watch.seeking.precise.gesture.is_some(), pull_up);
        if pull_up {
            let entered = watch.seeking.precise.gesture.as_ref().unwrap().position();
            frame(&mut watch, moved(pos2(560.0, 12.0), egui::TouchPhase::Move));
            assert!(watch.seeking.precise.gesture.as_ref().unwrap().position() > entered);
        }
        let final_pos = pos2(675.0, y);
        let mut release = moved(final_pos, egui::TouchPhase::End);
        release.push(button(final_pos, false));
        if touch {
            release.push(egui::Event::PointerGone);
        }
        frame(&mut watch, release);
        assert!(watch.seeking.precise.gesture.is_none());
        let (left, right) = Rail {
            rect: rail_rect,
            end,
        }
        .ends();
        let expected = f64::from((final_pos.x - left) / (right - left)) * end;
        assert!((watch.seeking.pending.unwrap().0 - expected).abs() < 0.001);
        if pull_up {
            assert!(matches!(commands.as_slice(), [Control::BeginPrecise(..)]));
        } else {
            assert!(
                matches!(commands.as_slice(), [Control::Seek(at)] if (*at - expected).abs() < 0.001)
            );
        }
    }
}

#[cfg(test)]
#[test]
fn video_surface_scrubbing_requires_a_pull_up_to_open_the_filmstrip() {
    let origin = pos2(200.0, 100.0);
    let state = Playback::default();
    let pointer = |point, down: bool| ScrubPointer {
        press: None,
        release: (!down).then_some(point),
        point: Some(point),
        down,
        cancelled: false,
    };
    for touch_scroll in [false, true] {
        let mut seeking = SeekBar {
            surface_drag: Some(SurfaceDrag {
                held: HeldScrub::new(origin, 0., 400.),
                touch_scroll,
                position: None,
            }),
            ..Default::default()
        };
        for lift in [4., 10., 23.] {
            assert!(
                seeking
                    .surface_motion(&pointer(origin - vec2(0., lift), true), &state, 60.)
                    .is_none()
            );
        }
        let command = seeking.surface_motion(&pointer(origin - vec2(0., 24.), true), &state, 60.);
        assert_eq!(
            matches!(command, Some(Control::BeginPrecise(..))),
            !touch_scroll
        );
    }
    for pull_up in [false, true] {
        let mut seeking = SeekBar {
            surface_drag: Some(SurfaceDrag {
                held: HeldScrub::new(origin, 0.0, 400.0),
                touch_scroll: false,
                position: None,
            }),
            ..Default::default()
        };
        for y in [100.0, 77.0] {
            let motion = pointer(pos2(250.0, y), true);
            assert!(seeking.surface_motion(&motion, &state, 60.0).is_none());
            assert_eq!(seeking.surface_position(), Some(37.5));
            assert!(seeking.precise.gesture.is_none());
        }
        if pull_up {
            let motion = pointer(pos2(300.0, 76.0), true);
            assert!(matches!(
                seeking.surface_motion(&motion, &state, 60.0),
                Some(Control::BeginPrecise(_, 45.0))
            ));
            assert!(seeking.precise.gesture.as_ref().unwrap().held.is_some());
        } else {
            let release = pointer(pos2(300.0, 77.0), false);
            assert!(matches!(
                seeking.surface_motion(&release, &state, 60.0),
                Some(Control::Seek(45.0))
            ));
            assert!(seeking.precise.gesture.is_none());
        }
        assert!(seeking.surface_drag.is_none());
    }
}

#[cfg(test)]
#[test]
fn mini_controls_follow_pointer_activity_in_both_viewports_and_all_themes() {
    let ctx = Context::default();
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated mini player test"));
    let mut watch = Watch::new(ctx.clone(), None);
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(420., 280.));
    let player = Rect::from_min_size(bounds.min, vec2(240., 135.));
    let actions = mini_actions(player);
    for viewport_id in [egui::ViewportId::ROOT, Watch::pip_id()] {
        for (index, style) in InterfaceTheme::ALL.into_iter().enumerate() {
            watch.restore_preferences(PlayerPreferences {
                interface_theme: style,
                auto_hide: true,
                hide_delay_ms: 400,
                ..Default::default()
            });
            let mut draw = |time, events| {
                let mut input = egui::RawInput {
                    viewport_id,
                    screen_rect: Some(bounds),
                    time: Some(time + 2. * index as f64),
                    events,
                    ..Default::default()
                };
                input.viewports.entry(viewport_id).or_default();
                let output = ctx.run_ui(input, |ui| {
                    watch.mini_content(
                        ui,
                        player.size(),
                        &mut Playback::default(),
                        false,
                        &service,
                    );
                });
                let mut visible = [false; 2];
                for shape in &output.shapes {
                    for (i, action) in actions.iter().enumerate() {
                        if action.contains_rect(shape.shape.visual_bounding_rect()) {
                            match &shape.shape {
                                egui::Shape::LineSegment { .. }
                                | egui::Shape::Mesh(_)
                                | egui::Shape::Path(_)
                                | egui::Shape::Circle(_) => visible[i] = true,
                                _ => {}
                            }
                        }
                    }
                }
                output.drop_without_applying_deltas();
                visible
            };
            assert_eq!(
                draw(0., vec![egui::Event::PointerMoved(pos2(310., 180.))]),
                [false; 2]
            );
            assert_eq!(
                draw(0.1, vec![egui::Event::PointerMoved(player.center())]),
                [true; 2]
            );
            assert_eq!(draw(1., vec![]), [false; 2]);
            assert_eq!(
                draw(
                    1.1,
                    vec![egui::Event::PointerMoved(player.center() + vec2(3., 0.))]
                ),
                [true; 2]
            );
            assert_eq!(draw(1.2, vec![egui::Event::PointerGone]), [false; 2]);
            assert_eq!(
                draw(1.3, vec![egui::Event::PointerMoved(player.center())]),
                [true; 2]
            );
        }
    }
}

#[cfg(test)]
#[test]
fn mini_overlay_targets_fit_beside_the_progress_controls() {
    let video = Rect::from_min_size(pos2(20.0, 20.0), vec2(240.0, 135.0));
    for position in [ProgressBarPosition::Top, ProgressBarPosition::Bottom] {
        let strip = control_bar::mini_timeline(video, ControlBarLayout::Classic, position);
        let [open, close] = mini_actions(video);
        for button in [open, close] {
            assert_eq!(button.top(), video.top() + 7.0);
            assert!(video.contains_rect(button));
            assert!(!strip.intersects(button));
        }
        assert!(open.center().x < close.center().x);
        assert!(!open.intersects(close));
    }
}

#[cfg(test)]
#[test]
fn glass_controller_order_and_autoplay_work_in_every_layout() {
    use ToolbarItem::{Autoplay, Captions, Fullscreen, Mini, Settings as Gear, Theater};
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated controller test"));
    let mut watch = Watch::new(ctx.clone(), None);
    watch.video = Some(crate::ui::tests::catalog_video(1));
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280., 720.));
    for style in [InterfaceTheme::LiquidGlass, InterfaceTheme::GlassOpaque] {
        for layout in ControlBarLayout::ALL {
            watch.restore_preferences(PlayerPreferences {
                interface_theme: style,
                auto_hide: false,
                ..Default::default()
            });
            watch.set_control_bar_layout(layout);
            let items = ToolbarItem::controller_items(&watch.prefs.toolbar, true, false);
            assert_eq!(items, [Autoplay, Mini, Captions, Gear, Theater, Fullscreen]);
            let frame = |watch: &mut Watch, events| {
                let mut center = Pos2::ZERO;
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let bounds = ui.available_rect_before_wrap();
                        let mut bar =
                            Bar::new(bounds, layout, ProgressBarPosition::FollowLayout, false);
                        bar.set_toolbar(&items);
                        center = pos2(
                            bar.row().right() - bar.actions_width()
                                + control_bar::BUTTON_WIDTH * 0.5,
                            bar.row().center().y,
                        );
                        watch.player_surface(
                            ui,
                            &Playback::default(),
                            &Queue::default(),
                            &service,
                            &[],
                            bounds.size(),
                        );
                    },
                )
                .drop_without_applying_deltas();
                center
            };
            let pos = frame(&mut watch, vec![]);
            for pressed in [true, false] {
                frame(
                    &mut watch,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert!(watch.prefs.autoplay, "{style:?} {layout:?}");
            let stored = serde_json::to_string(&watch.prefs).unwrap();
            assert!(
                serde_json::from_str::<PlayerPreferences>(&stored)
                    .unwrap()
                    .autoplay
            );
        }
    }
}

#[cfg(test)]
#[test]
fn precise_overlay_spans_the_picture_in_every_controller_layout() {
    let ctx = Context::default();
    theme::install(&ctx);
    let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated player test"));
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(1280., 720.));
    let state = Playback::default();
    let mut watch = Watch::new(ctx.clone(), None);
    watch.video = Some(crate::ui::tests::catalog_video(1));
    watch.seeking.precise.start(&state, 10., 60., None);
    for style in crate::player::options::InterfaceTheme::ALL {
        watch.restore_preferences(PlayerPreferences {
            interface_theme: style,
            ..Default::default()
        });
        for layout in ControlBarLayout::ALL {
            watch.set_control_bar_layout(layout);
            watch.prefs.classic_layout = layout;
            for position in ProgressBarPosition::ALL {
                watch.progress_bar_position = position;
                let (width, height) = if position == ProgressBarPosition::Top {
                    (9, 16)
                } else {
                    (16, 9)
                };
                let media = &mut watch.video.as_mut().unwrap().media;
                media.width = Some(width);
                media.height = Some(height);
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(bounds),
                        ..Default::default()
                    },
                    |ui| {
                        let size = ui.available_size();
                        let picture = video_picture(
                            ui.available_rect_before_wrap(),
                            width as f32 / height as f32,
                        );
                        watch.player_surface(ui, &state, &Queue::default(), &service, &[], size);
                        let overlay = ui
                            .memory(|m| m.area_rect(Id::new("precise_seek_overlay")))
                            .unwrap();
                        assert!((overlay.left() - picture.left()).abs() < 1.);
                        assert!((overlay.right() - picture.right()).abs() < 1.);
                    },
                )
                .drop_without_applying_deltas();
            }
        }
    }
}

#[cfg(test)]
#[test]
fn filmstrip_fills_the_visible_picture_with_more_tiles_on_wide_players() {
    let mut previous_count = 0;
    for (width, height) in [(700.0, 380.0), (1280.0, 720.0), (3840.0, 2160.0)] {
        let slot = Rect::from_min_size(pos2(25.0, 50.0), vec2(width, height));
        let picture = video_picture(slot, 16.0 / 9.0);
        let panel = PrecisePanel::new(picture, 16.0 / 9.0);
        assert!(panel.tiles > previous_count);
        previous_count = panel.tiles;
        assert!(panel.rect.height() <= 132.0);
        assert!(picture.contains_rect(panel.rect));
        assert_eq!(panel.rect.x_range(), picture.x_range());
        let middle = panel.tiles / 2;
        assert!((panel.tile(0, middle, 0.0).left() - picture.left()).abs() < 0.001);
        assert!((panel.tile(panel.tiles - 1, middle, 0.0).right() - picture.right()).abs() < 0.001);
        for offset in [-0.45, 0.0, 0.45] {
            for tile in 1..panel.tiles {
                assert_eq!(
                    panel.tile(tile - 1, middle, offset).right(),
                    panel.tile(tile, middle, offset).left()
                );
            }
            assert!(
                panel
                    .tile(middle, middle, offset)
                    .contains(panel.strip.center())
            );
        }
        assert!(panel.track.bottom() < panel.strip.top());
    }

    let fullscreen = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0));
    let portrait = video_picture(fullscreen, 9.0 / 16.0);
    let panel = PrecisePanel::new(portrait, 9.0 / 16.0);
    assert_eq!(panel.rect.x_range(), portrait.x_range());
    assert!(panel.rect.left() > fullscreen.left());
    assert!(panel.rect.right() < fullscreen.right());
}

#[cfg(test)]
#[test]
fn precise_panel_captures_strip_and_rail_motion_through_release() {
    let ctx = Context::default();
    theme::install(&ctx);
    let player = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 480.0));
    let panel = PrecisePanel::new(player, 16.0 / 9.0);
    let video = crate::ui::tests::catalog_video(3);
    let mut precise = PreciseSeek::default();
    precise.start(&Playback::default(), 20.0, 45.0, None);
    let gesture = precise.gesture.as_mut().unwrap();
    let peek = Peek::default();
    let frame = |gesture: &mut Gesture, events| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(player),
                events,
                ..Default::default()
            },
            |ui| {
                assert!(
                    panel
                        .show(ui, gesture, &video, &Playback::default(), &peek)
                        .is_none()
                );
            },
        )
        .drop_without_applying_deltas();
    };
    frame(gesture, vec![]);
    let start = panel.strip.center();
    let end = start - vec2(panel.tile_pitch() * 1.5, 0.0);
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        gesture,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    frame(
        gesture,
        vec![egui::Event::PointerMoved(end), button(end, false)],
    );

    assert_eq!(gesture.position(), 21.5);

    let rail_center = panel.track.center();
    let rail_end = rail_center + vec2(panel.rect.width(), -24.0);
    frame(
        gesture,
        vec![
            egui::Event::PointerMoved(rail_center),
            button(rail_center, true),
        ],
    );
    frame(
        gesture,
        vec![egui::Event::PointerMoved(rail_end), button(rail_end, false)],
    );
    assert_eq!(gesture.position(), 44.999);
    assert!(gesture.rail_drag.is_none());

    let filmstrip = gesture.filmstrip();
    let first = panel.tile(1, filmstrip.selected, filmstrip.offset).center();
    let expected = filmstrip.positions[1];
    frame(
        gesture,
        vec![egui::Event::PointerMoved(first), button(first, true)],
    );
    frame(gesture, vec![button(first, false)]);
    assert_eq!(gesture.position(), expected);
}

#[cfg(test)]
#[test]
fn short_clip_panel_draws_the_circular_thumb_and_subsecond_time_in_every_theme() {
    let ctx = Context::default();
    theme::install(&ctx);
    let player = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 480.0));
    let panel = PrecisePanel::new(player, 16.0 / 9.0);
    let state = Playback::default();
    let mut precise = PreciseSeek::default();
    precise.start(&state, 0.375, 2.0, None);
    let video = crate::ui::tests::catalog_video(3);
    for style in crate::player::options::InterfaceTheme::ALL {
        theme::apply_style(&ctx, style, crate::player::options::DEFAULT_GLASS_STRENGTH);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(player),
                ..Default::default()
            },
            |ui| {
                panel.show(
                    ui,
                    precise.gesture.as_mut().unwrap(),
                    &video,
                    &state,
                    &Peek::default(),
                );
            },
        );
        let target = precise_clock(0.375);
        assert!(!output.shapes.iter().any(|s| matches!(&s.shape,
        egui::Shape::Rect(rect) if rect.fill != Color32::TRANSPARENT
            && (rect.rect == panel.rect || rect.rect == panel.strip))));
        assert!(output.shapes.iter().any(|s| matches!(&s.shape,
        egui::Shape::Text(text) if text.galley.text() == target)));
        assert!(output.shapes.iter().any(|s| matches!(&s.shape,
        egui::Shape::Circle(thumb) if thumb.center.y == panel.track.center().y
            && thumb.fill == Color32::WHITE && thumb.stroke.is_empty())));
        output.drop_without_applying_deltas();
    }
}

#[cfg(test)]
#[test]
fn independent_rail_style_changes_only_the_track_in_every_theme() {
    let ctx = Context::default();
    let mut watch = Watch::new(ctx.clone(), None);
    let rect = Rect::from_min_size(pos2(20., 20.), vec2(500., 22.));
    for interface_theme in InterfaceTheme::ALL {
        let mut prefs = PlayerPreferences {
            interface_theme,
            ..Default::default()
        };
        watch.restore_preferences(prefs.clone());
        let original_layout = watch.layout();
        for style in [
            None,
            Some(InterfaceTheme::LiquidGlass),
            Some(InterfaceTheme::GlassOpaque),
            Some(InterfaceTheme::Classic),
        ] {
            prefs.progress_bar_style = style;
            let saved = serde_json::to_vec(&prefs).unwrap();
            watch.restore_preferences(serde_json::from_slice(&saved).unwrap());
            let output = ctx.run_ui(Default::default(), |ui| {
                Rail { rect, end: 60. }.paint(ui, &Playback::default(), 30., None, true, false)
            });
            let height = if style.unwrap_or(interface_theme).glass_ui() {
                4.
            } else {
                3.
            };
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Rect(r)
                if r.rect.height() == height && r.fill == Color32::WHITE))
            );
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Circle(c)
                if c.center == rect.center() && c.radius == 5.))
            );
            assert_eq!(theme::interface_theme(&ctx), interface_theme);
            assert_eq!(watch.layout(), original_layout);
            output.drop_without_applying_deltas();
        }
    }
}

#[cfg(test)]
#[test]
fn classic_progress_draws_a_white_played_segment_without_an_outline() {
    let ctx = Context::default();
    theme::apply_style(&ctx, InterfaceTheme::Classic, 62);
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let mut seek = SeekBar {
            drag: Some(RailDrag {
                rail: Id::NULL,
                origin: Pos2::ZERO,
                position: 5.0,
            }),
            ..SeekBar::default()
        };
        timeline(
            ui,
            None::<fn(Control)>,
            &Playback::default(),
            10.0,
            &mut seek,
            None,
            None,
        );
    });
    let segments: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.rect.height() == 3.0 => Some(rect),
            _ => None,
        })
        .collect();
    let [track, played] = segments.as_slice() else {
        panic!("expected the track and its played segment");
    };
    assert_eq!(track.rect.left(), played.rect.left());
    assert!(played.rect.right() < track.rect.right());
    assert!(track.stroke.is_empty() && played.stroke.is_empty());
    assert_eq!(played.fill, Color32::WHITE);
    output.drop_without_applying_deltas();
}

fn message(ui: &Ui, rect: Rect, label: &str) {
    let colors = theme::palette(ui);
    let painter = ui.painter().with_clip_rect(rect);
    let galley = painter.layout(
        label.to_owned(),
        egui::FontId::proportional(14.0),
        colors.accent,
        (rect.width() - 24.0).max(1.0),
    );
    painter.galley(rect.center() - galley.size() * 0.5, galley, colors.accent);
}

#[test]
fn checkpoints_follow_confirmed_playback_and_clear_once_at_eof() {
    let now = Instant::now();
    let mut checkpoint = Checkpoint::default();
    let mut state = Playback::default();
    state.position = 91.0;
    assert_eq!(checkpoint.update(&state, now, true), None);
    state.ready = true;
    state.confirmed_position = Some(14.0);
    assert_eq!(
        checkpoint.update(&state, now + Duration::from_secs(4), false),
        None
    );
    assert_eq!(
        checkpoint.update(&state, now + CHECKPOINT_INTERVAL, false),
        Some(Some(14.0))
    );
    state.paused = true;
    state.confirmed_position = Some(16.0);
    assert_eq!(
        checkpoint.update(&state, now + Duration::from_secs(6), false),
        Some(Some(16.0))
    );
    state.confirmed_position = Some(23.0);
    assert_eq!(
        checkpoint.update(&state, now + Duration::from_secs(7), true),
        Some(Some(23.0))
    );
    state.ended = true;
    assert_eq!(
        checkpoint.update(&state, now + Duration::from_secs(8), false),
        Some(None)
    );
    assert_eq!(
        checkpoint.update(&state, now + Duration::from_secs(9), true),
        None
    );
}
