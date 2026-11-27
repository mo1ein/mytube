mod classic_settings;
mod context_menu;
mod control_bar;
mod dialogs;
mod library;
#[cfg(target_os = "macos")]
mod macos;
mod marquee;
mod online_subtitles;
mod pan_selection;
mod player_keys;
mod player_layout;
mod player_settings;
mod player_window;
mod power;
mod precise_seek;
mod preview;
mod settings_form;
mod theme;
mod watch;

use crate::{
    files::Operation,
    model::{Appearance, CollectionTarget, Filter, LibraryView, Sort, Video, now},
    scan::Progress,
    service::{
        Action, Event, SearchPathStatus, SelectionRange, SelectionTarget, Service, Settings,
        TimelineOrigin,
    },
};
use dialogs::{Dialog, FileIntent};
use eframe::egui::{
    self, Align, Color32, Context, Frame, Id, Layout, Modifiers, Panel, ScrollArea, Sense, Stroke,
    TextEdit, Ui, vec2,
};
use library::{Browser, Thumbnails, folder_label};
use marquee::{Marquee, MarqueeSelection, SelectionMode};
use pan_selection::{PanInput, PanSelection};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use theme::{ButtonKind::*, Icon, button, icon_button, strong, text};

const ONE_DAY: i64 = 86_400_000;
const GRID_COLUMNS: [usize; 4] = [6, 4, 3, 2];
const LIST_HEIGHTS: [f32; 4] = [48.0, 63.0, 82.0, 104.0];
const SEARCH_ID: &str = "library_search";
const CATALOG_GUTTER: f32 = 14.0;
struct PendingSelection {
    id: u64,
    mode: SelectionMode,
    marquee: Option<MarqueeSelection>,
    pan: bool,
    result: Option<Result<Vec<i64>, String>>,
}

struct Subfolders {
    path: PathBuf,
    request: u64,
    result: Option<Result<Vec<PathBuf>, String>>,
}

#[derive(Clone, Copy)]
struct CatalogViewport {
    layer: egui::LayerId,
    bounds: egui::Rect,
    columns: usize,
    rows_top: f32,
    row_height: f32,
    stride: f32,
    height: f32,
    offset: f32,
}

impl CatalogViewport {
    fn row_top(self, index: usize) -> f32 {
        self.rows_top + (index / self.columns) as f32 * self.stride
    }

    fn relative_top(self, index: usize) -> f32 {
        self.row_top(index) - self.offset
    }

    fn shows(self, index: usize) -> bool {
        let top = self.relative_top(index);
        top < self.height && top + self.row_height > 0.0
    }

    fn first_visible(self) -> usize {
        ((self.offset - self.rows_top + self.stride - self.row_height) / self.stride) as usize
            * self.columns
    }

    fn reveal(self, index: usize) -> f32 {
        let top = self.row_top(index);
        self.offset.clamp(
            top + self.row_height - self.height.max(self.row_height),
            top,
        )
    }

    fn restore(self, anchor: CatalogAnchor) -> f32 {
        let top = anchor
            .top
            .clamp(1.0 - self.row_height, self.height.max(1.0) - 1.0);
        self.row_top(anchor.index) - top
    }
}

#[derive(Clone, Copy)]
struct CatalogAnchor {
    id: i64,
    index: usize,
    top: f32,
}

enum CatalogScroll {
    Reveal(i64),
    RestoreOffset(f32),
    RestoreAnchor(CatalogAnchor),
}

struct FileJob {
    label: &'static str,
    done: usize,
    total: usize,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Location {
    #[default]
    All,
    Recent,
    WatchLater,
    Collection(i64),
    Source(i64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaybackView {
    Current,
    FullPlayer,
}

struct RequestedVideo {
    key: (i64, i64),
    view: PlaybackView,
}

// Native viewport callbacks run on the UI thread. Use a weak reference here so
// the Settings window edits the same model and never owns the application.
thread_local! {
    static DESKTOP: std::cell::RefCell<std::rc::Weak<std::cell::RefCell<MyTube>>> = Default::default();
}

pub struct Desktop(std::rc::Rc<std::cell::RefCell<MyTube>>);

impl Desktop {
    pub fn new(app: MyTube) -> Self {
        let app = std::rc::Rc::new(std::cell::RefCell::new(app));
        DESKTOP.with_borrow_mut(|slot| *slot = std::rc::Rc::downgrade(&app));
        Self(app)
    }
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        self.0.borrow_mut().ui(ui, frame);
    }
    fn logic(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        self.0.borrow_mut().logic(ctx, frame);
    }
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.0.borrow().clear_color(visuals)
    }
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.0.borrow_mut().on_exit(gl);
    }
}

fn settings_window(ui: &mut Ui, _: egui::ViewportClass) {
    if let Some(app) = DESKTOP.with_borrow(std::rc::Weak::upgrade) {
        let mut app = app.borrow_mut();
        let MyTube {
            watch,
            service,
            appearance,
            ..
        } = &mut *app;
        watch.settings_window(ui, service, appearance);
    }
}

pub struct MyTube {
    #[cfg(target_os = "macos")]
    dialog_glass: macos::controller_glass::ControllerGlass,
    #[cfg(target_os = "macos")]
    breadcrumb_glass: macos::controller_glass::ControllerGlass,
    #[cfg(target_os = "macos")]
    breadcrumb_popup: Option<egui::Response>,
    #[cfg(target_os = "macos")]
    media_buttons: macos::media_buttons::MediaButtons,
    #[cfg(target_os = "macos")]
    library_glass: macos::library_glass::LibraryGlass,
    #[cfg(target_os = "macos")]
    _glass_accessibility: macos::GlassAccessibility,
    service: Service,
    settings: Settings,
    browser: Browser,
    thumbs: Thumbnails,
    watch: watch::Watch,
    ready: bool,
    validating: bool,
    startup_error: Option<String>,
    missing: Vec<&'static str>,
    location: Location,
    filter: Filter,
    filter_open: bool,
    suggestion: Option<String>,
    sidebar_open: bool,
    appearance: Appearance,
    view: LibraryView,
    last_view_wheel: Option<Instant>,
    selection: HashSet<i64>,
    selection_anchor: Option<i64>,
    range_request: Option<PendingSelection>,
    marquee: Option<Marquee>,
    pan_input: PanInput,
    pan_selection: Option<PanSelection>,
    pan_mode: bool,
    request_number: u64,
    subfolders: Option<Subfolders>,
    cursor: Option<usize>,
    pending_keyboard_selection: bool,
    pending_play: Option<(usize, PlaybackView)>,
    initial_window_applied: bool,
    aspect_lock: player_window::AspectLock,
    playback_request: Option<RequestedVideo>,
    visible_videos: Vec<(i64, i64)>,
    detail: Option<Video>,
    catalog_viewport: Option<CatalogViewport>,
    catalog_scroll: Option<CatalogScroll>,
    scroll_before_details: Option<f32>,
    detail_removal: Option<(i64, CatalogAnchor)>,
    memberships: Vec<i64>,
    dialog: Option<dialogs::Dialog>,
    file_job: Option<FileJob>,
    file_errors: Vec<(PathBuf, String)>,
    drives: Option<Vec<PathBuf>>,
    notice: Option<(String, bool, Instant)>,
    recent_since: i64,
    window_placement_deadline: Option<Instant>,
}

impl MyTube {
    pub fn new(
        ctx: &Context,
        loader: Option<crate::player::GlProc>,
        defer_window_size: bool,
        service: Service,
    ) -> Self {
        theme::install(ctx);
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        Self {
            #[cfg(target_os = "macos")]
            dialog_glass: Default::default(),
            #[cfg(target_os = "macos")]
            breadcrumb_glass: Default::default(),
            #[cfg(target_os = "macos")]
            breadcrumb_popup: None,
            #[cfg(target_os = "macos")]
            media_buttons: Default::default(),
            #[cfg(target_os = "macos")]
            library_glass: Default::default(),
            #[cfg(target_os = "macos")]
            _glass_accessibility: macos::GlassAccessibility::new(ctx),
            service,
            settings: Settings::default(),
            browser: Browser::default(),
            thumbs: Thumbnails::default(),
            watch: watch::Watch::new(ctx.clone(), loader),
            ready: false,
            validating: false,
            startup_error: None,
            missing: Vec::new(),
            location: Location::All,
            filter: Filter::default(),
            filter_open: false,
            suggestion: None,
            sidebar_open: true,
            appearance: Appearance::default(),
            view: LibraryView::default(),
            last_view_wheel: None,
            selection: HashSet::new(),
            selection_anchor: None,
            range_request: None,
            marquee: None,
            pan_input: PanInput::default(),
            pan_selection: None,
            pan_mode: false,
            request_number: 0,
            subfolders: None,
            cursor: None,
            pending_keyboard_selection: false,
            pending_play: None,
            initial_window_applied: false,
            aspect_lock: Default::default(),
            playback_request: None,
            visible_videos: Vec::new(),
            detail: None,
            catalog_viewport: None,
            catalog_scroll: None,
            scroll_before_details: None,
            detail_removal: None,
            memberships: Vec::new(),
            dialog: None,
            file_job: None,
            file_errors: Vec::new(),
            drives: None,
            notice: None,
            recent_since: now() - 7 * ONE_DAY,
            window_placement_deadline: defer_window_size
                .then(|| Instant::now() + Duration::from_secs(2)),
        }
    }

    fn placing_window(&mut self, ctx: &Context, frame: &eframe::Frame) -> bool {
        let Some(deadline) = self.window_placement_deadline else {
            return false;
        };
        let viewport = ctx.viewport_rect().size();
        let placement = egui::Vec2::from(crate::PLACEMENT_WINDOW_SIZE);
        let managed = ctx.input(|input| {
            let viewport = input.viewport();
            viewport.fullscreen == Some(true) || viewport.maximized == Some(true)
        });
        if Instant::now() >= deadline || managed || (viewport - placement).abs().max_elem() > 1.0 {
            self.window_placement_deadline = None;
            return false;
        }

        // Mutter can keep using the primary monitor's geometry scale if a large
        // Wayland window overlaps two outputs during its first placement.
        if let Some(window) = frame.winit_window()
            && let Some(monitor) = window.current_monitor()
            && (monitor.scale_factor() - window.scale_factor().ceil()).abs() < 0.01
        {
            self.window_placement_deadline = None;
            let visible = monitor
                .size()
                .to_logical::<f32>(window.scale_factor() * f64::from(ctx.zoom_factor()));
            let available = vec2(visible.width - 16.0, visible.height - 64.0);
            let size = egui::Vec2::from(crate::DEFAULT_WINDOW_SIZE)
                .min(available)
                .max(egui::Vec2::from(crate::MIN_WINDOW_SIZE));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        ctx.request_repaint_after(Duration::from_millis(32));
        true
    }

    fn play(&mut self, video: Video, view: PlaybackView) {
        self.watch.save_checkpoint(&self.service, true);
        self.pending_play = None;
        self.cursor = self.browser.position(video.id);
        self.pending_keyboard_selection = false;
        self.playback_request = Some(RequestedVideo {
            key: video.thumbnail_key(),
            view,
        });
        self.service.play(video);
    }

    fn watch_visible_videos(&mut self) {
        let visible = !self.watch.fullscreen() && (!self.watch.active() || self.watch.mini());
        let videos = self
            .browser
            .viewport()
            .filter(|_| visible)
            .filter_map(|index| self.browser.get(index))
            .chain(self.watch.video())
            .chain(self.detail.as_ref());
        if !videos
            .clone()
            .map(Video::thumbnail_key)
            .eq(self.visible_videos.iter().copied())
        {
            self.visible_videos = videos.clone().map(Video::thumbnail_key).collect();
            self.service.watch_visible(videos.cloned().collect());
        }
    }

    fn play_at(&mut self, index: usize, view: PlaybackView) {
        if self.browser.total().is_none_or(|total| index >= total) {
            return;
        }
        if let Some(video) = self.browser.get(index).cloned() {
            self.play(video, view);
        } else {
            self.playback_request = None;
            self.pending_play = Some((index, view));
            self.browser.set_target(index..index + 1);
        }
    }

    fn browse_folder(&mut self, path: impl Into<PathBuf>) {
        if !self.watch.mini() {
            self.watch.minimize();
        }
        self.location = Location::All;
        self.detail = None;
        self.filter.text.clear();
        self.filter.directory = Some(path.into());
        self.apply_filter();
    }

    fn keys_allowed(&self, ctx: &Context) -> bool {
        self.dialog.is_none()
            && !self.watch.settings_panel.open
            && !egui::Popup::is_any_open(ctx)
            && !ctx.text_edit_focused()
            && !ctx.memory(|memory| memory.has_focus(Id::new(SEARCH_ID)))
    }

    fn playback_and_clipboard_shortcuts(&mut self, ctx: &Context) {
        if !self.keys_allowed(ctx) {
            return;
        }
        self.watch.mini_shortcut(true);
        let shortcut = consume_press(ctx, egui::Key::C, Modifiers::COMMAND);
        let copy_event = ctx.input(|input| input.events.contains(&egui::Event::Copy));
        if !shortcut && !copy_event {
            return;
        }
        if ctx
            .plugin::<egui::text_selection::LabelSelectionState>()
            .lock()
            .has_selection()
        {
            if !copy_event {
                ctx.input_mut(|input| input.events.push(egui::Event::Copy));
            }
            return;
        }
        if let Some(video) = self.watch.video().filter(|_| !self.watch.mini()) {
            ctx.copy_text(crate::service::clipboard_paths([video.path.as_path()]));
        } else if self.range_request.is_none() && !self.selection.is_empty() {
            self.service
                .send(Action::CopyPaths(self.selection.iter().copied().collect()));
        }
    }

    fn directory(&self) -> Option<&Path> {
        self.filter
            .directory
            .as_deref()
            .or_else(|| match self.location {
                Location::Source(id) => self
                    .settings
                    .sources
                    .iter()
                    .find(|source| source.id == id)
                    .map(|source| source.path.as_path()),
                _ => None,
            })
    }

    fn choose(&mut self, location: Location) {
        if !self.watch.mini() {
            self.watch.minimize();
        }
        if self.location != location || self.filter.directory.is_some() || self.filter.narrowing() {
            self.location = location;
            self.detail = None;
            self.filter_open = false;
            self.filter = Filter {
                sort: self.filter.sort,
                ..Filter::default()
            };
            self.apply_filter();
        }
    }

    fn apply_filter(&mut self) {
        self.suggestion = None;
        self.subfolders = None;
        self.pending_play = None;
        self.playback_request = None;
        self.clear_selection();
        self.catalog_viewport = None;
        self.catalog_scroll = None;
        self.scroll_before_details = None;
        if let Some((removed, _)) = self.detail_removal.take()
            && self
                .detail
                .as_ref()
                .is_some_and(|video| video.id == removed)
        {
            self.detail = None;
            self.memberships.clear();
        }
        let mut filter = self.filter.clone();
        match self.location {
            Location::All => {}
            Location::Recent => {
                filter.added_since = Some(
                    filter
                        .added_since
                        .unwrap_or(self.recent_since)
                        .max(self.recent_since),
                );
            }
            Location::WatchLater => filter.watch_later = true,
            Location::Collection(id) => filter.collection = Some(id),
            Location::Source(id) => filter.source = Some(id),
        }
        self.browser.set_filter(filter);
    }

    fn inspect(&mut self, video: Video) {
        if self.detail.is_none() {
            self.scroll_before_details = self.catalog_viewport.map(|viewport| viewport.offset);
        }
        if self.detail.as_ref().map(|v| v.id) != Some(video.id) {
            self.memberships.clear();
        }
        self.service.send(Action::Inspect(video.id));
        self.catalog_scroll = Some(CatalogScroll::Reveal(video.id));
        self.detail = Some(video);
    }

    fn catalog_anchor(&self, detail: i64, removed: &[i64]) -> Option<CatalogAnchor> {
        let viewport = self.catalog_viewport?;
        let origin = self
            .browser
            .position(detail)
            .filter(|&index| viewport.shows(index))
            .unwrap_or_else(|| viewport.first_visible());
        let (index, id) = self.browser.neighbor(origin, removed)?;
        let top = viewport.relative_top(if viewport.shows(index) { index } else { origin });
        Some(CatalogAnchor { id, index, top })
    }

    fn close_details(&mut self, anchor: Option<CatalogAnchor>) {
        self.detail = None;
        self.memberships.clear();
        let offset = self.scroll_before_details.take();
        self.catalog_scroll = anchor
            .map(CatalogScroll::RestoreAnchor)
            .or_else(|| offset.map(CatalogScroll::RestoreOffset));
    }

    fn begin_detail_removal(&mut self, removed: &[i64]) -> bool {
        let Some(id) = self.detail.as_ref().map(|video| video.id) else {
            return false;
        };
        if !removed.contains(&id) {
            return false;
        }
        if let Some(anchor) = self.catalog_anchor(id, removed) {
            // Keep the old layout until the refreshed page and anchor position arrive together.
            self.detail_removal = Some((id, anchor));
            self.browser.refresh_now();
        } else {
            self.close_details(None);
            self.browser.remove_missing(removed);
        }
        true
    }

    fn finish_detail_removal(&mut self, position: Option<usize>) {
        if let Some((id, mut anchor)) = self.detail_removal.take()
            && self.detail.as_ref().is_some_and(|video| video.id == id)
        {
            anchor.index = position.unwrap_or(anchor.index);
            self.close_details(Some(anchor));
        }
    }

    fn select_video(&mut self, id: i64, modifiers: Modifiers) {
        let toggle = modifiers.ctrl || modifiers.command;
        self.range_request = None;
        if modifiers.shift
            && let Some(anchor) = self.selection_anchor
        {
            self.request_number += 1;
            let mode = if toggle {
                SelectionMode::Add
            } else {
                SelectionMode::Replace
            };
            if let Some(ids) = self.browser.loaded_range(anchor, id) {
                mode.apply(&mut self.selection, ids);
                return;
            }
            self.range_request = Some(PendingSelection {
                id: self.request_number,
                mode,
                marquee: None,
                pan: false,
                result: None,
            });
            self.service.select_range(SelectionRange {
                id: self.request_number,
                filter: self.browser.filter().clone(),
                target: SelectionTarget::Between(anchor, id),
            });
            return;
        }
        self.selection_anchor = Some(id);
        if !toggle {
            self.selection.clear();
        }
        if !toggle || !self.selection.remove(&id) {
            self.selection.insert(id);
        }
    }

    fn cancel_catalog_gestures(&mut self) {
        self.marquee = None;
        self.pan_input.clear();
        self.pan_selection = None;
        self.pan_mode = false;
        if self
            .range_request
            .as_ref()
            .is_some_and(|pending| pending.marquee.is_some() || pending.pan)
        {
            self.range_request = None;
        }
    }

    fn apply_selection_range(&mut self, result: Result<Vec<i64>, String>) {
        let Some(pending) = self.range_request.take() else {
            return;
        };
        match result {
            Ok(ids) => {
                if let Some(marquee) = pending.marquee {
                    self.selection = marquee.base;
                    if pending.mode == SelectionMode::Replace {
                        self.selection_anchor = None;
                        self.cursor = None;
                        self.pending_keyboard_selection = false;
                    }
                }
                pending.mode.apply(&mut self.selection, ids);
            }
            Err(error) => self.status(error, true),
        }
    }

    fn clear_selection(&mut self) {
        self.cancel_catalog_gestures();
        self.selection.clear();
        self.selection_anchor = None;
        self.range_request = None;
        self.cursor = None;
        self.pending_keyboard_selection = false;
    }

    fn can_delete_selection(&self) -> bool {
        !self.selection.is_empty()
            && self.range_request.is_none()
            && self.file_job.is_none()
            && self.dialog.is_none()
    }

    fn delete_selection(&mut self) {
        self.open_file_dialog(
            FileIntent::Delete,
            self.selection.iter().copied().collect(),
            None,
        );
    }

    fn open_file_dialog(&mut self, intent: FileIntent, ids: Vec<i64>, video: Option<Video>) {
        if ids.len() == 1
            && let Some(video) = video
        {
            self.dialog = Some(intent.dialog(vec![video]));
        } else {
            self.request_number += 1;
            let count = ids.len();
            if self
                .service
                .send(Action::SelectFiles(self.request_number, ids))
            {
                self.dialog = Some(Dialog::ResolveFiles(self.request_number, intent, count));
            }
        }
    }

    fn start_file_job(&mut self, videos: &[Video], operation: Operation) -> bool {
        let label = operation.progress_label();
        if self.file_job.is_some()
            || videos.is_empty()
            || !self
                .service
                .send(Action::ManageFiles(videos.to_vec(), operation))
        {
            return false;
        }
        self.file_errors.clear();
        self.file_job = Some(FileJob {
            label,
            done: 0,
            total: videos.len(),
        });
        true
    }

    fn status(&mut self, message: String, error: bool) {
        self.notice = Some((
            message,
            error,
            Instant::now() + Duration::from_secs(if error { 14 } else { 7 }),
        ));
    }

    fn events(&mut self, ctx: &Context) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.cancel_catalog_gestures();
        }
        while let Ok(event) = self.service.events.try_recv() {
            match event {
                Event::Ready(settings, missing, page) => {
                    self.watch.restore_preferences(settings.player.clone());
                    self.view = settings.view;
                    self.appearance = settings.appearance;
                    self.watch
                        .set_control_bar_layout(settings.control_bar_layout);
                    self.watch
                        .set_progress_bar_position(settings.progress_bar_position);
                    self.settings = settings;
                    self.missing = missing;
                    self.ready = true;
                    self.apply_filter();
                    self.browser.loaded(page);
                }
                Event::StartupFailed(error) => self.startup_error = Some(error),
                Event::History(history) => {
                    if history.is_empty() {
                        self.status("Playback history is empty".into(), false);
                    }
                    self.watch.settings_panel.history = history;
                }
                Event::Settings(settings) => {
                    self.settings = settings;
                    if let Location::Source(id) = self.location
                        && !self.settings.sources.iter().any(|source| source.id == id)
                    {
                        self.choose(Location::All);
                    }
                }
                Event::WatchLater(id, saved) => {
                    self.status(
                        if saved {
                            "Saved to Watch Later"
                        } else {
                            "Removed from Watch Later"
                        }
                        .into(),
                        false,
                    );
                    if saved {
                        self.settings.watch_later.insert(id);
                    } else {
                        self.settings.watch_later.remove(&id);
                    }
                    self.browser.watch_later_changed();
                }
                Event::Changed => self.browser.changed(),
                Event::VideosMissing(ids) => {
                    self.cancel_catalog_gestures();
                    if !self.begin_detail_removal(&ids) {
                        self.browser.remove_missing(&ids);
                    }
                    for id in &ids {
                        self.selection.remove(id);
                    }
                    if self.selection_anchor.is_some_and(|id| ids.contains(&id)) {
                        self.selection_anchor = None;
                    }
                    if self
                        .watch
                        .video()
                        .is_some_and(|video| ids.contains(&video.id))
                    {
                        self.watch.close(&self.service);
                    }
                    if self
                        .playback_request
                        .as_ref()
                        .is_some_and(|request| ids.contains(&request.key.0))
                    {
                        self.playback_request = None;
                    }
                    self.pending_play = None;
                }
                Event::PlaybackReady(video, position) => {
                    if let Some(request) = self
                        .playback_request
                        .take_if(|r| r.key == video.thumbnail_key())
                    {
                        if request.view == PlaybackView::FullPlayer {
                            self.watch.restore();
                        }
                        self.watch.open(*video, position, &self.service);
                    }
                }
                Event::PlaybackUnavailable(key) => {
                    if self.playback_request.take_if(|r| r.key == key).is_some() {
                        self.status("This video is currently unavailable.".into(), true);
                    }
                }
                Event::Page {
                    request,
                    index,
                    total,
                    rows,
                    anchor_position,
                } => {
                    let refresh = self.detail_removal.is_some() && total.is_some();
                    // Paging through the playlist must keep the playing item's
                    // catalog position available for previous, next and autoplay.
                    if let Some(video) = self.watch.video() {
                        self.browser.keep_loaded(video.id);
                    }
                    if self.browser.applied(request, index, total, rows, refresh) && refresh {
                        self.finish_detail_removal(anchor_position);
                    }
                }
                Event::SearchPath(request, status) => self.browser.set_path_status(request, status),
                Event::Validating(active) => self.validating = active,
                Event::PageFailed(request, error) => {
                    if self.browser.fail(request) {
                        self.finish_detail_removal(None);
                        self.status(error, true);
                    }
                }
                Event::Detail(id, video, members) => {
                    if self.detail.as_ref().is_some_and(|detail| detail.id == id) {
                        if let Some(video) = video {
                            self.detail = Some(*video);
                            self.memberships = members;
                        } else if self.detail_removal.is_none() {
                            self.close_details(None);
                        }
                    }
                }
                Event::Thumbnail(key, image) => self.thumbs.load(ctx, key, image),
                Event::Preview(key, segment, image) => {
                    self.thumbs.hover.load(ctx, key, segment, image)
                }
                Event::Timeline(origin, key, image, exact) => match origin {
                    TimelineOrigin::Player => self.watch.load_preview(key, image, exact),
                    TimelineOrigin::Precise(sequence) => {
                        self.watch.load_precise(sequence, key, image)
                    }
                    TimelineOrigin::Library => {
                        self.thumbs.hover.load_timeline(ctx, key, image, exact)
                    }
                },
                Event::TimelinePrefetched(origin, key, image) => match origin {
                    TimelineOrigin::Player => self.watch.load_prefetched(key, image),
                    TimelineOrigin::Library => self.thumbs.hover.load_prefetched(ctx, key, image),
                    TimelineOrigin::Precise(_) => {}
                },
                Event::Drives(drives) => self.drives = Some(drives),
                Event::Subfolders(request, folders) => {
                    if let Some(menu) = self
                        .subfolders
                        .as_mut()
                        .filter(|menu| menu.request == request)
                    {
                        menu.result = Some(folders);
                    }
                }
                Event::CopiedPaths(Ok(paths)) => ctx.copy_text(paths),
                Event::CopiedPaths(Err(error)) => self.status(error, true),
                Event::Picked(choice) => {
                    if let Some(dialogs::Dialog::Path(path)) = &mut self.dialog {
                        path.complete_pick(choice);
                    }
                }
                Event::Message(message, error) => self.status(message, error),
                Event::SelectionRange(request, result) => {
                    if let Some(pending) = self.range_request.as_mut().filter(|p| p.id == request) {
                        if pending.marquee.is_some() || pending.pan {
                            // Let this frame handle Escape and layout changes before applying the result.
                            pending.result = Some(result);
                        } else {
                            self.apply_selection_range(result);
                        }
                    }
                }
                Event::SelectedFiles(request, result) => {
                    if let Some(Dialog::ResolveFiles(id, intent, _)) = self.dialog
                        && id == request
                    {
                        self.dialog = None;
                        match result {
                            Ok(videos) => self.dialog = Some(intent.dialog(videos)),
                            Err(error) => self.status(error, true),
                        }
                    }
                }
                Event::FileProgress(done) => {
                    if let Some(job) = &mut self.file_job {
                        job.done = done;
                    }
                }
                Event::FilesDone(report) => {
                    self.file_job = None;
                    if !report.keep_selection {
                        for id in &report.succeeded {
                            self.selection.remove(id);
                            if self.selection_anchor == Some(*id) {
                                self.selection_anchor = None;
                            }
                        }
                        if self
                            .watch
                            .video()
                            .is_some_and(|video| report.succeeded.contains(&video.id))
                        {
                            self.watch.close(&self.service);
                        }
                    }
                    if !(report.deleted && self.begin_detail_removal(&report.succeeded))
                        && let Some(video) = &self.detail
                    {
                        self.service.send(Action::Inspect(video.id));
                    }
                    let completed = report.succeeded.len();
                    let noun = if completed == 1 { "video" } else { "videos" };
                    let summary = format!("{completed} {noun} {}", report.label);
                    if report.failed.is_empty() {
                        self.status(summary, false);
                    } else {
                        self.file_errors = report.failed;
                        self.status(
                            format!("{summary} · {} failed", self.file_errors.len()),
                            true,
                        );
                        if self.dialog.is_none() {
                            self.dialog = Some(Dialog::FileErrors);
                        }
                    }
                }
            }
        }
        if let Some((index, view)) = self.pending_play {
            if let Some(video) = self.browser.get(index).cloned() {
                self.play(video, view);
            } else if self.browser.failed()
                || self.browser.total().is_some_and(|total| index >= total)
            {
                self.pending_play = None;
            } else {
                self.browser.set_target(index..index + 1);
            }
        }
        if self.ready
            && self.browser.update(
                ctx,
                &self.service,
                self.detail_removal.map(|(_, anchor)| anchor.id),
            )
            && self.detail_removal.is_none()
            && let Some(video) = &self.detail
        {
            self.service.send(Action::Inspect(video.id));
        }
        if let Some((_, _, until)) = &self.notice {
            if *until <= Instant::now() {
                self.notice = None;
            } else {
                ctx.request_repaint_after(until.saturating_duration_since(Instant::now()));
            }
        }
    }

    fn title(&self) -> String {
        match self.location {
            Location::All => self
                .directory()
                .map_or_else(|| "All videos".into(), folder_label),
            Location::Recent => "Recently added".into(),
            Location::WatchLater => "Watch Later".into(),
            Location::Collection(id) => self
                .settings
                .collections
                .iter()
                .find(|item| item.id == id)
                .map_or_else(|| "Collection".into(), |item| item.name.clone()),
            Location::Source(id) => self
                .settings
                .sources
                .iter()
                .find(|item| item.id == id)
                .map_or_else(|| "Source".into(), |item| folder_label(&item.path)),
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let colors = theme::palette(ui);
        let width = if !self.sidebar_open {
            52.0
        } else if ui.available_width() >= 940.0 {
            220.0
        } else {
            170.0
        };
        Panel::left("library_navigation")
            .show_separator_line(false)
            .exact_size(width)
            .resizable(false)
            .frame(theme::sidebar_frame(
                if self.sidebar_open { 12 } else { 8 },
                0,
            ))
            .show(ui, |ui| {
                window_handle(
                    ui,
                    "sidebar_top_edge",
                    egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 12.0)),
                );
                let expanded = self.sidebar_open;
                self.sidebar_footer(ui);
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let size = ui.available_width().min(34.);
                    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::hover());
                    ui.painter().rect_filled(rect, 8, colors.accent);
                    Icon::Film.paint(ui.painter(), rect.center(), 22., colors.on_primary);
                    if expanded {
                        ui.label(strong("MyTube", 21., colors.text));
                    }
                });
                ui.add_space(14.);
                if expanded {
                    section(ui, "LIBRARY");
                }
                ScrollArea::vertical()
                    .id_salt("sidebar-list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if theme::nav(
                            ui,
                            "All videos",
                            Icon::Grid,
                            self.location == Location::All && self.directory().is_none(),
                            !expanded,
                        )
                        .on_hover_text("All videos")
                        .clicked()
                        {
                            self.choose(Location::All);
                        }
                        if theme::nav(
                            ui,
                            "Recently added",
                            Icon::Calendar,
                            self.location == Location::Recent,
                            !expanded,
                        )
                        .on_hover_text("Recently added")
                        .clicked()
                        {
                            self.choose(Location::Recent);
                        }
                        if theme::nav(
                            ui,
                            "Watch Later",
                            Icon::Clock,
                            self.location == Location::WatchLater,
                            !expanded,
                        )
                        .on_hover_text("Watch Later")
                        .clicked()
                        {
                            self.choose(Location::WatchLater);
                        }
                        let create = if expanded {
                            ui.add_space(14.0);
                            ui.horizontal(|ui| {
                                section(ui, "COLLECTIONS");
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_enabled_ui(self.ready, |ui| {
                                        icon_button(ui, "New collection", Icon::Plus, Quiet)
                                    })
                                    .inner
                                    .clicked()
                                })
                                .inner
                            })
                            .inner
                        } else {
                            ui.separator();
                            ui.add_enabled_ui(self.ready, |ui| {
                                theme::nav(ui, "New collection", Icon::Plus, false, true)
                                    .on_hover_text("New collection")
                            })
                            .inner
                            .clicked()
                        };
                        if create {
                            self.dialog =
                                Some(dialogs::Dialog::Collection(String::new(), Vec::new()));
                        }

                        let mut location = None;
                        for collection in &self.settings.collections {
                            let target = Location::Collection(collection.id);
                            ui.push_id(collection.id, |ui| {
                                if theme::nav(
                                    ui,
                                    &collection.name,
                                    Icon::Collection,
                                    self.location == target,
                                    !expanded,
                                )
                                .on_hover_text(&collection.name)
                                .clicked()
                                {
                                    location = Some(target);
                                }
                            });
                        }
                        if expanded && self.settings.collections.is_empty() {
                            ui.label(text("   No collections yet", 12.0, colors.muted));
                        }
                        if expanded {
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                section(ui, "SOURCES");
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled_ui(self.ready, |ui| {
                                            icon_button(ui, "Manage sources", Icon::Plus, Quiet)
                                        })
                                        .inner
                                        .clicked()
                                    {
                                        self.dialog = Some(dialogs::Dialog::Sources);
                                    }
                                });
                            });
                        } else if !self.settings.sources.is_empty() {
                            ui.separator();
                        }
                        for source in &self.settings.sources {
                            let target = Location::Source(source.id);
                            ui.push_id(("source", source.id), |ui| {
                                let label = folder_label(&source.path);
                                if theme::nav(
                                    ui,
                                    &label,
                                    Icon::Folder,
                                    self.location == target,
                                    !expanded,
                                )
                                .on_hover_text(source.path.to_string_lossy())
                                .clicked()
                                {
                                    location = Some(target);
                                }
                            });
                        }
                        if expanded && self.settings.sources.is_empty() {
                            ui.label(text("   No sources added", 12.0, colors.muted));
                        }
                        if let Some(location) = location {
                            self.choose(location);
                        }
                    });
            });
    }

    fn sidebar_footer(&mut self, ui: &mut Ui) {
        Panel::bottom("source_settings")
            .exact_size(152.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE)
            .show(ui, |ui| {
                ui.separator();
                if ui
                    .add_enabled_ui(self.ready, |ui| {
                        theme::nav(ui, "Manage sources", Icon::Disk, false, !self.sidebar_open)
                            .on_hover_text("Manage sources")
                    })
                    .inner
                    .clicked()
                {
                    self.dialog = Some(dialogs::Dialog::Sources);
                }
                if ui
                    .add_enabled_ui(self.ready, |ui| {
                        theme::nav(
                            ui,
                            "Settings",
                            Icon::Filter,
                            self.watch.settings_panel.open,
                            !self.sidebar_open,
                        )
                        .on_hover_text("Open settings")
                    })
                    .inner
                    .clicked()
                {
                    self.watch.toggle_settings();
                }
                let label = if self.sidebar_open {
                    "Collapse sidebar"
                } else {
                    "Expand sidebar"
                };
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 34.0),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        if icon_button(ui, label, Icon::Sidebar, Quiet).clicked() {
                            self.sidebar_open = !self.sidebar_open;
                        }
                    },
                );
            });
    }

    fn header(&mut self, ui: &mut Ui) {
        let playing = self.watch.active() && !self.watch.mini();
        Panel::top("topbar")
            .exact_size(58.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::layer(Color32::TRANSPARENT, 24, 0))
            .show(ui, |ui| {
                let edge =
                    egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 12.0));
                window_handle(ui, "top_edge", edge);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if playing && button(ui, "Back to library", Some(Icon::Back), Quiet).clicked() {
                        self.watch.minimize();
                    }
                    let available = ui.available_width();
                    let width = (available - 255.0).clamp(180.0, 610.0);
                    let leading = ((available - width - 225.0) * 0.5).max(0.0);
                    if leading > 0.0 {
                        let (_, gap) = ui.allocate_space(vec2(leading, 36.0));
                        window_handle(ui, "search_leading_gap", gap);
                    }
                    self.search_field(ui, width);
                    let drag_width = (ui.available_width() - 211.0).max(0.0);
                    let (_, gap) = ui.allocate_space(vec2(drag_width, 36.0));
                    window_handle(ui, "header_gap", gap);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(self.ready, |ui| {
                            if catalog_button(
                                &self.watch,
                                ui,
                                "add_source",
                                "Add source",
                                110.,
                                false,
                                Some(Icon::Plus),
                            )
                            .clicked()
                            {
                                self.dialog = Some(dialogs::Dialog::Sources);
                            }
                            self.theme_button(ui);
                            if playing
                                && icon_button(
                                    ui,
                                    "Player settings and tools",
                                    Icon::Settings,
                                    if self.watch.settings_panel.open {
                                        Selected
                                    } else {
                                        Quiet
                                    },
                                )
                                .clicked()
                            {
                                self.watch.toggle_settings();
                            }
                        });
                    });
                });
            });
    }

    fn theme_button(&mut self, ui: &mut Ui) {
        let icon = match self.appearance {
            Appearance::System => Icon::System,
            Appearance::Light => Icon::Sun,
            Appearance::Dark => Icon::Moon,
        };
        let next = self.appearance.next();
        let label = format!(
            "Theme is {}. Switch to {}.",
            self.appearance.label(),
            next.label()
        );
        if icon_button(ui, &label, icon, Quiet).clicked() {
            self.appearance = next;
            theme::apply(ui.ctx(), next);
            self.service.send(Action::SaveAppearance(next));
        }
    }

    fn search_field(&mut self, ui: &mut Ui, width: f32) {
        let colors = theme::palette(ui);
        use egui::Key;

        let id = Id::new(SEARCH_ID);
        let shortcuts = self.dialog.is_none() && !egui::Popup::is_any_open(ui.ctx());
        let slash_shortcut = !ui.ctx().text_edit_focused();
        if shortcuts
            && ui.input_mut(|input| {
                let find = input.consume_key(Modifiers::COMMAND, Key::F);
                if !slash_shortcut {
                    return find;
                }
                let count = input.events.len();
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Text(text) if text == "/"));
                find || input.events.len() != count
            })
        {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        let focused = ui.memory(|memory| memory.has_focus(id));
        let mut suggestions = if focused && self.dialog.is_none() {
            self.browser.suggestions()
        } else {
            Vec::new()
        };
        let consume = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
        let mut submit = false;
        let mut changed = false;
        if focused && shortcuts {
            if consume(Key::Escape) {
                ui.memory_mut(|memory| memory.surrender_focus(id));
                self.suggestion = None;
                suggestions.clear();
            } else if consume(Key::Enter) {
                if let Some(suggestion) = self
                    .suggestion
                    .take()
                    .filter(|value| suggestions.contains(value))
                {
                    self.filter.text = suggestion;
                    changed = true;
                }
                submit = true;
            } else if !suggestions.is_empty() {
                let delta = if consume(Key::ArrowDown) {
                    1
                } else if consume(Key::ArrowUp) {
                    -1
                } else {
                    0
                };
                if delta != 0 {
                    let current = self
                        .suggestion
                        .as_ref()
                        .and_then(|value| suggestions.iter().position(|item| item == value));
                    let count = suggestions.len() as isize;
                    let next = current.map_or(if delta > 0 { 0 } else { count - 1 }, |index| {
                        (index as isize + delta).rem_euclid(count)
                    });
                    self.suggestion = Some(suggestions[next as usize].clone());
                }
            }
        }
        let box_frame = Frame::new()
            .fill(theme::control_fill(ui, colors.well))
            .stroke(Stroke::new(
                1.0,
                if focused { colors.muted } else { colors.line },
            ))
            .inner_margin(egui::Margin {
                left: 15,
                right: 2,
                top: 2,
                bottom: 2,
            })
            .corner_radius(22)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let input_width = width - 104.0;
                    changed |= ui
                        .add(
                            TextEdit::singleline(&mut self.filter.text)
                                .id(id)
                                .char_limit(240)
                                .font(egui::FontId::proportional(14.0))
                                .hint_text(text("Search videos", 14.0, colors.muted))
                                .desired_width(input_width)
                                .min_size(vec2(input_width, 34.0))
                                .vertical_align(Align::Center)
                                .frame(Frame::NONE),
                        )
                        .on_hover_text("Search by name or full path")
                        .changed();
                    if self.filter.text.is_empty() {
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                        let keycap = egui::Rect::from_center_size(rect.center(), vec2(19.0, 21.0));
                        ui.painter().rect(
                            keycap,
                            4,
                            colors.well,
                            Stroke::new(1.0, colors.line),
                            egui::StrokeKind::Inside,
                        );
                        theme::centered_text(
                            ui.painter(),
                            keycap.center(),
                            "/",
                            12.0,
                            colors.muted,
                        );
                        response.on_hover_text("Press / to focus search");
                    } else if icon_button(ui, "Clear search", Icon::Close, Quiet).clicked() {
                        self.filter.text.clear();
                        ui.memory_mut(|memory| memory.request_focus(id));
                        changed = true;
                    }
                    submit |= icon_button(ui, "Search", Icon::Search, Secondary).clicked();
                });
            })
            .response;
        if !changed && !submit && !suggestions.is_empty() {
            let width = box_frame.rect.width() - 16.0;
            if let Some(title) = egui::Popup::from_response(&box_frame)
                .gap(4.0)
                .width(width)
                .layout(Layout::top_down_justified(Align::Min))
                .show(|ui| {
                    ui.set_width(width);
                    suggestions.into_iter().find(|title| {
                        ui.add(
                            egui::Button::selectable(
                                self.suggestion.as_ref() == Some(title),
                                text(title, 13.0, colors.text),
                            )
                            .truncate(),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    })
                })
                .and_then(|popup| popup.inner)
            {
                self.filter.text = title;
                changed = true;
                submit = true;
            }
        }
        if submit {
            ui.memory_mut(|memory| memory.surrender_focus(id));
            self.suggestion = None;
        }
        if changed {
            self.detail = None;
            self.watch.minimize();
            self.apply_filter();
        } else if submit && !self.filter.text.trim().is_empty() {
            if self.detail.is_some() {
                self.close_details(None);
            }
            self.watch.minimize();
        }
    }

    fn breadcrumb_path(&self) -> Option<PathBuf> {
        let path = self.directory().map(Path::to_path_buf);
        let typed = Path::new(self.filter.text.trim());
        path.or_else(|| {
            typed.is_absolute().then(|| {
                if crate::model::video_ext(typed).is_some() || self.browser.path_status().is_some()
                {
                    typed.parent().unwrap_or(typed).to_path_buf()
                } else {
                    typed.to_path_buf()
                }
            })
        })
    }

    fn breadcrumbs(&mut self, ui: &mut Ui, path: &Path) {
        let colors = theme::palette(ui);
        let mut go = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            theme::style_form(ui);
            if breadcrumb(ui, "Library", 80.0).clicked() {
                go = Some(None);
            }
            let menu_width = 38.0;
            let separator_width = 16.0;
            let remaining = (ui.available_width() - menu_width - separator_width).max(32.0);
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty());
            let show_parent = parent.is_some() && remaining >= 220.0;
            if let Some(parent) = parent
                && (!show_parent || parent.parent().is_some())
            {
                breadcrumb_separator(ui);
                let button = icon_button(ui, "Parent folders", Icon::More, Quiet);
                let popup = self.breadcrumb_menu(&button, "Parent folders", |_, ui| {
                    let ancestors = parent
                        .ancestors()
                        .skip(usize::from(show_parent))
                        .map(Path::to_path_buf)
                        .collect::<Vec<_>>();
                    folder_choices(ui, &ancestors)
                });
                if let Some(folder) = popup.and_then(|popup| popup.inner) {
                    go = Some(Some(folder));
                }
            }
            let remaining = (ui.available_width() - menu_width - separator_width).max(32.);
            let current_width = if let Some(parent) = parent.filter(|_| show_parent) {
                breadcrumb_separator(ui);
                let response = breadcrumb(ui, folder_label(parent), (remaining * 0.35).min(180.0))
                    .on_hover_text(parent.to_string_lossy());
                if response.clicked() {
                    go = Some(Some(parent.to_path_buf()));
                }
                (remaining - response.rect.width() - separator_width).max(32.0)
            } else {
                remaining
            };
            breadcrumb_separator(ui);
            ui.scope(|ui| {
                ui.set_max_width(current_width);
                ui.add(egui::Label::new(strong(folder_label(path), 14.0, colors.text)).truncate())
            })
            .inner
            .on_hover_text(path.to_string_lossy());
            if let Some(folder) = self.folder_menu(ui, path) {
                go = Some(Some(folder));
            }
        });
        match go {
            Some(Some(path)) => self.browse_folder(path),
            Some(None) => self.choose(Location::All),
            None => {}
        }
    }

    fn breadcrumb_menu(
        &mut self,
        button: &egui::Response,
        title: &str,
        contents: impl FnOnce(&mut Self, &mut Ui) -> Option<PathBuf>,
    ) -> Option<egui::InnerResponse<Option<PathBuf>>> {
        #[cfg(target_os = "macos")]
        let native = self.dialog.is_none()
            && !button.ctx.embed_viewports()
            && self.watch.menu_host.view().is_some()
            && theme::menu_material(&button.ctx) == control_bar::DisplayMaterial::Glass;
        #[cfg(not(target_os = "macos"))]
        let native = false;
        let popup = theme::folder_popup(button, native, title, |ui| {
            let folder = contents(self, ui);
            if folder.is_some() {
                ui.close();
            }
            folder
        });
        #[cfg(target_os = "macos")]
        if native && let Some(popup) = &popup {
            self.breadcrumb_popup = Some(popup.response.clone());
        }
        popup
    }

    fn folder_menu(&mut self, ui: &mut Ui, path: &Path) -> Option<PathBuf> {
        ui.push_id(path, |ui| {
            let button = icon_button(ui, "Subfolders", Icon::ChevronDown, Quiet);
            let popup = self.breadcrumb_menu(&button, "Subfolders", |app, ui| {
                if app.subfolders.as_ref().is_none_or(|menu| menu.path != path) {
                    app.request_number += 1;
                    app.service
                        .list_subfolders(app.request_number, path.to_path_buf());
                    app.subfolders = Some(Subfolders {
                        path: path.to_path_buf(),
                        request: app.request_number,
                        result: None,
                    });
                }
                match app
                    .subfolders
                    .as_ref()
                    .and_then(|menu| menu.result.as_ref())
                {
                    Some(Ok(folders)) if folders.is_empty() => {
                        ui.label("No subfolders");
                    }
                    Some(Ok(folders)) => return folder_choices(ui, folders),
                    Some(Err(error)) => {
                        ui.add(egui::Label::new("Could not load subfolders").wrap())
                            .on_hover_text(error);
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading subfolders…");
                        });
                    }
                }
                None
            });
            let open = popup.is_some();
            let selected = popup.and_then(|popup| popup.inner);
            if !open || selected.is_some() {
                self.subfolders = None;
            }
            selected
        })
        .inner
    }

    fn footer(&mut self, ui: &mut Ui, progress: &Progress) {
        let colors = theme::palette(ui);
        Panel::bottom("main_status_bar")
            .resizable(false)
            .exact_size(39.0)
            .show_separator_line(false)
            .frame(theme::layer(Color32::TRANSPARENT, 20, 1))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 29.0), Sense::hover());
                    if self.startup_error.is_some() || progress.skipped > 0 {
                        Icon::Info.paint(ui.painter(), rect.center(), 11.0, colors.text);
                    } else if progress.running || self.validating {
                        ui.painter()
                            .circle_filled(rect.center(), 3.0, colors.accent);
                    } else {
                        ui.painter().circle_stroke(
                            rect.center(),
                            3.0,
                            Stroke::new(1.0, colors.muted),
                        );
                    }
                    let message = if let Some(job) = &self.file_job {
                        format!(
                            "{} {} of {}",
                            job.label,
                            grouped(job.done),
                            grouped(job.total)
                        )
                    } else if progress.running {
                        format!(
                            "{}  ·  source {}/{}  ·  {} files to check",
                            if progress.paused {
                                "Scan paused"
                            } else {
                                "Scanning"
                            },
                            progress.source,
                            progress.sources,
                            grouped(progress.found as usize)
                        )
                    } else if self.validating {
                        "Verifying video files…".into()
                    } else if progress.skipped > 0 {
                        format!(
                            "Scan complete  ·  {} {} skipped",
                            grouped(progress.skipped as usize),
                            if progress.skipped == 1 {
                                "path"
                            } else {
                                "paths"
                            }
                        )
                    } else if self.ready {
                        "Local library".into()
                    } else if self.startup_error.is_some() {
                        "Library unavailable".into()
                    } else {
                        "Opening library…".into()
                    };
                    if !progress.running
                        && !self.validating
                        && self.file_job.is_none()
                        && progress.skipped > 0
                    {
                        let report = ui
                            .button(text(message, 12.0, colors.muted))
                            .on_hover_text("Show skipped paths and reasons");
                        egui::Popup::from_toggle_button_response(&report)
                            .width(420.0_f32.min(ui.ctx().content_rect().width() - 24.))
                            .show(|ui| {
                                ui.strong("Skipped during scan");
                                ui.label("The scanner could not read these files or folders.");
                                if progress.skipped > progress.skipped_paths.len() as u64 {
                                    ui.label(format!(
                                        "Showing the first {} of {}.",
                                        progress.skipped_paths.len(),
                                        grouped(progress.skipped as usize)
                                    ));
                                }
                                egui::ScrollArea::vertical()
                                    .max_height(230.)
                                    .show(ui, |ui| {
                                        for (path, reason) in progress.skipped_paths.iter() {
                                            ui.separator();
                                            ui.label(path.display().to_string());
                                            ui.weak(reason);
                                        }
                                    });
                            });
                    } else {
                        let status = ui.label(text(message, 12.0, colors.muted));
                        if progress.running {
                            status.on_hover_text(format!(
                                "{}\n{} files checked",
                                progress.current.display(),
                                grouped(progress.checked as usize)
                            ));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !self.file_errors.is_empty()
                            && button(ui, "File operation results", None, Quiet).clicked()
                        {
                            self.dialog = Some(Dialog::FileErrors);
                        }
                        if progress.running {
                            self.pause_button(ui, progress);
                        } else if !self.missing.is_empty()
                            && button(ui, "Media setup", Some(Icon::Info), Quiet)
                                .on_hover_text(format!("Missing: {}", self.missing.join(", ")))
                                .clicked()
                        {
                            self.dialog = Some(dialogs::Dialog::Media);
                        }
                    });
                });
            });
    }

    fn pause_button(&self, ui: &mut Ui, progress: &Progress) {
        let (label, icon) = if progress.paused {
            ("Resume", Icon::Play)
        } else {
            ("Pause", Icon::Pause)
        };
        if icon_button(ui, label, icon, Quiet).clicked() {
            self.service.scan.set_paused(!progress.paused);
            ui.ctx().request_repaint();
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let colors = theme::palette(ui);
        let title = if self.filter.text.trim().is_empty() {
            self.title()
        } else {
            "Search results".into()
        };
        let count = self.browser.total();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                if let Some(path) = self.breadcrumb_path() {
                    self.breadcrumbs(ui, &path);
                } else {
                    self.subfolders = None;
                    ui.add(egui::Label::new(strong(title, 28.0, colors.text)).truncate());
                }
                let mut description = if let Some(count) = count {
                    format!(
                        "{} {}",
                        grouped(count),
                        if count == 1 { "video" } else { "videos" }
                    )
                } else if self.ready {
                    "Searching…".into()
                } else {
                    "Opening library…".into()
                };
                if self.range_request.is_some() {
                    description.push_str("  ·  Selecting range…");
                } else if self.selection.len() > 1 {
                    description
                        .push_str(&format!("  ·  {} selected", grouped(self.selection.len())));
                }
                ui.label(text(description, 13.0, colors.muted));
            });
            if self.pan_mode && button(ui, "Done", None, Secondary).clicked() {
                self.pan_mode = false;
            }
        });
        ui.add_space(13.0);
        let mut modified = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.y = 34.0;
            modified |= self.quick_filters(ui);
            let more = catalog_button(
                &self.watch,
                ui,
                "advanced_filters",
                "Filters",
                76.,
                false,
                Some(Icon::Filter),
            );
            if more.clicked() {
                self.dialog = Some(dialogs::Dialog::Filters(Box::new(
                    dialogs::FilterDraft::new(self.filter.clone()),
                )));
            }
            if self.filter.narrowing()
                && icon_button(ui, "Clear filters", Icon::Close, Quiet).clicked()
            {
                self.filter = Filter {
                    sort: self.filter.sort,
                    directory: self.filter.directory.clone(),
                    ..Filter::default()
                };
                self.filter_open = false;
                modified = true;
            }
            modified |= self.sort_and_view(ui);
        });
        if modified {
            self.apply_filter();
        }
        ui.add_space(9.0);
    }

    fn quick_filters(&mut self, ui: &mut Ui) -> bool {
        let mut modified = false;
        modified |= quick(
            &self.watch,
            ui,
            "format",
            "Format",
            &mut self.filter.ext,
            &[
                ("", "All formats"),
                ("mp4", "MP4"),
                ("mkv", "MKV"),
                ("mov", "MOV"),
                ("webm", "WebM"),
                ("avi", "AVI"),
                ("m4v", "M4V"),
            ],
        );
        let mut resolution = match (self.filter.min_height, self.filter.max_height) {
            (None, Some(720)) => "sd",
            (Some(720), Some(1080)) => "hd",
            (Some(1080), Some(2160)) => "full",
            (Some(2160), None) => "4k",
            _ => "",
        }
        .to_owned();
        if quick(
            &self.watch,
            ui,
            "resolution",
            "Resolution",
            &mut resolution,
            &[
                ("", "Any resolution"),
                ("sd", "Below 720p"),
                ("hd", "720p HD"),
                ("full", "1080p–2K"),
                ("4k", "4K+"),
            ],
        ) {
            (self.filter.min_height, self.filter.max_height) = match resolution.as_str() {
                "sd" => (None, Some(720)),
                "hd" => (Some(720), Some(1080)),
                "full" => (Some(1080), Some(2160)),
                "4k" => (Some(2160), None),
                _ => (None, None),
            };
            modified = true;
        }
        let mut duration = match (self.filter.min_seconds, self.filter.max_seconds) {
            (None, Some(300)) => "short",
            (Some(300), Some(1200)) => "middle",
            (Some(1200), None) => "long",
            _ => "",
        }
        .to_owned();
        if quick(
            &self.watch,
            ui,
            "duration",
            "Duration",
            &mut duration,
            &[
                ("", "Any duration"),
                ("short", "Under 5 min"),
                ("middle", "5–20 min"),
                ("long", "20 min or more"),
            ],
        ) {
            (self.filter.min_seconds, self.filter.max_seconds) = match duration.as_str() {
                "short" => (None, Some(300)),
                "middle" => (Some(300), Some(1200)),
                "long" => (Some(1200), None),
                _ => (None, None),
            };
            modified = true;
        }
        modified
    }

    fn sort_and_view(&mut self, ui: &mut Ui) -> bool {
        const SORT_WIDTH: f32 = 146.0;
        let group_width = SORT_WIDTH + ui.spacing().item_spacing.x + 34.0;
        if ui.available_size_before_wrap().x < group_width && ui.max_rect().width() >= group_width {
            ui.end_row();
        }
        ui.add_space((ui.available_size_before_wrap().x - group_width).max(0.0));
        let mut modified = false;
        let sort = catalog_button(
            &self.watch,
            ui,
            "video_sort",
            self.filter.sort.label(),
            SORT_WIDTH,
            false,
            None,
        )
        .on_hover_text("Sort videos");
        sort.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::ComboBox,
                true,
                format!("Sort videos: {}", self.filter.sort.label()),
            )
        });
        if let Some(choice) = choose_filter(
            &self.watch,
            &sort,
            SORT_WIDTH,
            Sort::ALL.into_iter().map(|s| (s, s.label())),
            self.filter.sort,
        ) {
            modified = choice != self.filter.sort;
            self.filter.sort = choice;
        }
        let (label, icon) = if self.view.list {
            ("Switch to grid view", Icon::Grid)
        } else {
            ("Switch to list view", Icon::List)
        };
        if ui
            .add_enabled_ui(self.ready, |ui| icon_button(ui, label, icon, Secondary))
            .inner
            .clicked()
        {
            self.cancel_catalog_gestures();
            self.view.list = !self.view.list;
            self.service.send(Action::SaveView(self.view));
        }
        modified
    }

    fn view_shortcuts(&mut self, ui: &Ui) {
        if !self.ready || !self.keys_allowed(ui.ctx()) {
            return;
        }
        let over_library = ui.rect_contains_pointer(ui.max_rect());
        let (mut direction, wheel) = ui.input(|input| {
            let mut keys = 0;
            let mut wheel = 0;
            for event in &input.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.ctrl || modifiers.command => {
                        keys += match key {
                            egui::Key::Plus | egui::Key::Equals => 1,
                            egui::Key::Minus => -1,
                            _ => 0,
                        };
                    }
                    egui::Event::MouseWheel {
                        delta, modifiers, ..
                    } if (modifiers.ctrl || modifiers.command)
                        && over_library
                        && delta.y != 0.0 =>
                    {
                        wheel = delta.y.signum() as isize;
                    }
                    _ => {}
                }
            }
            (keys, wheel)
        });
        if wheel != 0
            && self
                .last_view_wheel
                .is_none_or(|last| last.elapsed() >= Duration::from_millis(90))
        {
            self.last_view_wheel = Some(Instant::now());
            direction += wheel;
        }
        let size = self
            .view
            .size
            .saturating_add_signed(direction)
            .min(GRID_COLUMNS.len() - 1);
        if self.view.size != size {
            self.cancel_catalog_gestures();
            self.view.size = size;
            self.service.send(Action::SaveView(self.view));
        }
    }

    fn body(&mut self, ui: &mut Ui, progress: &Progress) {
        egui::CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| {
                self.view_shortcuts(ui);
                let mut scroll = theme::page_scroll(ui);
                let bounds = ui.available_rect_before_wrap();
                let content =
                    bounds.with_max_x(bounds.right() - ui.spacing().scroll.allocated_width());
                let mut columns = ui.new_child(egui::UiBuilder::new().max_rect(content));
                let close = self.detail_panel(&mut columns);
                let catalog_right = columns.available_rect_before_wrap().right();
                let salt = ("video_content", self.view.list, self.view.size);
                if self.pan_input.blocking() {
                    scroll = scroll.scroll_source(egui::scroll_area::ScrollSource {
                        drag: egui::scroll_area::DragScroll::Never,
                        ..Default::default()
                    });
                }
                if self.browser.take_scroll_reset() {
                    scroll = scroll.vertical_scroll_offset(0.);
                }
                self.catalog_viewport = None;
                let output =
                    scroll
                        .id_salt(salt)
                        .animated(false)
                        .show_viewport(ui, |page, visible| {
                            page.set_max_width(catalog_right - page.max_rect().left());
                            page.shrink_clip_rect(page.clip_rect().with_max_x(catalog_right));
                            theme::layer(Color32::TRANSPARENT, 24, 18).show(page, |ui| {
                                self.toolbar(ui);
                                match self.browser.total() {
                                    _ if self.startup_error.is_some() || self.browser.failed() => {
                                        self.empty(ui, progress, true)
                                    }
                                    Some(0) => self.empty(ui, progress, false),
                                    None => {
                                        ui.add_space(32.);
                                        ui.vertical_centered(|ui| ui.spinner());
                                    }
                                    Some(total) => self.videos(ui, total, bounds, visible.top()),
                                }
                            });
                        });
                if let Some(viewport) = &mut self.catalog_viewport {
                    viewport.offset = output.state.offset.y;
                }
                if close && self.detail.is_some() {
                    self.close_details(None);
                    ui.ctx().request_repaint();
                }
            });
    }

    fn read_catalog_pan(&mut self, ctx: &Context, raw: &mut egui::RawInput) {
        if self.dialog.is_some() || !raw.focused || (self.watch.active() && !self.watch.mini()) {
            self.pan_input.clear();
            self.pan_selection = None;
            return;
        }
        if let Some(viewport) = self.catalog_viewport {
            self.pan_input.read(
                &mut raw.events,
                viewport.bounds,
                ctx.pointer_hover_pos(),
                |point| {
                    viewport.bounds.contains(point)
                        && ctx.layer_id_at(point) == Some(viewport.layer)
                },
            );
        }
    }

    fn trace_catalog_pan(&mut self, ui: &Ui, grid: marquee::Grid, area: egui::Rect, offset: f32) {
        if self.pan_input.has_path() {
            if self.pan_selection.is_none() {
                self.marquee = None;
                self.range_request = None;
                self.pan_mode = true;
                ui.memory_mut(|memory| memory.surrender_focus(Id::new(SEARCH_ID)));
            }
            self.pan_selection
                .get_or_insert_with(PanSelection::default)
                .trace(
                    grid,
                    area,
                    offset,
                    &mut self.pan_input,
                    |index| self.browser.get(index).map(|video| video.id),
                    &mut self.selection,
                );
        }
        if self.pan_input.take_finished()
            && let Some(pan) = self.pan_selection.take()
        {
            let (mode, positions) = pan.finish();
            if !positions.is_empty() {
                self.request_number += 1;
                self.range_request = Some(PendingSelection {
                    id: self.request_number,
                    mode,
                    marquee: None,
                    pan: true,
                    result: None,
                });
                self.service.select_range(SelectionRange {
                    id: self.request_number,
                    filter: self.browser.filter().clone(),
                    target: SelectionTarget::Positions(positions),
                });
            }
        }
    }

    fn videos(&mut self, ui: &mut Ui, total: usize, page: egui::Rect, page_offset: f32) {
        let origin = ui.next_widget_position();
        let grid = marquee::Grid::catalog(self.view, ui.available_width(), total);
        let bounds = egui::Rect::from_min_max(
            egui::pos2(origin.x, origin.y.max(page.top())),
            egui::pos2(origin.x + ui.available_width(), page.bottom()),
        );
        let viewport = CatalogViewport {
            layer: ui.layer_id(),
            bounds,
            columns: grid.columns,
            rows_top: origin.y + page_offset - page.top(),
            row_height: grid.size.y,
            stride: grid.stride(),
            height: page.height(),
            offset: page_offset,
        };
        let pan_blocks_pointer = self.pan_input.blocking();
        let mut pressed = None;
        let mut play = None;
        let mut browse = None;
        let mut select = None;
        let mut clear_selection = false;
        let mut file_action = None;
        let selected = self.detail.as_ref().map(|video| video.id);
        let mouse_click = ui.input(|input| {
            input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                } => Some((*pos, *modifiers)),
                _ => None,
            })
        });
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let primary_down = ui.input(|input| input.pointer.primary_down());
        let press = ui.input(primary_press);
        let cancel_drag = self.marquee.is_some()
            && !pan_blocks_pointer
            && (ui.input(|input| !input.focused || input.key_pressed(egui::Key::Escape))
                || self.dialog.is_some());
        if cancel_drag {
            self.cancel_catalog_gestures();
        }
        let pending_scroll = self.catalog_scroll.take();
        let mut target = if self.marquee.is_none()
            && !pan_blocks_pointer
            && self.keys_allowed(ui.ctx())
            && let Some(index) = self.grid_keyboard(ui, grid.columns, total)
        {
            Some(viewport.reveal(index))
        } else {
            match pending_scroll {
                Some(CatalogScroll::Reveal(id)) => self
                    .browser
                    .position(id)
                    .filter(|&index| index < total)
                    .map(|index| viewport.reveal(index)),
                Some(CatalogScroll::RestoreOffset(offset)) => Some(offset),
                Some(CatalogScroll::RestoreAnchor(mut anchor)) => {
                    anchor.index = self.browser.position(anchor.id).unwrap_or(anchor.index);
                    anchor.index = anchor.index.min(total - 1);
                    Some(viewport.restore(anchor))
                }
                None => None,
            }
        };
        let max_offset = (viewport.rows_top + grid.height() + 18. - viewport.height).max(0.0);
        let drag_point = self
            .marquee
            .as_mut()
            .zip(pointer)
            .and_then(|(drag, point)| {
                drag.dragged |= point.distance(drag.press) >= 5.0;
                (drag.dragged && primary_down && !pan_blocks_pointer).then_some(point)
            });
        let pan_point = self
            .pan_selection
            .as_ref()
            .and_then(|_| self.pan_input.point());
        if let Some(point) = pan_point.or(drag_point) {
            let dt = ui.input(|input| input.stable_dt).min(0.05);
            let delta = marquee::edge_scroll(point, viewport.bounds, dt);
            let offset = (viewport.offset + delta).clamp(0.0, max_offset);
            if offset != viewport.offset {
                target = Some(offset);
                ui.ctx().request_repaint();
            }
        }
        if let Some(target) = target {
            ui.scroll_with_delta(vec2(0., page_offset - target.clamp(0., max_offset)));
        }
        if self.pending_keyboard_selection
            && let Some(video) = self.cursor.and_then(|index| self.browser.get(index))
        {
            self.select_video(video.id, Modifiers::NONE);
            self.pending_keyboard_selection = false;
        }
        let area = bounds;
        let visible = area.translate(-origin.to_vec2());
        let hit_rect = area.intersect(ui.clip_rect());
        let cols = grid.columns;
        let height = grid.size.y;
        let offset = visible.top();
        let mut finished_drag = cancel_drag || pan_blocks_pointer;
        if pan_blocks_pointer {
            self.marquee = None;
            self.trace_catalog_pan(ui, grid, area, offset);
        }
        if !finished_drag
            && self.marquee.is_none()
            && let Some((press, modifiers)) = press
            && hit_rect.contains(press)
            && ui.is_enabled()
            && self.dialog.is_none()
            && ui.ctx().layer_id_at(press) == Some(ui.layer_id())
            && !grid.occupied(marquee::content_point(press, area, offset))
        {
            ui.memory_mut(|memory| memory.surrender_focus(Id::new(SEARCH_ID)));
            self.suggestion = None;
            self.range_request = None;
            self.marquee = Some(Marquee {
                start: marquee::content_point(press, area, offset),
                press,
                base: self.selection.clone(),
                mode: SelectionMode::from_modifiers(modifiers),
                dragged: false,
            });
        }
        let live = self
            .marquee
            .as_mut()
            .zip(mouse_click.map(|(pos, _)| pos).or(pointer))
            .and_then(|(drag, pointer)| {
                drag.dragged |= pointer.distance(drag.press) >= 5.0;
                drag.dragged.then(|| {
                    let rect = marquee::marquee_rect(drag.start, pointer, area, offset);
                    (rect, grid.region(rect))
                })
            });
        // Cards register after the background so their click targets have priority.
        let background = ui.interact(
            hit_rect,
            ui.make_persistent_id("catalog_background"),
            Sense::CLICK,
        );
        ui.allocate_space(vec2(area.width(), grid.height()));
        let highlight = self
            .marquee
            .as_ref()
            .zip(live.as_ref())
            .map(|(drag, (_, region))| (drag.mode, &drag.base, region))
            .or_else(|| {
                let pending = self.range_request.as_ref()?;
                let marquee = pending.marquee.as_ref()?;
                Some((pending.mode, &marquee.base, &marquee.region))
            });
        let range = grid.visible_rows(visible);
        ui.spacing_mut().item_spacing = vec2(grid.gap, grid.gap);
        self.browser
            .set_target(range.start * cols..(range.end * cols).min(total));
        for row in range {
            let row_rect = egui::Rect::from_min_size(
                origin + vec2(CATALOG_GUTTER, row as f32 * grid.stride()),
                vec2(grid.width(), height),
            );
            ui.scope_builder(
                egui::UiBuilder::new().id_salt(row).max_rect(row_rect),
                |ui| {
                    ui.horizontal(|ui| {
                        for index in row * cols..((row + 1) * cols).min(total) {
                            let Some(video) = self.browser.get(index) else {
                                library::placeholder(ui, grid.size);
                                continue;
                            };
                            let active = highlight.map_or_else(
                                || self.selection.contains(&video.id),
                                |(mode, base, region)| {
                                    mode.selected(base.contains(&video.id), region.contains(index))
                                },
                            );
                            let (response, play_artwork, over_timeline) = if self.view.list {
                                library::list_item(ui, video, active, height, &mut self.thumbs)
                            } else {
                                library::card(ui, video, active, grid.size, &mut self.thumbs)
                            };
                            let saved = self.settings.watch_later.contains(&video.id);
                            let save_label = watch_later_label(saved);
                            if !pan_blocks_pointer
                                && library::watch_later_button(
                                    ui,
                                    &response,
                                    self.view.list,
                                    saved,
                                    save_label,
                                )
                            {
                                self.service.send(Action::WatchLater(video.id, !saved));
                            }
                            let modifiers = mouse_click
                                .filter(|(position, _)| response.rect.contains(*position))
                                .map_or(Modifiers::NONE, |(_, modifiers)| modifiers);
                            let modified = modifiers.shift || modifiers.ctrl || modifiers.command;
                            if !finished_drag
                                && self.marquee.is_none()
                                && response.clicked()
                                && !theme::control_click(&response)
                                && (self.pan_mode || modified || !over_timeline)
                            {
                                select = Some((
                                    video.id,
                                    if self.pan_mode {
                                        Modifiers::CTRL
                                    } else {
                                        modifiers
                                    },
                                ));
                                self.cursor = Some(index);
                                self.pending_keyboard_selection = false;
                                if !modified && !self.pan_mode {
                                    if play_artwork || response.double_clicked() {
                                        play = Some(video.clone());
                                    } else {
                                        pressed = Some(video.clone());
                                    }
                                }
                            }
                            let selected_here = self.selection.contains(&video.id);
                            let action = self.watch.context_menu(&response, 230., || {
                                context_menu::LibraryMenu {
                                    selected_here,
                                    selection_count: self.selection.len(),
                                    save_label,
                                    range_ready: self.range_request.is_none(),
                                    files_idle: self.file_job.is_none(),
                                    collections: &self.settings.collections,
                                }
                                .items()
                            });
                            if let Some(action) = action {
                                use context_menu::LibraryAction as A;
                                let ids = || {
                                    if selected_here {
                                        self.selection.iter().copied().collect()
                                    } else {
                                        vec![video.id]
                                    }
                                };
                                match action {
                                    A::Play => play = Some(video.clone()),
                                    A::Details => pressed = Some(video.clone()),
                                    A::Browse => {
                                        browse = video.path.parent().map(Path::to_path_buf)
                                    }
                                    A::WatchLater => {
                                        self.service.send(Action::WatchLater(video.id, !saved));
                                    }
                                    A::NewCollection => {
                                        self.dialog = Some(Dialog::Collection(String::new(), ids()))
                                    }
                                    A::Collection(id) => {
                                        self.service.send(Action::AddToCollection(
                                            CollectionTarget::Existing(id),
                                            ids(),
                                            selected,
                                        ));
                                    }
                                    A::ToggleSelection => {
                                        select = Some((video.id, Modifiers::CTRL))
                                    }
                                    A::ClearSelection => clear_selection = true,
                                    A::Reveal => {
                                        self.service.send(Action::Reveal(video.path.clone()));
                                    }
                                    A::CopyPaths => {
                                        self.service.send(Action::CopyPaths(ids()));
                                    }
                                    A::File(intent) => {
                                        file_action = Some((intent, video.clone(), ids()));
                                    }
                                }
                            }
                        }
                    });
                },
            );
        }
        if let Some((rect, region)) = live {
            marquee::paint(ui, rect, area, offset);
            if mouse_click.is_some() {
                let drag = self.marquee.take().unwrap();
                self.request_number += 1;
                self.range_request = Some(PendingSelection {
                    id: self.request_number,
                    mode: drag.mode,
                    marquee: Some(MarqueeSelection {
                        base: drag.base,
                        region: region.clone(),
                        rect,
                    }),
                    pan: false,
                    result: None,
                });
                self.service.select_range(SelectionRange {
                    id: self.request_number,
                    filter: self.browser.filter().clone(),
                    target: SelectionTarget::Region(region),
                });
                finished_drag = true;
            }
        } else if let Some(marquee) = self
            .range_request
            .as_ref()
            .and_then(|pending| pending.marquee.as_ref())
        {
            marquee::paint(ui, marquee.rect, area, offset);
        }
        if !primary_down {
            self.marquee = None;
        }
        self.catalog_viewport = Some(viewport);

        if clear_selection
            || (!finished_drag
                && self.dialog.is_none()
                && !egui::Popup::is_any_open(ui.ctx())
                && background.clicked_by(egui::PointerButton::Primary)
                && mouse_click.is_some_and(|(_, modifiers)| modifiers.is_none()))
        {
            self.clear_selection();
            ui.ctx().request_repaint();
        }
        if !finished_drag && let Some((id, modifiers)) = select {
            self.select_video(id, modifiers);
            ui.ctx().request_repaint();
        }
        if let Some((intent, video, ids)) = file_action {
            self.open_file_dialog(intent, ids, Some(video));
        }
        if !finished_drag && let Some(video) = play {
            self.play(video, PlaybackView::FullPlayer);
        }
        if !finished_drag && let Some(video) = pressed {
            self.inspect(video);
            ui.ctx().request_repaint();
        }
        if let Some(path) = browse {
            self.browse_folder(path);
        }
        self.browser.update(
            ui.ctx(),
            &self.service,
            self.detail_removal.map(|(_, anchor)| anchor.id),
        );
    }

    fn grid_keyboard(&mut self, ui: &Ui, cols: usize, total: usize) -> Option<usize> {
        use egui::{Key, Modifiers};
        let consume = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
        if self.can_delete_selection()
            && !ui.ctx().text_edit_focused()
            && (consume(Key::Delete)
                || (cfg!(target_os = "macos")
                    && (consume(Key::Backspace)
                        || ui.input_mut(|input| {
                            input.consume_key(Modifiers::COMMAND, Key::Backspace)
                        }))))
        {
            self.delete_selection();
            return None;
        }
        let change = ui.input_mut(|input| {
            let mut count = |key| input.count_and_consume_key(Modifiers::NONE, key) as isize;
            let horizontal = count(Key::ArrowRight) - count(Key::ArrowLeft);
            let vertical = count(Key::ArrowDown) - count(Key::ArrowUp);
            horizontal + vertical * cols as isize
        });
        let first = self.browser.viewport().start.min(total - 1);
        if change != 0 {
            let index = self
                .cursor
                .map_or(first, |i| i.saturating_add_signed(change).min(total - 1));
            self.cursor = Some(index);
            self.pending_keyboard_selection = true;
            Some(index)
        } else if consume(Key::Enter)
            && let Some(index) = self.cursor
        {
            self.play_at(index, PlaybackView::FullPlayer);
            None
        } else {
            None
        }
    }

    fn empty(&mut self, ui: &mut Ui, progress: &Progress, error: bool) {
        let colors = theme::palette(ui);
        let (heading, description) = if error {
            (
                "Couldn’t open the library",
                self.startup_error
                    .as_deref()
                    .unwrap_or("The catalog could not be read.")
                    .to_owned(),
            )
        } else if let Some(status) = self.browser.path_status() {
            match status {
                SearchPathStatus::Unsupported => (
                    "Unsupported file",
                    "This file does not contain a supported video stream.".into(),
                ),
                SearchPathStatus::Checking => (
                    "Checking video file",
                    "It will appear after MyTube verifies its video stream.".into(),
                ),
                SearchPathStatus::OutsideLibrary => (
                    "File outside the library",
                    "Add this file’s containing folder as a source to check for videos.".into(),
                ),
            }
        } else if (progress.running || self.validating) && !self.filter.narrowing() {
            (
                "Looking for videos",
                "Your videos will appear here as the scan finds them.".into(),
            )
        } else if self.filter.narrowing() {
            (
                "No matching videos",
                "Try a different search or clear your filters.".into(),
            )
        } else {
            match self.location {
                Location::Collection(_) => (
                    "This collection is empty",
                    "Right-click a video in your library to add it to a collection.".into(),
                ),
                Location::WatchLater => (
                    "Watch Later is empty",
                    "Use the clock on a video to save it here.".into(),
                ),
                Location::Recent => (
                    "No recent videos",
                    "Videos added in the last seven days will appear here.".into(),
                ),
                Location::Source(_) => (
                    "No videos in this source",
                    "Refresh your sources after adding videos to this folder.".into(),
                ),
                Location::All if self.settings.auto_scan && self.settings.sources.is_empty() => (
                    "Looking for local drives",
                    "Mounted drives are discovered automatically.".into(),
                ),
                Location::All if self.settings.sources.is_empty() => (
                    "Your videos, all in one place",
                    "Add a folder or drive to start building your library.".into(),
                ),
                Location::All => (
                    "Your library is empty",
                    "Add another source or refresh your library after adding videos.".into(),
                ),
            }
        };
        let height = (ui.available_height() - 5.0).clamp(219.0, 495.0);
        let width = ui.available_width();
        let frame = Frame::NONE.inner_margin(egui::Margin::symmetric(17, 8));
        frame.show(ui, |ui| {
            ui.set_min_size(vec2((width - 36.0).max(238.0), height - 18.0));
            ui.add_space(((height - 300.0) * 0.5).max(7.0));
            ui.vertical_centered(|ui| {
                if error
                    || matches!(
                        self.browser.path_status(),
                        Some(SearchPathStatus::Unsupported)
                    )
                {
                    let (rect, _) = ui.allocate_exact_size(vec2(48.0, 64.0), Sense::hover());
                    Icon::Info.paint(ui.painter(), rect.center(), 32.0, colors.text);
                } else {
                    library::empty_illustration(ui, (height / 330.0).clamp(0.58, 1.0));
                }
                ui.add_space(7.0);
                ui.label(strong(
                    heading,
                    if width < 395.0 { 20.0 } else { 24.0 },
                    colors.text,
                ));
                ui.add(egui::Label::new(text(description, 13.5, colors.muted)).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let first_library = self.location == Location::All && !self.filter.narrowing();
                    if self.startup_error.is_some() && ui.button("Retry opening library").clicked()
                    {
                        self.startup_error = None;
                        self.service = Service::start(ui.ctx().clone());
                    } else if first_library && !error && !progress.running && !self.validating {
                        let spacing = (ui.available_width() - 254.0).max(0.0) / 2.0;
                        ui.add_space(spacing);
                        if button(ui, "Add folder", Some(Icon::Plus), Primary).clicked() {
                            self.dialog = Some(dialogs::Dialog::path(dialogs::PathTarget::Source));
                        }
                        if button(ui, "Add drive", Some(Icon::Disk), Secondary).clicked() {
                            self.open_drives();
                        }
                    } else if self.filter.narrowing() {
                        ui.add_space((ui.available_width() - 124.0).max(0.0) / 2.0);
                        if button(ui, "Clear filters", Some(Icon::Close), Secondary).clicked() {
                            self.filter = Filter {
                                sort: self.filter.sort,
                                directory: self.filter.directory.clone(),
                                ..Filter::default()
                            };
                            self.filter_open = false;
                            self.apply_filter();
                        }
                    }
                });
            });
        });
    }

    fn show_ui(&mut self, ui: &mut Ui) {
        if self.ready
            && self.dialog.is_none()
            && ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, egui::Key::Comma))
        {
            self.watch.toggle_settings();
        }
        self.playback_and_clipboard_shortcuts(ui.ctx());
        let progress = self.service.scan.progress();
        self.thumbs.begin();
        let reduce_motion = self.watch.reduce_motion();
        self.thumbs.hover.begin(
            self.dialog.is_none()
                && !self.pan_input.blocking()
                && self.marquee.is_none()
                && !ui.input(|i| i.modifiers.any() || i.pointer.primary_down() || i.any_touches()),
            reduce_motion,
        );
        if !self.watch.fullscreen() {
            if !self.watch.active() || self.watch.mini() {
                self.sidebar(ui);
            }
            self.header(ui);
            self.footer(ui, &progress);
        }
        if self.watch.active() && !self.watch.mini() {
            let shortcuts = self.keys_allowed(ui.ctx());
            let slot = self.watch.prepare_sidebars(ui, reduce_motion);
            let player = if self.watch.fullscreen() || self.watch.music {
                slot
            } else {
                ui.available_rect_before_wrap()
            };
            self.watch
                .show_playlist(ui, &mut self.browser, &mut self.thumbs);
            ui.scope_builder(egui::UiBuilder::new().max_rect(player), |ui| {
                ui.set_clip_rect(player);
                self.watch.show(
                    ui,
                    &mut self.browser,
                    shortcuts,
                    &mut self.thumbs,
                    &self.service,
                    &self.settings,
                );
            });
        } else {
            self.watch.settings_panel.sidebar = None;
            if self.watch.mini() {
                let shortcuts = self.keys_allowed(ui.ctx());
                let queue = watch::Queue::for_video(&self.browser, self.watch.video());
                self.watch
                    .show_mini(ui, &queue, &mut self.thumbs, &self.service, shortcuts);
            } else {
                self.watch.cancel_preview(&self.service);
            }
            self.body(ui, &progress);
        }
        if let Some(result) = self
            .range_request
            .as_mut()
            .and_then(|pending| pending.result.take())
        {
            self.apply_selection_range(result);
            ui.ctx().request_repaint();
        }
        self.watch_visible_videos();
        if let Some(position) = self.watch.take_request() {
            self.play_at(position, PlaybackView::Current);
        }
        self.thumbs.flush(&self.service);
        self.thumbs.hover.flush(&self.service);
        let player = self.watch.timeline_update();
        let library = self.thumbs.hover.timeline_update();
        match (player, library) {
            (Some(Some(request)), _) | (_, Some(Some(request))) => {
                self.service.timeline_preview(Some(request));
            }
            (Some(None), _) | (_, Some(None)) => self.service.timeline_preview(None),
            (None, None) => {}
        }
        if let Some(id) = self.watch.take_new_collection() {
            self.dialog = Some(Dialog::Collection(String::new(), vec![id]));
        }
        if self.ready
            && let Some((message, error)) = self
                .watch
                .preferences_frame(&self.service, &mut self.appearance)
        {
            self.status(message, error);
        }
        if std::mem::take(&mut self.watch.settings_panel.export_playlist) {
            self.service
                .save_playlist_dialog(self.browser.filter().clone());
        }
        if std::mem::take(&mut self.watch.settings_panel.clear_history) {
            self.service.send(Action::ClearPlaybackHistory);
        }
        if std::mem::take(&mut self.watch.settings_panel.request_history) {
            self.service.send(Action::History);
        }
        if let Some(video) = self.watch.settings_panel.history_choice.take() {
            self.watch.settings_panel.open = false;
            self.play(video, PlaybackView::FullPlayer);
        }
        self.notice(ui.ctx());
        self.show_dialog(ui.ctx(), &progress);
    }

    fn notice(&mut self, ctx: &Context) {
        let colors = theme::colors(ctx, &ctx.global_style().visuals);
        if let Some((message, error, _)) = &self.notice {
            let mut close = false;
            egui::Area::new(Id::new("library_notice"))
                .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-20.0, -48.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    Frame::new()
                        .fill(colors.card)
                        .stroke(Stroke::new(
                            1.0,
                            if *error { colors.text } else { colors.line },
                        ))
                        .corner_radius(9)
                        .inner_margin(egui::Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.set_max_width(392.0);
                                ui.add(egui::Label::new(text(message, 13.0, colors.text)).wrap());
                                close = icon_button(ui, "Dismiss message", Icon::Close, Quiet)
                                    .clicked();
                            });
                        });
                });
            if close {
                self.notice = None;
            }
        }
    }
}

impl eframe::App for MyTube {
    fn raw_input_hook(&mut self, ctx: &Context, raw: &mut egui::RawInput) {
        self.read_catalog_pan(ctx, raw);
    }

    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.events(ctx);
        self.watch.sync_minimized();
        theme::apply(ctx, self.appearance);
        self.watch.save_checkpoint(&self.service, false);
        #[cfg(target_os = "macos")]
        if let Some(index) = self
            .watch
            .update_now_playing(&watch::Queue::for_video(&self.browser, self.watch.video()))
        {
            self.play_at(index, PlaybackView::Current);
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        ui.set_style(ui.ctx().global_style());
        if self.placing_window(ui.ctx(), frame) {
            return;
        }
        if self.ready && !self.initial_window_applied {
            player_window::apply_initial(ui.ctx(), frame, &self.settings.player.window);
            self.initial_window_applied = true;
        }
        #[cfg(target_os = "macos")]
        {
            self.watch.menu_host.attach(frame);
            self.watch
                .settings_panel
                .native
                .attach(self.watch.menu_host.view());
        }
        let background = theme::palette(ui).back;
        #[cfg(target_os = "macos")]
        let background = {
            let strength = if !self.watch.fullscreen()
                && theme::menu_material(ui.ctx()) == control_bar::DisplayMaterial::Glass
            {
                theme::glass_amount(ui.ctx())
            } else {
                0.
            };
            self.library_glass
                .update(self.watch.menu_host.view(), strength);
            if self.library_glass.visible() {
                // One translucent surface for all library panels, including the title bar.
                theme::glass_tint(ui.ctx(), background)
            } else {
                background
            }
        };
        ui.painter().rect_filled(ui.max_rect(), 0, background);
        #[cfg(target_os = "macos")]
        if !self.watch.fullscreen()
            && let Some(root) = self.watch.menu_host.view()
            && let Some(window) = root.window()
        {
            let height = root.bounds().size.height;
            if height > 0. {
                let inset = ((height - window.contentLayoutRect().size.height).max(0.) / height)
                    as f32
                    * ui.ctx().viewport_rect().height();
                if inset > 0. {
                    Panel::top("native_titlebar")
                        .exact_size(inset)
                        .resizable(false)
                        .show_separator_line(false)
                        .frame(Frame::NONE)
                        .show(ui, |ui| {
                            window_handle(ui, "native_window_drag", ui.max_rect());
                        });
                }
            }
        }
        #[cfg(target_os = "macos")]
        self.media_buttons.begin(
            ui.ctx(),
            self.dialog.is_none()
                && self.watch.menu_host.view().is_some()
                && theme::menu_material(ui.ctx()) == control_bar::DisplayMaterial::Glass,
        );
        self.show_ui(ui);
        #[cfg(target_os = "macos")]
        {
            self.watch.menu_host.finish_catalog_buttons(
                ui.ctx(),
                self.dialog.is_some(),
                self.dialog_glass.dialog_view(),
            );
            if self.dialog.is_some() {
                self.media_buttons.begin(ui.ctx(), false);
            }
            self.media_buttons.finish(
                ui.ctx(),
                self.watch.menu_host.view(),
                self.thumbs.hover.timeline_bounds,
            );
            if let Some(popup) = self.breadcrumb_popup.take().filter(|popup| {
                self.dialog.is_none() && egui::Popup::is_id_open(ui.ctx(), popup.layer_id.id)
            }) {
                self.breadcrumb_glass.dialog(
                    &popup,
                    0,
                    theme::FOLDER_GLASS_RADIUS.into(),
                    self.watch.menu_host.view(),
                );
            } else {
                self.breadcrumb_glass.hide();
            }
            self.watch.settings_panel.native.raise_sidebar();
        }
        #[cfg(target_os = "macos")]
        if !self.watch.active() || self.dialog.is_some() || self.watch.music {
            self.watch.controller_glass.hide();
            self.watch.playlist_glass.hide();
        }
        self.aspect_lock
            .update(ui.ctx(), frame, self.watch.locked_aspect());
        self.watch.prepare_pip_window();
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.watch.shutdown(gl, &self.service);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.watch.mini() {
            return [0.; 4];
        }
        #[cfg(target_os = "macos")]
        if self.library_glass.visible() {
            return [0.; 4];
        }
        Color32::to_normalized_gamma_f32(theme::Palette::for_visuals(visuals).back)
    }
}

fn primary_press(input: &egui::InputState) -> Option<(egui::Pos2, Modifiers)> {
    input.events.iter().find_map(|event| match event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers,
        } => Some((*pos, *modifiers)),
        _ => None,
    })
}

fn consume_press(ctx: &Context, key: egui::Key, modifiers: Modifiers) -> bool {
    ctx.input_mut(|input| {
        let previous = input.events.len();
        input.events.retain(|event| {
            !matches!(event, egui::Event::Key {
            key: pressed, pressed: true, repeat: false, modifiers: held, ..
        } if *pressed == key && held.matches_exact(modifiers))
        });
        input.events.len() != previous
    })
}

fn watch_later_control(ui: &mut Ui, id: i64, saved: bool, service: &Service) {
    if icon_button(
        ui,
        watch_later_label(saved),
        Icon::Clock,
        if saved { Selected } else { Secondary },
    )
    .clicked()
    {
        service.send(Action::WatchLater(id, !saved));
    }
}

fn watch_later_label(saved: bool) -> &'static str {
    if saved {
        "Remove from Watch Later"
    } else {
        "Save to Watch Later"
    }
}

fn window_handle(ui: &Ui, id: &str, rect: egui::Rect) {
    let handle = ui.interact(
        rect,
        ui.make_persistent_id(id),
        Sense::click_and_drag() - Sense::focusable_noninteractive(),
    );
    if handle.double_clicked() {
        let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if handle.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

fn folder_choices(ui: &mut Ui, folders: &[PathBuf]) -> Option<PathBuf> {
    const LIST_HEIGHT: f32 = 208.;
    ScrollArea::vertical()
        .max_height(LIST_HEIGHT)
        .min_scrolled_height(LIST_HEIGHT)
        .show_rows(ui, 28., folders.len(), |ui, range| {
            let mut selected = None;
            for folder in &folders[range] {
                if ui
                    .add_sized(
                        [ui.available_width(), 28.],
                        egui::Button::new(folder_label(folder))
                            .right_text(())
                            .frame_when_inactive(false)
                            .truncate(),
                    )
                    .on_hover_text(folder.to_string_lossy())
                    .clicked()
                {
                    selected = Some(folder.clone());
                }
            }
            selected
        })
        .inner
}

fn breadcrumb(ui: &mut Ui, name: impl Into<String>, max_width: f32) -> egui::Response {
    let colors = theme::palette(ui);
    let name = name.into();
    let width = ui
        .painter()
        .layout_no_wrap(
            theme::visual(&name).into_owned(),
            egui::FontId::proportional(13.),
            colors.text,
        )
        .size()
        .x
        + 22.;
    theme::quiet_button(ui, &name, width.min(max_width))
}

fn breadcrumb_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(8., 16.), Sense::hover());
    let c = rect.center();
    ui.painter().add(egui::Shape::line(
        vec![c + vec2(-2., -3.), c + vec2(1., 0.), c + vec2(-2., 3.)],
        Stroke::new(1.4, theme::palette(ui).muted),
    ));
}

fn section(ui: &mut Ui, name: &str) {
    let colors = theme::palette(ui);
    ui.label(strong(format!("  {name}"), 10.0, colors.muted));
}

fn catalog_button(
    _watch: &watch::Watch,
    ui: &mut Ui,
    id: &str,
    label: &str,
    width: f32,
    active: bool,
    icon: Option<Icon>,
) -> egui::Response {
    let colors = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 30.), Sense::hover());
    let response = ui
        .interact(rect, ui.make_persistent_id(id), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if icon.is_some() {
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
    }
    #[cfg(target_os = "macos")]
    if ui.clip_rect().contains_rect(rect)
        && _watch
            .menu_host
            .catalog_face(&response, label, icon, active, false)
    {
        return response;
    }
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let fill = if active {
        colors.accent
    } else if open || response.hovered() {
        colors.hover
    } else {
        colors.well
    };
    let ink = if active {
        colors.on_primary
    } else {
        colors.text
    };
    ui.painter().rect_filled(
        rect,
        15,
        if active {
            fill
        } else {
            theme::control_fill(ui, fill)
        },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.),
            17,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
    }
    let font = egui::FontId::proportional(13.);
    let text_height = ui.fonts_mut(|f| f.row_height(&font));
    theme::cut(
        ui.painter(),
        rect.left_center() + vec2(12., -text_height / 2.),
        width - 34.,
        label,
        font,
        ink,
    );
    let c = rect.right_center() - vec2(13., 0.);
    if let Some(icon) = icon {
        icon.paint(ui.painter(), c, 14., ink);
    } else {
        ui.painter().add(egui::Shape::line(
            vec![c + vec2(-3., -1.5), c + vec2(0., 1.5), c + vec2(3., -1.5)],
            Stroke::new(1.4, ink),
        ));
    }
    response
}

fn quick(
    watch: &watch::Watch,
    ui: &mut Ui,
    id: &str,
    default: &str,
    value: &mut String,
    options: &[(&str, &str)],
) -> bool {
    let font = egui::FontId::proportional(13.);
    let colors = theme::palette(ui);
    let width = options
        .iter()
        .filter(|(key, _)| !key.is_empty())
        .map(|(_, label)| *label)
        .chain([default])
        .map(|label| {
            ui.painter()
                .layout_no_wrap(label.to_owned(), font.clone(), colors.text)
                .size()
                .x
        })
        .fold(0., f32::max)
        .ceil()
        + 48.;
    if ui.available_size_before_wrap().x < width && ui.max_rect().width() >= width {
        ui.end_row();
    }
    let label = if value.is_empty() {
        default
    } else {
        options
            .iter()
            .find(|(key, _)| key == value)
            .map_or(value.as_str(), |(_, label)| *label)
    };
    let mut selected = false;
    let response = catalog_button(watch, ui, id, label, width, !value.is_empty(), None);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ComboBox,
            true,
            if value.is_empty() {
                default.to_owned()
            } else {
                format!("{default}: {label}")
            },
        )
    });
    let choice = choose_filter(
        watch,
        &response,
        width.max(155.),
        options.iter().map(|(key, title)| (*key, *title)),
        value.as_str(),
    );
    if let Some(key) = choice {
        selected = key != value;
        *value = key.into();
    }

    selected
}

fn choose_filter<'a, T: Copy + Eq>(
    _watch: &watch::Watch,
    response: &egui::Response,
    width: f32,
    options: impl Iterator<Item = (T, &'a str)>,
    current: T,
) -> Option<T> {
    #[cfg(target_os = "macos")]
    if theme::menu_material(&response.ctx) == control_bar::DisplayMaterial::Glass
        && let Some(view) = _watch.menu_host.view()
    {
        if !response.clicked() || !_watch.menu_host.begin_tracking(&response.ctx) {
            return None;
        }
        let items = options
            .map(|(action, title)| context_menu::Item::Command {
                title: title.into(),
                action,
                checked: Some(action == current),
                enabled: true,
                destructive: false,
            })
            .collect::<Vec<_>>();
        return macos::context_menu::choose_at(
            view,
            response,
            &items,
            response.rect.left_bottom() + vec2(0., 4.),
        );
    }
    let mut selected = None;
    egui::Popup::menu(response)
        .style(theme::menu_style(&response.ctx))
        .width(width)
        .gap(4.)
        .show(|ui| {
            for (value, label) in options {
                if ui.selectable_label(current == value, label).clicked() {
                    selected = Some(value);
                    ui.close();
                }
            }
        });
    selected
}

fn grouped(number: usize) -> String {
    let raw = number.to_string();
    let mut out = String::new();
    for (index, char) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, PAGE, Stamp};
    use crate::player::options::{InterfaceTheme, PlayerPreferences};

    pub(super) fn catalog_video(id: i64) -> Video {
        Video {
            id,
            rev: 1,
            path: PathBuf::from(format!("videos/{id}.mp4")),
            title: format!("Video {id}"),
            ext: "mp4".into(),
            stamp: Stamp {
                bytes: 10,
                modified: 0,
            },
            added: 0,
            created: None,
            media: Media::default(),
        }
    }

    fn catalog_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(810.0, 660.0),
            )),
            ..Default::default()
        }
    }

    fn click_at(position: egui::Pos2, modifiers: Modifiers) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            },
        ]
    }

    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn breadcrumb_menus_use_the_active_theme_and_navigate_to_the_selected_folder() {
        use crate::player::options::{DEFAULT_GLASS_STRENGTH, InterfaceTheme};
        let ctx = Context::default();
        ctx.enable_accesskit();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated breadcrumbs"));
        let mut app = MyTube::new(&ctx, None, false, service);
        let path = Path::new("/Volumes/TestDrive/Movies/Archive");
        let child = path.join("Clips");
        let draw = |app: &mut MyTube, events| {
            ctx.run_ui(catalog_input(events), |ui| app.breadcrumbs(ui, path))
        };
        let center = |output: &egui::FullOutput, label| {
            let (_, node) = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label}"));
            let rect = node.bounds().unwrap();
            egui::pos2(
                ((rect.x0 + rect.x1) / 2.) as f32,
                ((rect.y0 + rect.y1) / 2.) as f32,
            )
        };
        for style in InterfaceTheme::ALL {
            theme::apply_style(&ctx, style, DEFAULT_GLASS_STRENGTH);
            for (trigger, label, destination) in [
                (
                    "Parent folders",
                    "TestDrive",
                    Path::new("/Volumes/TestDrive"),
                ),
                ("Subfolders", "Clips", child.as_path()),
            ] {
                let output = draw(&mut app, vec![]);
                let button = center(&output, trigger);
                output.drop_without_applying_deltas();
                draw(&mut app, click_at(button, Modifiers::NONE)).drop_without_applying_deltas();
                if trigger == "Subfolders" {
                    app.subfolders.as_mut().unwrap().result = Some(Ok(vec![
                        path.join("Bonus"),
                        path.join("Behind the scenes"),
                        child.clone(),
                    ]));
                }
                let output = draw(&mut app, vec![]);
                let expected_radius = if style.glass_ui() {
                    theme::FOLDER_GLASS_RADIUS
                } else {
                    10
                };
                let shapes = output.shapes.iter().flat_map(|shape| match &shape.shape {
                    egui::Shape::Vec(shapes) => shapes.as_slice(),
                    shape => std::slice::from_ref(shape),
                });
                assert!(shapes.into_iter().any(|shape| {
                    matches!(shape, egui::Shape::Rect(rect) if rect.corner_radius.nw == expected_radius && rect.rect.width() >= 239.)
                }), "missing {style:?} frame for {trigger}");
                let row = center(&output, label);
                output.drop_without_applying_deltas();
                draw(&mut app, click_at(row, Modifiers::NONE)).drop_without_applying_deltas();
                assert_eq!(app.filter.directory.as_deref(), Some(destination));
                assert!(!egui::Popup::is_any_open(&ctx));
            }
        }
    }

    #[test]
    fn add_source_opens_sources_from_the_catalog_toolbar_in_every_theme() {
        use crate::player::options::{DEFAULT_GLASS_STRENGTH, InterfaceTheme};
        let ctx = Context::default();
        ctx.enable_accesskit();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated toolbar test"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.ready = true;
        for style in InterfaceTheme::ALL {
            theme::apply_style(&ctx, style, DEFAULT_GLASS_STRENGTH);
            let output = ctx.run_ui(catalog_input(vec![]), |ui| app.show_ui(ui));
            let nodes = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes;
            let bounds = |label| {
                nodes
                    .iter()
                    .find(|(_, n)| n.label() == Some(label))
                    .unwrap()
                    .1
                    .bounds()
                    .unwrap()
            };
            let add = bounds("Add source");
            let filters = bounds("Filters");
            assert_eq!(add.y1 - add.y0, filters.y1 - filters.y0);
            let center = egui::pos2(
                ((add.x0 + add.x1) / 2.) as f32,
                ((add.y0 + add.y1) / 2.) as f32,
            );
            output.drop_without_applying_deltas();
            ctx.run_ui(catalog_input(click_at(center, Modifiers::NONE)), |ui| {
                app.show_ui(ui)
            })
            .drop_without_applying_deltas();
            assert!(matches!(app.dialog.take(), Some(Dialog::Sources)));
            ctx.run_ui(catalog_input(vec![]), |ui| app.show_ui(ui))
                .drop_without_applying_deltas();
        }
    }

    #[test]
    fn playback_ready_preserves_window_size_with_legacy_resize_preferences() {
        let ctx = Context::default();
        let mut service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated playback"));
        let (send, receive) = crossbeam_channel::unbounded();
        service.events = receive;
        let mut app = MyTube::new(&ctx, None, false, service);
        for (id, width, height, fullscreen) in [(1, 3840, 2160, false), (2, 720, 1280, true)] {
            let preferences = serde_json::json!({
                "lock_aspect": true, "fullscreen_on_open": fullscreen,
                "window": {"resize": "Always", "scale": 0}
            });
            app.watch
                .restore_preferences(serde_json::from_value(preferences).unwrap());
            let mut video = catalog_video(id);
            video.media.width = Some(width);
            video.media.height = Some(height);
            app.playback_request = Some(RequestedVideo {
                key: video.thumbnail_key(),
                view: PlaybackView::FullPlayer,
            });
            send.send(Event::PlaybackReady(Box::new(video), None))
                .unwrap();
            let mut input = catalog_input(vec![]);
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .monitor_size = Some(vec2(1920., 1080.));
            let output = ctx.run_ui(input, |ui| {
                app.events(ui.ctx());
                app.aspect_lock.update(
                    ui.ctx(),
                    &eframe::Frame::_new_kittest(),
                    app.watch.locked_aspect(),
                );
            });
            assert_eq!(app.watch.video().unwrap().id, id);
            assert!(
                !output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .iter()
                    .any(|c| matches!(
                        c,
                        egui::ViewportCommand::InnerSize(_)
                            | egui::ViewportCommand::Fullscreen(_)
                            | egui::ViewportCommand::Maximized(_)
                    ))
            );
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn choosing_another_video_expands_only_after_its_own_lookup_succeeds() {
        let ctx = Context::default();
        let mut service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated playback"));
        let (send, receive) = crossbeam_channel::unbounded();
        service.events = receive;
        let mut app = MyTube::new(&ctx, None, false, service);
        app.watch.open(catalog_video(1), None, &app.service);
        app.watch.minimize();
        for id in [2, 3] {
            app.play(catalog_video(id), PlaybackView::FullPlayer);
            assert!(app.watch.mini());
        }
        send.send(Event::PlaybackReady(Box::new(catalog_video(2)), None))
            .unwrap();
        app.events(&ctx);
        assert!(app.watch.mini());
        assert_eq!(app.watch.video().unwrap().id, 1);
        send.send(Event::PlaybackReady(Box::new(catalog_video(3)), None))
            .unwrap();
        app.events(&ctx);
        assert!(!app.watch.mini());
        assert_eq!(app.watch.video().unwrap().id, 3);
    }

    #[test]
    fn resizing_playback_keeps_settings_at_the_window_edge() {
        use crate::player::options::{FilterPreset, InterfaceTheme, PlayerPreferences, Section};
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated playback"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.ready = true;
        app.watch.restore_preferences(PlayerPreferences {
            interface_theme: InterfaceTheme::Classic,
            reduce_motion: true,
            filters: (1..=12)
                .map(|id| FilterPreset {
                    id,
                    name: format!("Preset {id}"),
                    expression: "hflip".into(),
                    audio: false,
                })
                .collect(),
            ..Default::default()
        });
        app.watch.open(catalog_video(1), None, &app.service);
        app.watch.settings_panel.section = Section::Filters;
        app.watch.toggle_settings();

        let draw = |app: &mut MyTube, width, height, events| {
            let mut input = catalog_input(events);
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(width, height),
            ));
            ctx.run_ui(input, |ui| app.show_ui(ui))
        };
        draw(&mut app, 900., 660., vec![]).drop_without_applying_deltas();
        let rendered = draw(&mut app, 900., 660., vec![]);
        let field = rendered
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Filter name" => {
                    Some(text.pos + text.galley.size() / 2.)
                }
                _ => None,
            })
            .expect("filter name field");
        rendered.drop_without_applying_deltas();
        draw(&mut app, 900., 660., click_at(field, Modifiers::NONE)).drop_without_applying_deltas();
        let focus = ctx
            .memory(|memory| memory.focused())
            .expect("focused filter field");
        let initial = app.watch.settings_panel.sidebar.unwrap();
        let final_preset_visible = |output: &egui::FullOutput| {
            output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Preset 12" => shape
                    .clip_rect
                    .contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())),
                _ => false,
            })
        };
        for (width, height) in [(740., 660.), (1010., 660.), (1010., 390.), (1010., 760.)] {
            draw(&mut app, width, height, vec![]).drop_without_applying_deltas();
            assert!(app.watch.settings_panel.open);
            assert!(app.watch.settings_panel.section == Section::Filters);
            assert_eq!(ctx.memory(|memory| memory.focused()), Some(focus));
            let panel = app.watch.settings_panel.sidebar.unwrap();
            assert_eq!(panel.top(), initial.top());
            assert_eq!(height - panel.bottom(), 660. - initial.bottom());
            assert_eq!(panel.right(), width - theme::PAGE_SCROLL_GUTTER - 16.);
            assert!(panel.width() <= width * 0.52);
            if height == 390. {
                let before_scroll = draw(&mut app, width, height, vec![]);
                assert!(!final_preset_visible(&before_scroll));
                before_scroll.drop_without_applying_deltas();
                draw(
                    &mut app,
                    width,
                    height,
                    vec![
                        egui::Event::PointerMoved(panel.center_bottom() - vec2(0., 25.)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: vec2(0., -4000.),
                            phase: egui::TouchPhase::Move,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                )
                .drop_without_applying_deltas();
                for _ in 0..16 {
                    draw(&mut app, width, height, vec![]).drop_without_applying_deltas();
                }
                let after_scroll = draw(&mut app, width, height, vec![]);
                assert!(final_preset_visible(&after_scroll));
                after_scroll.drop_without_applying_deltas();
            }
        }
        app.watch.toggle_settings();
        draw(&mut app, 1010., 760., vec![]).drop_without_applying_deltas();
        assert!(!app.watch.settings_panel.open);
    }

    #[test]
    fn sidebar_footer_toggle_keeps_bottom_padding_in_both_states() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated sidebar test"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.ready = true;

        for (expanded, label) in [(true, "Collapse sidebar"), (false, "Expand sidebar")] {
            assert_eq!(app.sidebar_open, expanded);
            let input = catalog_input(vec![]);
            let bottom = f64::from(input.screen_rect.unwrap().bottom());
            let output = ctx.run_ui(input, |ui| app.sidebar(ui));
            let nodes = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes;
            let bounds = |label| {
                nodes
                    .iter()
                    .find_map(|(_, node)| {
                        (node.label() == Some(label))
                            .then(|| node.bounds())
                            .flatten()
                    })
                    .unwrap_or_else(|| panic!("Missing sidebar button {label}"))
            };
            let toggle = bounds(label);
            assert!(bounds("Manage sources").y1 < bounds("Settings").y0);
            assert!(bounds("Settings").y1 < toggle.y0);
            assert_eq!(bounds("Settings").x1, toggle.x1);
            assert!(bottom - toggle.y1 >= 16.0);
            let center = egui::pos2(
                ((toggle.x0 + toggle.x1) / 2.0) as f32,
                ((toggle.y0 + toggle.y1) / 2.0) as f32,
            );
            output.drop_without_applying_deltas();
            ctx.run_ui(catalog_input(click_at(center, Modifiers::NONE)), |ui| {
                app.sidebar(ui)
            })
            .drop_without_applying_deltas();
            assert_ne!(app.sidebar_open, expanded);
        }
    }

    #[test]
    fn player_gear_waits_for_sidebar_bounds_before_showing_any_settings() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated player"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.watch.restore_preferences(PlayerPreferences {
            interface_theme: InterfaceTheme::LiquidGlass,
            ..Default::default()
        });
        app.watch.open(catalog_video(1), None, &app.service);
        ctx.enable_accesskit();
        app.watch.settings_panel.open = true;
        for docked in [false, true] {
            let output = ctx.run_ui(catalog_input(vec![]), |ui| {
                if docked {
                    app.watch.prepare_sidebars(ui, false);
                }
                app.watch
                    .preferences_frame(&app.service, &mut app.appearance);
            });
            let contains_settings = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Playback"));
            assert_eq!(contains_settings, docked);
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn settings_shortcut_opens_all_sections_and_accepts_pointer_input() {
        use crate::player::options::Section;
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated settings test"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.watch.restore_preferences(PlayerPreferences {
            interface_theme: InterfaceTheme::LiquidGlass,
            ..Default::default()
        });
        app.ready = true;
        ctx.enable_accesskit();
        let render = |app: &mut MyTube, events| {
            let output = ctx.run_ui(catalog_input(events), |ui| app.show_ui(ui));
            let nodes = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .filter_map(|(_, node)| Some((node.label()?.to_owned(), node.bounds()?)))
                .collect::<Vec<_>>();
            output.drop_without_applying_deltas();
            nodes
        };
        render(&mut app, vec![]);
        let mut command = key(egui::Key::Comma, true);
        if let egui::Event::Key { modifiers, .. } = &mut command {
            *modifiers = Modifiers::COMMAND;
        }
        render(&mut app, vec![command]);
        assert!(app.watch.settings_panel.open);
        let click_tab = |app: &mut MyTube, label: &str| {
            let nodes = render(app, vec![]);
            let (_, bounds) = nodes
                .iter()
                .rfind(|(name, _)| name == label)
                .unwrap_or_else(|| panic!("Missing settings tab {label}"));
            let center = egui::pos2(
                ((bounds.x0 + bounds.x1) / 2.) as f32,
                ((bounds.y0 + bounds.y1) / 2.) as f32,
            );
            render(app, click_at(center, Modifiers::NONE));
        };
        for tab in Section::ALL {
            click_tab(&mut app, tab.title());
            assert_eq!(app.watch.settings_panel.section.title(), tab.title());
        }
    }

    #[test]
    fn i_toggles_the_same_video_once_and_leaves_dialog_input_alone() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.watch.open(catalog_video(17), None, &app.service);
        let run = |app: &mut MyTube, events| {
            ctx.run_ui(catalog_input(events), |_| {
                app.playback_and_clipboard_shortcuts(&ctx)
            })
            .drop_without_applying_deltas();
        };
        run(&mut app, vec![key(egui::Key::I, true)]);
        assert!(app.watch.mini());
        run(&mut app, vec![key(egui::Key::I, true)]);
        assert!(app.watch.mini());
        run(
            &mut app,
            vec![key(egui::Key::I, false), key(egui::Key::I, true)],
        );
        assert!(!app.watch.mini());
        assert_eq!(app.watch.video().map(|video| video.id), Some(17));

        app.dialog = Some(Dialog::Sources);
        run(
            &mut app,
            vec![key(egui::Key::I, false), key(egui::Key::I, true)],
        );
        assert!(!app.watch.mini());
    }

    #[test]
    fn copying_a_word_in_the_video_details_title_uses_the_text_selection() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        let video = catalog_video(17);
        app.selection.insert(video.id);
        app.detail = Some(video.clone());
        let mut render = |events| {
            ctx.run_ui(catalog_input(events), |ui| {
                app.playback_and_clipboard_shortcuts(&ctx);
                app.detail_panel(ui);
            })
        };
        let output = render(vec![]);
        let title = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == video.title => Some(text),
                _ => None,
            })
            .unwrap();
        let first_word = title.pos + vec2(5.0, title.galley.size().y / 2.0);
        output.drop_without_applying_deltas();
        for _ in 0..2 {
            render(click_at(first_word, Modifiers::NONE)).drop_without_applying_deltas();
        }
        let shortcut = egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        for event in [egui::Event::Copy, shortcut] {
            let output = render(vec![event]);
            assert_eq!(
                output.platform_output.commands,
                [egui::OutputCommand::CopyText("Video".into())]
            );
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn main_player_copy_writes_the_absolute_path_and_respects_text_focus() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        let mut video = catalog_video(1);
        video.path = std::env::current_dir()
            .unwrap()
            .join("fixture with spaces.mp4");
        app.watch.open(video.clone(), None, &app.service);
        let input = catalog_input(vec![egui::Event::Copy]);
        let result = ctx.run_ui(input.clone(), |_| {
            app.playback_and_clipboard_shortcuts(&ctx)
        });
        assert_eq!(
            result.platform_output.commands,
            [egui::OutputCommand::CopyText(
                video.path.display().to_string()
            )]
        );
        result.drop_without_applying_deltas();
        ctx.memory_mut(|memory| memory.request_focus(Id::new(SEARCH_ID)));
        let result = ctx.run_ui(input, |ui| {
            app.playback_and_clipboard_shortcuts(&ctx);
            ui.add(TextEdit::singleline(&mut app.filter.text).id(Id::new(SEARCH_ID)));
        });
        assert!(result.platform_output.commands.is_empty());
        result.drop_without_applying_deltas();
    }

    #[test]
    fn mini_player_picture_fills_its_area_in_each_appearance() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        app.watch.open(catalog_video(17), None, &app.service);
        app.watch.minimize();
        let render = |app: &mut MyTube, appearance, at| {
            theme::apply(&ctx, appearance);
            let mut input = catalog_input(vec![]);
            input.time = Some(at);
            ctx.run_ui(input, |ui| {
                ui.set_style(ctx.global_style());
                assert_eq!(ui.visuals().dark_mode, appearance == Appearance::Dark);
                app.watch.show_mini(
                    ui,
                    &watch::Queue::default(),
                    &mut app.thumbs,
                    &app.service,
                    false,
                );
            })
        };
        render(&mut app, Appearance::Dark, 1.0).drop_without_applying_deltas();
        for (appearance, at) in [(Appearance::Dark, 2.0), (Appearance::Light, 3.0)] {
            let output = render(&mut app, appearance, at);
            let area = ctx.memory(|m| m.area_rect(Id::new("mini_player"))).unwrap();
            assert!(output.shapes.iter().any(
                |shape| matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == Color32::BLACK && rect.rect == area && rect.stroke.is_empty()
                       && rect.corner_radius == egui::CornerRadius::same(12))
            ));
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn marquee_uses_empty_space_and_escape_preserves_the_original_selection() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        assert!(app.browser.applied(
            0,
            0,
            Some(PAGE + 10),
            (1..=PAGE as i64).map(catalog_video).collect(),
            false
        ));
        let paint = |app: &mut MyTube, events| {
            ctx.run_ui(catalog_input(events), |ui| {
                app.body(ui, &Progress::default())
            })
            .drop_without_applying_deltas();
        };
        paint(&mut app, vec![]);
        app.select_video(9, Modifiers::NONE);
        let press = app.catalog_viewport.unwrap().bounds.left_top() + vec2(4., 8.);
        let mut events = click_at(press, Modifiers::SHIFT);
        events.pop();
        paint(&mut app, events);
        assert!(app.marquee.is_some());
        paint(
            &mut app,
            vec![egui::Event::PointerMoved(egui::pos2(600.0, 640.0))],
        );
        assert!(
            app.marquee
                .as_ref()
                .is_some_and(|drag| drag.dragged && drag.mode == SelectionMode::Add)
        );
        paint(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            }],
        );
        assert!(app.marquee.is_none());
        assert_eq!(app.selection, HashSet::from([9]));
        assert!(app.range_request.is_none());
    }

    #[test]
    fn marquee_releases_across_pages_in_grid_and_list() -> anyhow::Result<()> {
        let data = tempfile::tempdir()?;
        let path = data.path().join("library.sqlite3");
        let mut db = crate::catalog::open(&path)?;
        crate::catalog::initialize(&db)?;
        db.execute("UPDATE library_preferences SET scan_drives=0", [])?;
        let total = PAGE + 10;
        for index in (0..total).rev() {
            let clip = data.path().join(format!("Clip {index:03}.mp4"));
            std::fs::write(&clip, b"test catalog entry")?;
            crate::catalog::upsert(&db, &clip, "mp4", Stamp::read(&clip)?, None, index as i64)?;
        }
        db.execute("UPDATE videos SET pinned=1,probe=1,codec='h264'", [])?;
        let filter = Filter {
            sort: Sort::Title,
            ..Filter::default()
        };
        let (_, first_page) = crate::catalog::page(&mut db, &filter, 0, true)?;
        let (_, second_page) = crate::catalog::page(&mut db, &filter, 1, false)?;
        assert_eq!(first_page.len(), PAGE);
        assert_eq!(second_page.len(), 10);
        let last = second_page.last().unwrap().id;
        let outside = first_page[0].id;
        let toggle = if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL
        };

        for list in [false, true] {
            let ctx = Context::default();
            let path = path.clone();
            let service = Service::start_with(ctx.clone(), move || {
                let lock = tempfile::tempfile()?;
                Ok((path, lock))
            });
            assert!(matches!(
                service.events.recv_timeout(Duration::from_secs(5))?,
                Event::Ready(..)
            ));
            let mut app = MyTube::new(&ctx, None, false, service);
            app.view.list = list;
            app.filter = filter.clone();
            app.apply_filter();
            assert!(
                app.browser
                    .applied(1, 0, Some(total), first_page.clone(), false)
            );
            let mut time = 0.0;
            let mut frame = |app: &mut MyTube, events, drain_events| {
                time += 0.05;
                let mut input = catalog_input(events);
                input.time = Some(time);
                let output = ctx.run_ui(input, |ui| {
                    if drain_events {
                        app.events(ui.ctx());
                    }
                    app.show_ui(ui);
                });
                let accent = library::selection_color(&ctx.global_style().visuals);
                let highlight = output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.fill == accent.gamma_multiply(0.16))
                });
                output.drop_without_applying_deltas();
                highlight
            };
            frame(&mut app, vec![], false);
            let initial = app.catalog_viewport.unwrap();
            let start_index = PAGE - 2 * initial.columns;
            let intersected = || {
                first_page[start_index..]
                    .iter()
                    .chain(&second_page)
                    .map(|video| video.id)
                    .collect::<HashSet<_>>()
            };

            for (modifiers, cancel) in [
                (Modifiers::NONE, false),
                (Modifiers::SHIFT, false),
                (toggle, false),
                (toggle, true),
            ] {
                let base = HashSet::from([outside, last]);
                app.selection.clone_from(&base);
                app.catalog_scroll =
                    Some(CatalogScroll::RestoreOffset(initial.row_top(start_index)));
                frame(&mut app, vec![], false);
                frame(&mut app, vec![egui::Event::Text("/".into())], false);
                assert!(ctx.memory(|memory| memory.has_focus(Id::new(SEARCH_ID))));
                let viewport = app.catalog_viewport.unwrap();
                let press = viewport.bounds.left_top() + vec2(7.0, 3.0);
                let edge = viewport.bounds.right_bottom() - vec2(7.0, 1.0);
                let mut press_events = click_at(press, modifiers);
                let release = click_at(edge, Modifiers::NONE).pop().unwrap();
                press_events.pop();
                frame(&mut app, press_events, false);
                assert!(
                    app.marquee.is_some(),
                    "gutter press with search focused, list={list}"
                );
                assert!(frame(
                    &mut app,
                    vec![egui::Event::PointerMoved(edge)],
                    false
                ));
                for _ in 0..24 {
                    frame(&mut app, vec![], false);
                }
                let scrolled = app.catalog_viewport.unwrap();
                assert!(scrolled.offset > viewport.offset);
                assert!(scrolled.shows(total - 1));
                assert_eq!(app.selection, base);
                frame(&mut app, vec![release], false);
                assert!(app.marquee.is_none());
                assert!(app.range_request.is_some());
                assert!(frame(&mut app, vec![], false));

                if cancel {
                    frame(&mut app, vec![key(egui::Key::Escape, true)], true);
                    assert!(app.range_request.is_none());
                    assert_eq!(app.selection, base);
                } else {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while app.range_request.is_some() && Instant::now() < deadline {
                        frame(&mut app, vec![], true);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    assert!(app.range_request.is_none());
                    let through_end = intersected();
                    let expected = if modifiers.shift {
                        through_end.union(&base).copied().collect()
                    } else if modifiers == toggle {
                        through_end.symmetric_difference(&base).copied().collect()
                    } else {
                        through_end
                    };
                    assert_eq!(
                        app.selection, expected,
                        "list={list}, modifiers={modifiers:?}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn two_finger_pan_selects_and_deselects_in_the_full_list_and_grid() {
        use egui::{Event as Input, TouchDeviceId, TouchId, TouchPhase, pos2};
        for list in [false, true] {
            let ctx = Context::default();
            let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
            assert!(matches!(
                service.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                Event::StartupFailed(_)
            ));
            let mut app = MyTube::new(&ctx, None, false, service);
            app.view.list = list;
            assert!(app.browser.applied(
                0,
                0,
                Some(16),
                (1..=16).map(catalog_video).collect(),
                false,
            ));
            let frame = |app: &mut MyTube, events| {
                let mut input = catalog_input(events);
                input.screen_rect = input.screen_rect.map(|rect| rect.with_max_y(830.0));
                app.read_catalog_pan(&ctx, &mut input);
                ctx.run_ui(input, |ui| {
                    app.events(&ctx);
                    app.show_ui(ui);
                })
                .drop_without_applying_deltas();
            };
            frame(&mut app, vec![]);
            let viewport = app.catalog_viewport.unwrap();
            let x = viewport.bounds.left() + CATALOG_GUTTER + 30.0;
            let y = viewport.bounds.top() + 16.0;
            let points = [pos2(x - 12.0, y), pos2(x + 12.0, y + 4.0)];
            let travel = vec2(0.0, viewport.stride * 2.0);
            let touch = |id, phase, pos| Input::Touch {
                device_id: TouchDeviceId(1),
                id: TouchId(id),
                phase,
                pos,
                force: None,
            };
            let pointer = |pos, pressed| Input::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let expected = HashSet::from([
                1,
                (viewport.columns + 1) as i64,
                (2 * viewport.columns + 1) as i64,
            ]);
            for deselect in [false, true] {
                frame(
                    &mut app,
                    vec![
                        touch(1, TouchPhase::Start, points[0]),
                        Input::PointerMoved(points[0]),
                        pointer(points[0], true),
                        touch(2, TouchPhase::Start, points[1]),
                    ],
                );
                assert_eq!(app.pan_mode, deselect);
                frame(
                    &mut app,
                    vec![
                        touch(1, TouchPhase::Move, points[0] + travel),
                        Input::PointerMoved(points[0] + travel),
                        touch(2, TouchPhase::Move, points[1] + travel),
                    ],
                );
                assert!(app.pan_mode);
                assert_eq!(
                    app.selection,
                    if deselect {
                        HashSet::new()
                    } else {
                        expected.clone()
                    }
                );
                frame(
                    &mut app,
                    vec![
                        touch(2, TouchPhase::End, points[1] + travel),
                        touch(1, TouchPhase::End, points[0] + travel),
                        pointer(points[0] + travel, false),
                        Input::PointerGone,
                    ],
                );
                assert!(app.pan_selection.is_none());
                assert!(app.detail.is_none());
            }
            frame(&mut app, vec![key(egui::Key::Escape, true)]);
            assert!(!app.pan_mode);
        }
    }

    #[test]
    fn full_catalog_drags_capture_either_gutter_row_gaps_and_space_below() {
        for list in [false, true] {
            let ctx = Context::default();
            let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
            let mut app = MyTube::new(&ctx, None, false, service);
            app.view.list = list;
            let total = if list { 2 } else { 5 };
            assert!(app.browser.applied(
                0,
                0,
                Some(total),
                (1..=total as i64).map(catalog_video).collect(),
                false,
            ));
            let frame = |app: &mut MyTube, events| {
                let mut input = catalog_input(events);
                input.screen_rect = input.screen_rect.map(|rect| rect.with_max_y(830.0));
                ctx.run_ui(input, |ui| app.show_ui(ui))
                    .drop_without_applying_deltas();
            };
            frame(&mut app, vec![]);
            let viewport = app.catalog_viewport.unwrap();
            let grid = marquee::Grid::catalog(app.view, viewport.bounds.width(), total);
            let bounds = viewport.bounds;
            let card = bounds.left_top() + vec2(CATALOG_GUTTER + 9.0, 20.0);
            let starts = [
                bounds.left_top() + vec2(7.0, 5.0),
                bounds.right_top() + vec2(-7.0, 5.0),
                card + vec2(0.0, grid.size.y + grid.gap * 0.5 - 20.0),
                bounds.left_top() + vec2(CATALOG_GUTTER + 9.0, grid.height() + 12.0),
            ];
            for press in starts {
                assert!(bounds.contains(press));
                let mut events = click_at(press, Modifiers::SHIFT);
                let release = events.pop().unwrap();
                frame(&mut app, events);
                assert!(app.marquee.is_some(), "list={list}, press={press:?}");
                frame(&mut app, vec![egui::Event::PointerMoved(card)]);
                assert!(app.marquee.as_ref().is_some_and(|drag| drag.dragged));
                frame(&mut app, vec![key(egui::Key::Escape, true)]);
                frame(&mut app, vec![release, key(egui::Key::Escape, false)]);
                assert!(app.marquee.is_none());
                assert!(app.range_request.is_none());
            }
            let mut events = click_at(card, Modifiers::NONE);
            events.pop();
            frame(&mut app, events);
            assert!(app.marquee.is_none());
        }
    }

    #[test]
    fn clicks_between_cards_or_below_videos_clear_selection() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        assert!(
            app.browser
                .applied(0, 0, Some(4), (1..=4).map(catalog_video).collect(), false)
        );
        let paint = |app: &mut MyTube, events| {
            ctx.run_ui(catalog_input(events), |ui| {
                app.body(ui, &Progress::default())
            })
            .drop_without_applying_deltas();
        };
        paint(&mut app, vec![]);
        app.select_video(1, Modifiers::NONE);
        app.select_video(2, Modifiers::CTRL);

        let bounds = app.catalog_viewport.unwrap().bounds;
        let grid = marquee::Grid::catalog(app.view, bounds.width(), 4);
        let gap = bounds.left_top() + vec2(grid.item_rect(0).right() + grid.gap * 0.5, 8.);
        paint(&mut app, click_at(gap, Modifiers::CTRL));
        assert_eq!(app.selection, HashSet::from([1, 2]));
        let third = bounds.left_top() + grid.item_rect(2).center().to_vec2();
        paint(&mut app, click_at(third, Modifiers::CTRL));
        assert_eq!(app.selection, HashSet::from([1, 2, 3]));
        paint(&mut app, click_at(gap, Modifiers::NONE));
        assert!(app.selection.is_empty());
        assert!(app.selection_anchor.is_none());

        app.view.list = true;
        paint(&mut app, vec![]);
        app.select_video(1, Modifiers::NONE);
        app.select_video(2, Modifiers::CTRL);
        paint(
            &mut app,
            click_at(egui::pos2(200.0, 480.0), Modifiers::NONE),
        );
        assert!(app.selection.is_empty());
    }

    #[test]
    fn delete_key_requests_the_selected_files() {
        assert_delete_requests_selected_files(egui::Key::Delete, Modifiers::NONE);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_delete_key_requests_the_selected_files() {
        assert_delete_requests_selected_files(egui::Key::Backspace, Modifiers::NONE);
        assert_delete_requests_selected_files(egui::Key::Backspace, Modifiers::COMMAND);
    }

    fn assert_delete_requests_selected_files(key: egui::Key, modifiers: Modifiers) {
        let ctx = Context::default();
        // Keep the worker's action queue open while checking the dialog it will populate.
        let (_hold, startup) = std::sync::mpsc::channel::<()>();
        let service = Service::start_with(ctx.clone(), move || {
            let _ = startup.recv();
            anyhow::bail!("no test library")
        });
        let mut app = MyTube::new(&ctx, None, false, service);
        assert!(
            app.browser
                .applied(0, 0, Some(3), (1..=3).map(catalog_video).collect(), false)
        );
        app.select_video(1, Modifiers::NONE);
        app.select_video(3, Modifiers::CTRL);
        let delete_key = egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        ctx.run_ui(catalog_input(vec![delete_key]), |ui| {
            app.body(ui, &Progress::default());
        })
        .drop_without_applying_deltas();
        assert!(matches!(
            app.dialog,
            Some(Dialog::ResolveFiles(1, FileIntent::Delete, 2))
        ));

        assert_eq!(app.selection, HashSet::from([1, 3]));
    }

    #[test]
    fn delete_dialog_keyboard_navigation_focuses_and_activates_buttons() {
        use egui::Key;

        let ctx = Context::default();
        let (_hold, startup) = std::sync::mpsc::channel::<()>();
        let service = Service::start_with(ctx.clone(), move || {
            let _ = startup.recv();
            anyhow::bail!("no test library")
        });
        let mut app = MyTube::new(&ctx, None, false, service);
        app.dialog = Some(Dialog::Delete(vec![catalog_video(1)]));
        let show = |app: &mut MyTube, events| {
            let mut focused_rect = None;
            let output = ctx.run_ui(catalog_input(events), |ui| {
                app.show_dialog(ui.ctx(), &Progress::default());
                focused_rect = ctx
                    .memory(|memory| memory.focused())
                    .and_then(|id| ctx.read_response(id))
                    .map(|response| response.rect);
            });
            (output, focused_rect)
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let expect_focus = |(output, rect): (egui::FullOutput, Option<egui::Rect>), label| {
            let rect = rect.unwrap();
            assert!(
                output.shapes.iter().any(|clipped| {
                    matches!(&clipped.shape, egui::Shape::Text(shape)
                    if shape.galley.text() == label
                        && rect.contains(shape.visual_bounding_rect().center()))
                }),
                "Expected keyboard focus on {label}"
            );
            output.drop_without_applying_deltas();
        };

        show(&mut app, vec![]).0.drop_without_applying_deltas();
        expect_focus(show(&mut app, vec![]), "Cancel");
        for (input, modifiers, label) in [
            (Key::Tab, Modifiers::NONE, "Delete Permanently"),
            (Key::Tab, Modifiers::SHIFT, "Cancel"),
            (Key::ArrowLeft, Modifiers::NONE, "Move to Trash"),
            (Key::ArrowRight, Modifiers::NONE, "Cancel"),
            (Key::ArrowRight, Modifiers::NONE, "Delete Permanently"),
        ] {
            expect_focus(show(&mut app, vec![key(input, modifiers)]), label);
        }
        show(&mut app, vec![key(Key::Enter, Modifiers::NONE)])
            .0
            .drop_without_applying_deltas();
        assert!(matches!(
            app.dialog,
            Some(Dialog::ConfirmPermanentDelete(_))
        ));
        assert!(app.file_job.is_none());

        show(&mut app, vec![]).0.drop_without_applying_deltas();
        expect_focus(show(&mut app, vec![]), "Cancel");
        expect_focus(
            show(&mut app, vec![key(Key::ArrowRight, Modifiers::NONE)]),
            "Delete Permanently",
        );
        show(&mut app, vec![key(Key::Enter, Modifiers::NONE)])
            .0
            .drop_without_applying_deltas();
        assert!(app.dialog.is_none());
        assert!(app.file_job.is_some());
    }

    #[test]
    fn details_reflow_keeps_the_catalog_in_place() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("no test library"));
        let mut app = MyTube::new(&ctx, None, false, service);
        let mut videos: Vec<_> = (1..=PAGE as i64).map(catalog_video).collect();
        assert!(app.browser.applied(0, 0, Some(PAGE), videos.clone(), false));
        let paint = |app: &mut MyTube| {
            ctx.run_ui(catalog_input(vec![]), |ui| {
                app.body(ui, &Progress::default());
            })
            .drop_without_applying_deltas();
            app.catalog_viewport.unwrap()
        };

        let index = 62;
        let selected = videos[index].clone();
        app.catalog_scroll = Some(CatalogScroll::Reveal(selected.id));
        let wide = paint(&mut app);
        assert!(wide.offset > 0.0 && wide.shows(index));

        app.inspect(selected.clone());
        let narrow = paint(&mut app);
        assert!(narrow.columns < wide.columns);
        assert!(narrow.offset > wide.offset);
        // The selected row just fits at the bottom after opening the panel.
        assert!((narrow.relative_top(index) + narrow.row_height - narrow.height).abs() < 1.0);

        app.catalog_scroll = Some(CatalogScroll::RestoreOffset(narrow.offset + 123.25));
        let scrolled = paint(&mut app);
        assert!(scrolled.offset > narrow.offset);
        app.inspect(videos[index + 1].clone());
        paint(&mut app);

        app.close_details(None);
        let closed = paint(&mut app);
        assert_eq!(closed.columns, wide.columns);
        assert_eq!(closed.offset, wide.offset);

        app.inspect(selected.clone());
        let before_delete = paint(&mut app);
        assert!(app.begin_detail_removal(&[selected.id]));
        let (_, anchor) = app.detail_removal.unwrap();
        assert_eq!(anchor.id, videos[index + 1].id);
        assert!((anchor.top - before_delete.relative_top(index + 1)).abs() < 1.0);
        videos.remove(index);
        assert!(app.browser.applied(1, 0, Some(videos.len()), videos, true));
        app.finish_detail_removal(Some(index));

        let after_delete = paint(&mut app);
        assert!(app.detail.is_none());
        assert_eq!(after_delete.columns, wide.columns);
        assert_eq!(app.browser.get(index).unwrap().id, anchor.id);
        assert!((after_delete.relative_top(index) - anchor.top).abs() < 1.0);
    }
}
