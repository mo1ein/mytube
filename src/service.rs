use crate::player::options::PlayerPreferences;
use std::{
    fs::{self, File},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, tick, unbounded};
use eframe::egui::{ColorImage, Context as Gui};
use rusqlite::Connection;

use crate::{
    catalog, files, media,
    model::{
        Appearance, Collection, CollectionTarget, ControlBarLayout, Filter, LibraryView, PAGE,
        ProgressBarPosition, Source, Stamp, Video, video_ext,
    },
    scan,
};

#[derive(Default)]
pub struct Settings {
    pub auto_scan: bool,
    pub sources: Vec<Source>,
    pub exclusions: Vec<Source>,
    pub collections: Vec<Collection>,
    pub watch_later: std::collections::HashSet<i64>,
    pub view: LibraryView,
    pub appearance: Appearance,
    pub player: PlayerPreferences,
    pub control_bar_layout: ControlBarLayout,
    pub progress_bar_position: ProgressBarPosition,
}

pub enum Action {
    AutoScan(bool),
    AddSource(PathBuf, bool),
    RemoveSource(i64, bool),
    AddToCollection(CollectionTarget, Vec<i64>, Option<i64>),
    Member(i64, i64, bool),
    WatchLater(i64, bool),
    SavePlayerSettings(PlayerPreferences),
    RecordPlay(i64),
    ClearPlaybackHistory,
    History,
    ExportPlaylist(Filter, PathBuf),
    SaveView(LibraryView),
    SaveAppearance(Appearance),
    SaveControlBarLayout(ControlBarLayout),
    SaveProgressBarPosition(ProgressBarPosition),
    Drives,
    CopyPaths(Vec<i64>),
    Inspect(i64),
    Reveal(PathBuf),
    SelectFiles(u64, Vec<i64>),
    ManageFiles(Vec<Video>, files::Operation),
}

pub enum Event {
    Ready(Settings, Vec<&'static str>, catalog::SearchPage),
    StartupFailed(String),
    Settings(Settings),
    WatchLater(i64, bool),
    Changed,
    VideosMissing(Vec<i64>),
    PlaybackReady(Box<Video>, Option<f64>),
    PlaybackUnavailable((i64, i64)),
    Page {
        request: u64,
        index: usize,
        total: Option<usize>,
        rows: Vec<Video>,
        anchor_position: Option<usize>,
    },
    PageFailed(u64, String),
    SearchPath(u64, SearchPathStatus),
    Validating(bool),
    Detail(i64, Option<Box<Video>>, Vec<i64>),
    Thumbnail((i64, i64), Option<ColorImage>),
    Preview((i64, i64), usize, Option<ColorImage>),
    Timeline(TimelineOrigin, TimelineKey, Option<ColorImage>, bool),
    TimelinePrefetched(TimelineOrigin, TimelineKey, ColorImage),
    History(Vec<Video>),
    Drives(Vec<PathBuf>),
    Subfolders(u64, Result<Vec<PathBuf>, String>),
    CopiedPaths(Result<String, String>),
    Picked(Option<PathBuf>),
    Message(String, bool),
    SelectionRange(u64, Result<Vec<i64>, String>),
    SelectedFiles(u64, Result<Vec<Video>, String>),
    FileProgress(usize),
    FilesDone(FileReport),
}

#[derive(Clone, Copy)]
pub enum SearchPathStatus {
    Unsupported,
    Checking,
    OutsideLibrary,
}

pub struct FileReport {
    pub succeeded: Vec<i64>,
    pub failed: Vec<(PathBuf, String)>,
    pub label: &'static str,
    pub keep_selection: bool,
    pub deleted: bool,
}

pub struct SelectionRange {
    pub id: u64,
    pub filter: Filter,
    pub target: SelectionTarget,
}

pub enum SelectionTarget {
    Between(i64, i64),
    Region(catalog::CatalogRegion),
    Positions(Vec<usize>),
}

pub type TimelineKey = (i64, i64, i64);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimelineOrigin {
    Player,
    Library,
    Precise(u64),
}

impl TimelineOrigin {
    fn index(self) -> usize {
        match self {
            Self::Library => 0,
            Self::Player | Self::Precise(_) => 1,
        }
    }
}

pub struct TimelineRequest {
    pub key: TimelineKey,
    pub video: Video,
    pub at: f64,
    pub has_preview: bool,
    pub neighbors: Vec<i64>,
    pub origin: TimelineOrigin,
}

pub struct TimelinePrefetch {
    pub video: Video,
    pub duration: f64,
    pub remote: bool,
    pub focus: f64,
}

pub struct PreviewRequest {
    pub video: Video,
    pub first_segment: usize,
}

pub struct Search {
    pub id: u64,
    pub filter: Filter,
    pub index: usize,
    pub count: bool,
    pub anchor: Option<i64>,
}

#[derive(Clone)]
struct Emitter {
    sender: Sender<Event>,
    gui: Gui,
}

impl Emitter {
    fn send(&self, event: Event) {
        if self.sender.send(event).is_ok() {
            self.gui.request_repaint();
        }
    }
    fn error(&self, error: anyhow::Error) {
        self.send(Event::Message(format!("{error:#}"), true));
    }
}

struct Latest<T> {
    next: Arc<Mutex<Option<T>>>,
    wake: Sender<()>,
}

impl<T> Latest<T> {
    fn channel() -> (Self, Inbox<T>) {
        let next = Arc::new(Mutex::new(None));
        let (wake, rx) = bounded(1);
        (
            Self {
                next: next.clone(),
                wake,
            },
            Inbox { next, rx },
        )
    }

    fn replace(&self, next: T) {
        *self.next.lock().unwrap() = Some(next);
        let _ = self.wake.try_send(());
    }
}

struct Inbox<T> {
    next: Arc<Mutex<Option<T>>>,
    rx: Receiver<()>,
}

impl<T> Inbox<T> {
    fn take(&self) -> Option<T> {
        self.next.lock().unwrap().take()
    }
}

pub struct Service {
    pub events: Receiver<Event>,
    pub scan: Arc<scan::Control>,
    scan_wake: Sender<()>,
    actions: Sender<Action>,
    searches: Latest<Search>,
    visible_videos: Latest<Vec<Video>>,
    playback: Latest<Video>,
    checkpoints: Sender<Checkpoint>,
    folders: Latest<(u64, PathBuf)>,
    ranges: Latest<SelectionRange>,
    thumbnails: Latest<Vec<Video>>,
    previews: Latest<Option<PreviewRequest>>,
    timeline: Latest<Option<TimelineRequest>>,
    timeline_prefetch: [Latest<Option<TimelinePrefetch>>; 2],
    emit: Emitter,
}

enum Checkpoint {
    Save((i64, i64), Option<f64>),
    Flush(Sender<()>),
}

impl Service {
    pub fn preload() -> impl FnOnce(Gui) -> Self {
        // Database upgrade and the first page can run while the window system
        // initializes. Only the service worker waits for the database thread.
        let opening = thread::spawn(|| open_library(prepare));
        move |gui| {
            Self::start_prepared(gui, || {
                opening
                    .join()
                    .map_err(|_| anyhow::anyhow!("Library startup stopped"))?
            })
        }
    }

    pub fn start(gui: Gui) -> Self {
        Self::start_with(gui, prepare)
    }

    pub(super) fn start_with(
        gui: Gui,
        prepare: impl FnOnce() -> Result<(PathBuf, File)> + Send + 'static,
    ) -> Self {
        Self::start_prepared(gui, move || open_library(prepare))
    }

    fn start_prepared(
        gui: Gui,
        open: impl FnOnce() -> Result<OpenedLibrary> + Send + 'static,
    ) -> Self {
        let (sender, events) = unbounded();
        let emit = Emitter { sender, gui };
        let startup = emit.clone();
        let scan = Arc::new(scan::Control::default());
        let control = scan.clone();
        let (scan_wake, scans) = bounded(1);
        let rescan = scan_wake.clone();
        let (actions, action_rx) = bounded(64);
        let (searches, search_rx) = Latest::channel();
        let (visible_videos, visible_rx) = Latest::channel();
        let (playback, playback_rx) = Latest::channel();
        let (checkpoints, checkpoint_rx) = unbounded();
        let (folders, folder_rx) = Latest::<(u64, PathBuf)>::channel();
        let (ranges, range_rx) = Latest::channel();
        let (thumbnails, thumb_rx) = Latest::channel();
        let (previews, preview_rx) = Latest::channel();
        let (timeline, timeline_rx) = Latest::channel();
        let (library_prefetch, library_prefetch_rx) = Latest::channel();
        let (player_prefetch, player_prefetch_rx) = Latest::channel();

        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let OpenedLibrary {
                    path,
                    lock,
                    db,
                    settings: initial,
                    page,
                } = open()?;
                let tools = media::Tools::find();
                let (probe_wake, probe_rx) = bounded(1);
                let writes = Arc::new(Mutex::new(()));
                let search_db = catalog::open(&path)?;
                let emitter = startup.clone();
                let search_probe = tools.ffprobe.clone();
                thread::spawn(move || {
                    search_loop(search_db, search_rx, range_rx, search_probe, emitter)
                });
                startup.send(Event::Ready(initial, tools.missing(), page));

                let file_db = catalog::open(&path)?;
                let presence_db = catalog::open(&path)?;
                let thumb_db = catalog::open(&path)?;
                let preview_db = catalog::open(&path)?;
                let timeline_db = catalog::open(&path)?;
                timeline_db.busy_timeout(Duration::from_millis(25))?;
                let probe_db = catalog::open(&path)?;

                let emitter = startup.clone();
                let file_writes = writes.clone();
                let file_control = control.clone();
                let transfer_probe = probe_wake.clone();
                thread::spawn(move || {
                    // Keep the per-library lock until the application drops the action queue.
                    let _lock = lock;
                    action_loop(
                        file_db,
                        file_writes,
                        file_control,
                        rescan,
                        action_rx,
                        emitter,
                        transfer_probe,
                    );
                });
                let emitter = startup.clone();
                let presence_writes = writes.clone();
                thread::spawn(move || {
                    presence_loop(
                        presence_db,
                        &presence_writes,
                        visible_rx,
                        playback_rx,
                        checkpoint_rx,
                        emitter,
                    )
                });
                let emitter = startup.clone();
                thread::spawn(move || {
                    while folder_rx.rx.recv().is_ok() {
                        if let Some((request, path)) = folder_rx.take() {
                            let result = subfolders(&path).map_err(|error| format!("{error:#}"));
                            emitter.send(Event::Subfolders(request, result));
                        }
                    }
                });
                let emitter = startup.clone();
                let preview_tool = tools.ffmpeg.clone();
                thread::spawn(move || preview_loop(preview_db, preview_tool, preview_rx, emitter));
                let emitter = startup.clone();
                let timeline_tool = tools.ffmpeg.clone();
                thread::spawn(move || {
                    timeline_loop(
                        timeline_db,
                        timeline_tool,
                        timeline_rx,
                        library_prefetch_rx,
                        player_prefetch_rx,
                        emitter,
                    )
                });
                let emitter = startup.clone();
                thread::spawn(move || thumbnail_loop(thumb_db, tools.ffmpeg, thumb_rx, emitter));
                let emitter = startup.clone();
                thread::spawn(move || probe_loop(probe_db, tools.ffprobe, probe_rx, emitter));

                let mut db = db;
                let mut previous = None;
                let mut scan_once = |requested: bool| {
                    let generation = control.generation();
                    if requested && previous == Some(generation) {
                        return;
                    }
                    let force = previous != Some(generation) && (requested || generation != 0);
                    previous = Some(generation);
                    let refresh = (|| -> Result<()> {
                        if catalog::auto_scan(&db)? {
                            let drives = scan::discover_drives();
                            let _guard = writes.lock().unwrap();
                            if control.generation() != generation {
                                return Ok(());
                            }
                            catalog::register_drives(&mut db, &drives)?;
                            startup.send(Event::Settings(settings(&db)?));
                        }
                        scan::refresh(&mut db, &control, &writes, generation, force, |changed| {
                            if changed {
                                let _ = probe_wake.try_send(());
                                startup.send(Event::Changed);
                            } else {
                                startup.gui.request_repaint();
                            }
                        })
                    })();
                    if let Err(error) = refresh {
                        startup.error(error.context("Scan could not finish"));
                    }
                };
                let _ = probe_wake.try_send(());
                scan_once(false);
                loop {
                    match scans.recv_timeout(scan::CACHE_TTL) {
                        Ok(()) => scan_once(true),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => scan_once(false),
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                startup.send(Event::StartupFailed(format!("{error:#}")));
            }
        });

        Self {
            events,
            scan,
            scan_wake,
            actions,
            searches,
            visible_videos,
            playback,
            checkpoints,
            folders,
            ranges,
            thumbnails,
            previews,
            timeline,
            timeline_prefetch: [library_prefetch, player_prefetch],
            emit,
        }
    }

    pub fn rescan(&self) {
        self.scan.restart();
        let _ = self.scan_wake.try_send(());
    }

    pub fn select_range(&self, range: SelectionRange) {
        self.ranges.replace(range);
    }

    pub fn send(&self, action: Action) -> bool {
        match self.actions.try_send(action) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.emit.send(Event::Message(
                    "The file queue is busy. Try again shortly.".into(),
                    true,
                ));
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.emit
                    .send(Event::Message("The library is unavailable.".into(), true));
                false
            }
        }
    }

    pub fn search(&self, request: Search) {
        self.searches.replace(request);
    }
    pub fn watch_visible(&self, videos: Vec<Video>) {
        self.visible_videos.replace(videos);
    }
    pub fn play(&self, video: Video) {
        self.playback.replace(video);
    }
    pub fn save_position(&self, key: (i64, i64), at: Option<f64>) {
        let _ = self.checkpoints.send(Checkpoint::Save(key, at));
    }
    pub fn flush_positions(&self) {
        let (ack, done) = bounded(1);
        if self.checkpoints.send(Checkpoint::Flush(ack)).is_ok() {
            let _ = done.recv();
        }
    }
    pub fn list_subfolders(&self, request: u64, path: PathBuf) {
        self.folders.replace((request, path));
    }
    pub fn thumbnails(&self, videos: Vec<Video>) {
        self.thumbnails.replace(videos);
    }
    pub fn preview(&self, request: Option<PreviewRequest>) {
        self.previews.replace(request);
    }
    pub fn timeline_preview(&self, request: Option<TimelineRequest>) {
        self.timeline.replace(request);
    }

    pub fn prefetch_timeline(&self, origin: TimelineOrigin, request: Option<TimelinePrefetch>) {
        self.timeline_prefetch[origin.index()].replace(request);
    }

    pub fn save_playlist_dialog(&self, filter: Filter) {
        let actions = self.actions.clone();
        thread::spawn(move || {
            let file = rfd::FileDialog::new()
                .set_title("Save current playlist")
                .add_filter("M3U8 playlist", &["m3u8"])
                .set_file_name("playlist.m3u8")
                .save_file();
            if let Some(path) = file {
                let _ = actions.send(Action::ExportPlaylist(filter, path));
            }
        });
    }

    pub fn pick_folder(&self) {
        let emit = self.emit.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .set_title("Choose a folder")
                .pick_folder();
            emit.send(Event::Picked(path));
        });
    }
}

struct OpenedLibrary {
    path: PathBuf,
    lock: File,
    db: Connection,
    settings: Settings,
    page: catalog::SearchPage,
}

fn open_library(prepare: impl FnOnce() -> Result<(PathBuf, File)>) -> Result<OpenedLibrary> {
    let (path, lock) = prepare()?;
    let mut db = catalog::open(&path)?;
    let settings = settings(&db)?;
    let page = catalog::search_page(&mut db, &Filter::default(), 0, true, None)?;
    Ok(OpenedLibrary {
        path,
        lock,
        db,
        settings,
        page,
    })
}

fn prepare() -> Result<(PathBuf, File)> {
    if let Some(path) = crate::setting("DATA_DIR") {
        return prepare_at(Path::new(&path));
    }
    let directory = directories::ProjectDirs::from("app", "MyTube", "MyTube")
        .context("Could not find your application data folder")?;
    prepare_at(directory.data_local_dir())
}

fn prepare_at(data: &Path) -> Result<(PathBuf, File)> {
    fs::create_dir_all(data).with_context(|| format!("Could not create {}", data.display()))?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data.join("library.lock"))?;
    lock.try_lock()
        .context("This library is already open or cannot be locked")?;
    let path = data.join("library.sqlite3");
    catalog::initialize(&catalog::open(&path)?)?;
    Ok((path, lock))
}

fn save_player_settings(db: &Connection, prefs: &PlayerPreferences) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "UPDATE player_settings SET options=? WHERE id=1",
        [serde_json::to_string(prefs)?],
    )?;
    let bytes = i64::from(prefs.thumbnail_cache_mb.clamp(1, 1024)) * 1024 * 1024;
    if tx.execute(
        "UPDATE timeline_budget SET bytes_allowed=?1 WHERE bytes_allowed!=?1",
        [bytes],
    )? != 0
    {
        media::trim_timeline(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

fn settings(db: &Connection) -> Result<Settings> {
    Ok(Settings {
        auto_scan: catalog::auto_scan(db)?,
        sources: catalog::sources(db, "sources")?,
        exclusions: catalog::sources(db, "exclusions")?,
        collections: catalog::collections(db)?,
        watch_later: catalog::watch_later(db)?.into_iter().collect(),
        view: catalog::library_view(db)?,
        appearance: catalog::appearance(db)?,
        player: serde_json::from_str(&db.query_row(
            "SELECT options FROM player_settings WHERE id=1",
            [],
            |row| row.get::<_, String>(0),
        )?)?,
        control_bar_layout: catalog::control_bar_layout(db)?,
        progress_bar_position: catalog::progress_bar_position(db)?,
    })
}

fn details(db: &Connection, id: i64, emit: &Emitter) -> Result<()> {
    let video = catalog::get_visible(db, id)?.map(Box::new);
    let members = if video.is_some() {
        catalog::memberships(db, id)?
    } else {
        Vec::new()
    };
    emit.send(Event::Detail(id, video, members));
    Ok(())
}

fn action_loop(
    mut db: Connection,
    writes: Arc<Mutex<()>>,
    scan: Arc<scan::Control>,
    wake: Sender<()>,
    actions: Receiver<Action>,
    emit: Emitter,
    probe_wake: Sender<()>,
) {
    for action in actions {
        let result = (|| -> Result<()> {
            match action {
                Action::AutoScan(enable) => {
                    let _guard = writes.lock().unwrap();
                    catalog::set_auto_scan(&db, enable)?;
                    update_sources(&db, &scan, &wake, &emit)?;
                }
                Action::AddSource(path, exclusion) => {
                    let path = dunce::canonicalize(&path)
                        .with_context(|| format!("Could not open {}", path.display()))?;
                    anyhow::ensure!(path.is_dir(), "Choose a folder or drive");
                    let _guard = writes.lock().unwrap();
                    catalog::set_source(&db, &path, exclusion)?;
                    update_sources(&db, &scan, &wake, &emit)?;
                }
                Action::RemoveSource(id, exclusion) => {
                    let _guard = writes.lock().unwrap();
                    catalog::remove_source(&db, id, exclusion)?;
                    update_sources(&db, &scan, &wake, &emit)?;
                }
                Action::AddToCollection(target, videos, detail) => {
                    let creating = matches!(&target, CollectionTarget::New(_));
                    let added = catalog::add_to_collection(&mut db, target, &videos)?;
                    if creating {
                        emit.send(Event::Settings(settings(&db)?));
                    }
                    if let Some(id) = detail {
                        details(&db, id, &emit)?;
                    }
                    if !videos.is_empty() {
                        emit.send(Event::Changed);
                        let message = match added {
                            0 => "No new videos added to collection".into(),
                            1 => "Added 1 video to collection".into(),
                            _ => format!("Added {added} videos to collection"),
                        };
                        emit.send(Event::Message(message, false));
                    }
                }
                Action::Member(id, collection, included) => {
                    catalog::set_membership(&db, id, collection, included)?;
                    details(&db, id, &emit)?;
                    emit.send(Event::Changed);
                }
                Action::WatchLater(id, saved) => {
                    let included = catalog::set_watch_later(&db, id, saved)?;
                    if included == saved {
                        emit.send(Event::WatchLater(id, included));
                    } else {
                        emit.send(Event::Message(
                            "Video is no longer in the library".into(),
                            true,
                        ));
                    }
                }
                Action::Drives => emit.send(Event::Drives(scan::mounted_drives())),
                Action::SavePlayerSettings(prefs) => save_player_settings(&db, &prefs)?,
                Action::RecordPlay(id) => {
                    db.execute("INSERT INTO playback_history(video,opened) SELECT id,? FROM videos WHERE id=? ON CONFLICT(video) DO UPDATE SET opened=excluded.opened",rusqlite::params![crate::model::now(),id])?;
                }
                Action::ClearPlaybackHistory => {
                    db.execute("DELETE FROM playback_history", [])?;
                    emit.send(Event::History(Vec::new()));
                }
                Action::History => emit.send(Event::History(catalog::playback_history(&db)?)),
                Action::ExportPlaylist(filter, destination) => {
                    catalog::export_playlist(&db, &filter, &destination)?;
                    emit.send(Event::Message(
                        format!("Saved playlist to {}", destination.display()),
                        false,
                    ));
                }
                Action::SaveView(view) => catalog::save_view(&db, view)?,
                Action::SaveAppearance(appearance) => catalog::save_appearance(&db, appearance)?,
                Action::SaveControlBarLayout(layout) => {
                    catalog::save_control_bar_layout(&db, layout)?
                }
                Action::SaveProgressBarPosition(position) => {
                    catalog::save_progress_bar_position(&db, position)?
                }
                Action::CopyPaths(ids) => {
                    let result = catalog::selection_files(&mut db, ids)
                        .map(|videos| {
                            clipboard_paths(videos.iter().map(|video| video.path.as_path()))
                        })
                        .map_err(|error| format!("{error:#}"));
                    emit.send(Event::CopiedPaths(result));
                }
                Action::Inspect(id) => details(&db, id, &emit)?,
                Action::Reveal(path) => files::reveal(&path)?,
                Action::SelectFiles(id, ids) => {
                    let videos = catalog::selection_files(&mut db, ids)
                        .map_err(|error| format!("{error:#}"));
                    emit.send(Event::SelectedFiles(id, videos));
                }
                Action::ManageFiles(videos, operation) => {
                    let report = manage_files(&mut db, &writes, videos, operation, |done| {
                        emit.send(Event::FileProgress(done));
                    });
                    if !report.succeeded.is_empty() {
                        let _ = probe_wake.try_send(());
                        emit.send(Event::Changed);
                    }
                    emit.send(Event::FilesDone(report));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            emit.error(error);
        }
    }
}

pub fn clipboard_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> String {
    paths
        .into_iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

fn subfolders(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .with_context(|| format!("Could not list {}", directory.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    paths.sort_by_cached_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    });
    Ok(paths)
}

fn update_sources(
    db: &Connection,
    scan: &scan::Control,
    wake: &Sender<()>,
    emit: &Emitter,
) -> Result<()> {
    catalog::invalidate_scan_history(db)?;
    scan.restart();
    let _ = wake.try_send(());
    emit.send(Event::Settings(settings(db)?));
    emit.send(Event::Changed);
    Ok(())
}

pub(crate) fn manage_files(
    db: &mut Connection,
    writes: &Mutex<()>,
    videos: Vec<Video>,
    operation: files::Operation,
    progress: impl Fn(usize),
) -> FileReport {
    use files::{DeleteMode, Operation};
    let (label, keep_selection) = match &operation {
        Operation::Delete(DeleteMode::Trash) => ("moved to Trash", false),
        Operation::Delete(DeleteMode::Permanent) => ("permanently deleted", false),
        Operation::Transfer { moving: true, .. } => ("moved", false),
        Operation::Transfer { moving: false, .. } => ("copied", true),
    };
    let mut report = FileReport {
        succeeded: Vec::new(),
        failed: Vec::new(),
        label,
        keep_selection,
        deleted: matches!(operation, Operation::Delete(_)),
    };
    let mut last = Instant::now();
    for (index, video) in videos.into_iter().enumerate() {
        let outcome = match &operation {
            Operation::Delete(mode) => files::delete(db, writes, &video, *mode),
            Operation::Transfer {
                destination,
                moving,
            } => files::transfer(db, writes, &video, destination, *moving).map(|_| ()),
        };
        match outcome {
            Ok(()) => report.succeeded.push(video.id),
            Err(error) => report.failed.push((video.path, format!("{error:#}"))),
        }
        if last.elapsed() >= Duration::from_millis(150) {
            progress(index + 1);
            last = Instant::now();
        }
    }
    report
}

fn path_missing(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => !metadata.is_file(),
        Err(error) => matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory),
    }
}

fn remove_missing(
    db: &mut Connection,
    writes: &Mutex<()>,
    videos: &[Video],
    mounted_drives: impl FnOnce() -> Vec<PathBuf>,
) -> Result<Vec<i64>> {
    let missing: Vec<_> = videos
        .iter()
        .filter(|video| path_missing(&video.path))
        .collect();
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let sources = catalog::sources(db, "sources")?;
    let drive_for = |path: &Path| {
        sources
            .iter()
            .filter(|source| source.automatic && path.starts_with(&source.path))
            .max_by_key(|source| source.path.as_os_str().len())
    };
    let mounted = missing
        .iter()
        .any(|video| drive_for(&video.path).is_some())
        .then(mounted_drives)
        .unwrap_or_default();
    let _guard = writes.lock().unwrap();
    let tx = db.transaction()?;
    let mut removed = Vec::new();
    for video in missing {
        let source = sources
            .iter()
            .filter(|source| video.path.starts_with(&source.path))
            .max_by_key(|source| source.path.as_os_str().len());
        // A missing source or unmounted drive can still have entries in the catalog.
        if drive_for(&video.path).is_some_and(|drive| !mounted.contains(&drive.path))
            || !source
                .map(|source| source.path.as_path())
                .or_else(|| video.path.parent())
                .is_some_and(Path::is_dir)
        {
            continue;
        }
        // Check again after taking the write lock in case a scan or file operation raced us.
        if path_missing(&video.path) && catalog::remove_missing(&tx, video)? {
            removed.push(video.id);
        }
    }
    tx.commit()?;
    Ok(removed)
}

fn presence_loop(
    mut db: Connection,
    writes: &Mutex<()>,
    visible: Inbox<Vec<Video>>,
    playback: Inbox<Video>,
    checkpoints: Receiver<Checkpoint>,
    emit: Emitter,
) {
    let visible_tick = tick(Duration::from_millis(350));
    let sweep_tick = tick(Duration::from_secs(1));
    let mut videos = Vec::new();
    let mut after = 0;
    let check = |db: &mut Connection, videos: &[Video]| {
        let removed = remove_missing(db, writes, videos, scan::discover_drives).unwrap_or_default();
        let changed = !removed.is_empty();
        if changed {
            emit.send(Event::VideosMissing(removed));
        }
        changed
    };

    loop {
        crossbeam_channel::select_biased! {
            recv(checkpoints) -> checkpoint => match checkpoint {
                Ok(Checkpoint::Save(key, at)) => {
                    if let Err(error) = catalog::save_playback_position(&db, key, at) {
                        emit.error(error.context("Could not save playback position"));
                    }
                }
                Ok(Checkpoint::Flush(ack)) => { let _ = ack.send(()); }
                Err(_) => break,
            },
            recv(playback.rx) -> wake => {
                if wake.is_err() { break; }
                if let Some(video) = playback.take() {
                    if !path_missing(&video.path) {
                        playback_ready(&db, video, &emit);
                    } else if !check(&mut db, std::slice::from_ref(&video)) {
                        if path_missing(&video.path) {
                            emit.send(Event::PlaybackUnavailable(video.thumbnail_key()));
                        } else {
                            playback_ready(&db, video, &emit);
                        }
                    }
                }
            },
            recv(visible.rx) -> wake => {
                if wake.is_err() { break; }
                if let Some(current) = visible.take() {
                    videos = current;
                    check(&mut db, &videos);
                }
            },
            recv(visible_tick) -> _ => { check(&mut db, &videos); },
            recv(sweep_tick) -> _ => {
                if let Ok(batch) = catalog::presence_batch(&db, after) {
                    after = if batch.len() < PAGE { 0 } else { batch[PAGE - 1].id };
                    check(&mut db, &batch);
                }
            },
        }
    }
}

fn playback_ready(db: &Connection, video: Video, emit: &Emitter) {
    let position = catalog::playback_position(db, video.thumbnail_key()).unwrap_or_else(|error| {
        emit.error(error.context("Could not read playback position"));
        None
    });
    emit.send(Event::PlaybackReady(Box::new(video), position));
}

fn search_loop(
    mut db: Connection,
    requests: Inbox<Search>,
    ranges: Inbox<SelectionRange>,
    probe: Option<PathBuf>,
    emit: Emitter,
) {
    loop {
        crossbeam_channel::select! {
            recv(requests.rx) -> wake => {
                if wake.is_err() {
                    break;
                }
                let Some(request) = requests.take() else { continue };
                let next = requests.next.clone();
                let _ = db.progress_handler(1024, Some(move || next.lock().unwrap().is_some()));
                let result = catalog::search_page(
                    &mut db, &request.filter, request.index, request.count, request.anchor,
                );
                let _ = db.progress_handler(0, None::<fn() -> bool>);
                if requests.next.lock().unwrap().is_some() {
                    continue;
                }
                match result {
                    Ok(page) => {
                        emit.send(Event::Page {
                            request: request.id,
                            index: page.index,
                            total: page.total,
                            rows: page.rows,
                            anchor_position: page.anchor_position,
                        });
                        let active = || requests.next.lock().unwrap().is_none();
                        if page.total == Some(0) && active()
                            && let Some(status) = search_path(&db, &request.filter.text, probe.as_deref(), &active)
                            && active()
                        {
                            emit.send(Event::SearchPath(request.id, status));
                        }
                    }
                    Err(error) => emit.send(Event::PageFailed(request.id, format!("{error:#}"))),
                }
            },
            recv(ranges.rx) -> wake => {
                if wake.is_err() {
                    break;
                }
                if let Some(range) = ranges.take() {
                    let ids = match range.target {
                        SelectionTarget::Between(first, last) => catalog::range_ids(&db, &range.filter, first, last),
                        SelectionTarget::Region(region) => catalog::region_ids(&db, &range.filter, region),
                        SelectionTarget::Positions(positions) => catalog::ids_at_positions(&db, &range.filter, positions),
                    }.map_err(|error| format!("{error:#}"));
                    emit.send(Event::SelectionRange(range.id, ids));
                }
            },
        }
    }
}

fn search_path(
    db: &Connection,
    text: &str,
    probe: Option<&Path>,
    active: &dyn Fn() -> bool,
) -> Option<SearchPathStatus> {
    use SearchPathStatus::*;
    let path = Path::new(text.trim());
    if !path.is_absolute() || path.extension().is_none() || !path.is_file() {
        return None;
    }
    let ext = video_ext(path);
    if ext
        .as_ref()
        .is_none_or(|ext| scan::candidate_content(path, ext).is_ok_and(|valid| !valid))
    {
        return Some(Unsupported);
    }
    match catalog::path_probe(db, path).ok().flatten() {
        Some(0) => Some(Checking),
        Some(2) => Some(Unsupported),
        Some(_) => None,
        None => Some(
            if probe.is_some_and(|tool| media::probe(tool, path, Some(active)).is_err()) {
                Unsupported
            } else {
                OutsideLibrary
            },
        ),
    }
}

fn thumbnail_loop(
    db: Connection,
    executable: Option<PathBuf>,
    requests: Inbox<Vec<Video>>,
    emit: Emitter,
) {
    let mut pending = Vec::new();
    let mut processed = 0;
    loop {
        if pending.is_empty() && requests.rx.recv().is_err() {
            break;
        }
        if let Some(mut visible) = requests.take() {
            visible.retain(|video| match media::cached_thumbnail(&db, video) {
                Ok(Some(image)) => {
                    emit.send(Event::Thumbnail(video.thumbnail_key(), image));
                    false
                }
                _ => true,
            });
            visible.reverse();
            pending = visible;
        }
        let Some(video) = pending.pop() else {
            continue;
        };
        let key = video.thumbnail_key();
        let active = || {
            requests
                .next
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|visible| visible.iter().any(|video| video.thumbnail_key() == key))
        };
        if !active() {
            continue;
        }
        let result = media::generate_thumbnail(&db, executable.as_deref(), &video, &active);
        if active() {
            emit.send(Event::Thumbnail(key, result.unwrap_or_default()));
        }
        processed += 1;
        if processed % 48 == 0
            && let Err(error) = media::trim_thumbnails(&db)
        {
            emit.error(error.context("Thumbnail cache could not be trimmed"));
        }
    }
}

fn preview_loop(
    db: Connection,
    tool: Option<PathBuf>,
    requests: Inbox<Option<PreviewRequest>>,
    emit: Emitter,
) {
    while requests.rx.recv().is_ok() {
        if let Some(Some(request)) = requests.take() {
            let active = || requests.next.lock().unwrap().is_none();
            for segment in request.first_segment..media::preview_count(&request.video) {
                if !active() {
                    break;
                }
                let image =
                    media::preview_segment(&db, tool.as_deref(), &request.video, segment, &active)
                        .ok()
                        .flatten();
                if active() {
                    emit.send(Event::Preview(
                        request.video.thumbnail_key(),
                        segment,
                        image,
                    ));
                }
            }
        }
    }
}

pub(crate) fn preview_positions(
    video: &Video,
    duration: f64,
    focus: f64,
    origin: TimelineOrigin,
) -> Vec<i64> {
    let mut samples = Vec::new();
    // Cover the full timeline first, then fill the gaps while the worker is idle.
    let divisions: u32 = if origin == TimelineOrigin::Player {
        128
    } else {
        16
    };
    let fractions = [focus, 0.0, 1.0]
        .into_iter()
        .chain((1..=divisions.ilog2()).flat_map(|level| {
            let count = 1_u32 << level;
            (1..count)
                .step_by(2)
                .map(move |n| f64::from(n) / f64::from(count))
        }));
    for fraction in fractions {
        let at = media::timeline_sample(video, duration, duration * fraction);
        let ms = (at * 1000.0).round() as i64;
        if !samples.contains(&ms) {
            samples.push(ms);
        }
    }
    samples
}

fn timeline_loop(
    db: Connection,
    tool: Option<PathBuf>,
    requests: Inbox<Option<TimelineRequest>>,
    library: Inbox<Option<TimelinePrefetch>>,
    player: Inbox<Option<TimelinePrefetch>>,
    emit: Emitter,
) {
    let mut drafts = [
        crate::player::thumbnail::DraftDecoder::default(),
        crate::player::thumbnail::DraftDecoder::default(),
    ];
    let mut plans = [None, None];
    let prefetch = [&library, &player];
    let origins = [TimelineOrigin::Library, TimelineOrigin::Player];
    let mut next_plan = 0;
    loop {
        if let Some(request) = requests.take() {
            if let Some(request) = request {
                let decoder = &mut drafts[request.origin.index()];
                resolve_timeline(&db, tool.as_deref(), &requests, request, &emit, decoder);
            }
            continue;
        }
        for (index, inbox) in prefetch.iter().enumerate() {
            if let Some(next) = inbox.take() {
                if next
                    .as_ref()
                    .is_none_or(|plan| !drafts[index].ready_for(&plan.video))
                {
                    drafts[index].release();
                }
                plans[index] = next
                    .filter(|next| next.remote || !scan::remote_file(&next.video.path))
                    .map(|next| {
                        let (id, rev) = next.video.thumbnail_key();
                        if let Ok(frames) = media::recent_timeline(&db, (id, rev)) {
                            for (ms, image) in frames {
                                emit.send(Event::TimelinePrefetched(
                                    origins[index],
                                    (id, rev, ms),
                                    image,
                                ));
                            }
                        }
                        let mut samples = preview_positions(
                            &next.video,
                            next.duration,
                            next.focus,
                            origins[index],
                        );
                        samples.reverse();
                        (next.video, samples)
                    });
                next_plan = index;
            }
        }
        let index = [next_plan, 1 - next_plan].into_iter().find(|&i| {
            plans[i]
                .as_ref()
                .is_some_and(|(_, samples)| !samples.is_empty())
        });
        if let Some(index) = index
            && let Some((video, positions)) = &mut plans[index]
            && let Some(ms) = positions.pop()
        {
            next_plan = 1 - index;
            let key = (video.id, video.rev, ms);
            let idle = || {
                requests.next.lock().unwrap().is_none()
                    && prefetch
                        .iter()
                        .all(|inbox| inbox.next.lock().unwrap().is_none())
            };
            let cached = media::cached_timeline(&db, key).ok().flatten();
            let mut jpeg = None;
            let image = cached.or_else(|| {
                let tool = tool.as_deref()?;
                if !idle() {
                    return None;
                }
                let (bytes, image) = drafts[index]
                    .frame(tool, video, ms as f64 / 1000., true, &idle)
                    .ok()?;
                jpeg = Some(bytes);
                Some(image)
            });
            if idle() {
                if let Some(image) = image {
                    emit.send(Event::TimelinePrefetched(origins[index], key, image));
                    if let Some(bytes) = jpeg {
                        let _ = media::store_timeline(&db, key, &bytes);
                    }
                }
            } else {
                positions.push(ms);
            }
            continue;
        }
        crossbeam_channel::select! {
            recv(requests.rx) -> wake => if wake.is_err() { break; },
            recv(library.rx) -> wake => if wake.is_err() { break; },
            recv(player.rx) -> wake => if wake.is_err() { break; },
        }
    }
}

fn resolve_timeline(
    db: &Connection,
    tool: Option<&Path>,
    requests: &Inbox<Option<TimelineRequest>>,
    request: TimelineRequest,
    emit: &Emitter,
    drafts: &mut crate::player::thumbnail::DraftDecoder,
) {
    let still_targeted = || {
        requests.next.lock().unwrap().as_ref().is_none_or(|next| {
            next.as_ref()
                .is_some_and(|next| next.key == request.key && next.origin == request.origin)
        })
    };
    if matches!(request.origin, TimelineOrigin::Precise(_)) {
        resolve_filmstrip(db, tool, &request, &still_targeted, emit);
        return;
    }
    if let Ok(Some(image)) = media::cached_timeline(db, request.key) {
        emit.send(Event::Timeline(
            request.origin,
            request.key,
            Some(image),
            true,
        ));
        return;
    }
    let Some(tool) = tool else {
        emit.send(Event::Timeline(request.origin, request.key, None, true));
        return;
    };
    if !request.has_preview
        && request.at > 0.15
        && let Ok(image) = (if drafts.ready_for(&request.video) {
            drafts
                .frame(tool, &request.video, request.at, false, &still_targeted)
                .map(|(_, image)| image)
        } else {
            // An immediate hover must not wait for a second playback core to initialize.
            media::timeline_preview(tool, &request.video, request.at, false, &still_targeted)
                .map(|(_, image)| image)
        })
        && still_targeted()
    {
        emit.send(Event::Timeline(
            request.origin,
            request.key,
            Some(image),
            false,
        ));
    }
    if !still_targeted() {
        return;
    }
    let exact = drafts
        .frame(tool, &request.video, request.at, true, &still_targeted)
        .ok();
    if still_targeted() {
        if let Some((jpeg, image)) = exact {
            emit.send(Event::Timeline(
                request.origin,
                request.key,
                Some(image),
                true,
            ));
            let _ = media::store_timeline(db, request.key, &jpeg);
        } else {
            emit.send(Event::Timeline(request.origin, request.key, None, true));
        }
    }
}

fn resolve_filmstrip(
    db: &Connection,
    tool: Option<&Path>,
    request: &TimelineRequest,
    still_targeted: &dyn Fn() -> bool,
    emit: &Emitter,
) {
    let (id, rev, center) = request.key;
    let mut missing = Vec::new();
    for ms in std::iter::once(center).chain(request.neighbors.iter().copied()) {
        if !still_targeted() {
            return;
        }
        let key = (id, rev, ms);
        if let Ok(Some(image)) = media::cached_timeline(db, key) {
            emit.send(Event::Timeline(request.origin, key, Some(image), true));
        } else {
            missing.push(ms);
        }
    }
    let mut chronological = missing.clone();
    chronological.sort_unstable();
    if missing.len() >= 2
        && let Some(tool) = tool
        && let Ok(frames) =
            media::filmstrip_frames(tool, &request.video, &chronological, still_targeted)
    {
        // Publish the whole visible strip before touching disk. A single cache
        // transaction keeps subsequent slots from waiting for earlier writes.
        let mut cache = Vec::with_capacity(frames.len());
        for (ms, (jpeg, image)) in chronological.into_iter().zip(frames) {
            let key = (id, rev, ms);
            emit.send(Event::Timeline(request.origin, key, Some(image), true));
            cache.push((key, jpeg));
        }
        let _ = media::store_timeline_batch(
            db,
            cache.iter().map(|(key, jpeg)| (*key, jpeg.as_slice())),
        );
    } else {
        for ms in missing {
            if !still_targeted() {
                break;
            }
            let key = (id, rev, ms);
            let frame = tool.and_then(|tool| {
                media::timeline_preview(
                    tool,
                    &request.video,
                    ms as f64 / 1000.0,
                    true,
                    still_targeted,
                )
                .ok()
            });
            if !still_targeted() {
                break;
            }
            let image = frame.map(|(jpeg, image)| {
                let _ = media::store_timeline(db, key, &jpeg);
                image
            });
            emit.send(Event::Timeline(request.origin, key, image, true));
        }
    }
}

fn probe_loop(mut db: Connection, tool: Option<PathBuf>, wake: Receiver<()>, emit: Emitter) {
    let Some(tool) = tool else {
        return;
    };
    while wake.recv().is_ok() {
        let result = probe_pass(&mut db, &tool, &emit);
        emit.send(Event::Validating(false));
        if let Err(error) = result {
            emit.error(error.context("Metadata indexing paused until the next scan"));
        }
    }
}

fn probe_pass(db: &mut Connection, tool: &Path, emit: &Emitter) -> Result<()> {
    let mut after = 0;
    let mut last = Instant::now();
    let mut changed = false;
    let mut checking = false;
    let mut pending = Vec::new();
    loop {
        let batch = catalog::pending(db, after)?;
        if batch.is_empty() {
            break;
        }
        if !checking {
            checking = true;
            emit.send(Event::Validating(true));
        }
        for video in batch {
            after = video.id;
            if Stamp::read(&video.path).ok() == Some(video.stamp) {
                let media = if scan::candidate_content(&video.path, &video.ext).unwrap_or(true) {
                    media::probe(tool, &video.path, None).ok()
                } else {
                    None
                };
                if Stamp::read(&video.path).ok() == Some(video.stamp) {
                    pending.push((video, media));
                }
            }
            if pending.len() >= 12
                || (!pending.is_empty() && last.elapsed() > Duration::from_millis(400))
            {
                commit_media(db, &mut pending)?;
                last = Instant::now();
                emit.send(Event::Changed);
            }
        }
        commit_media(db, &mut pending)?;
        changed = true;
    }
    if changed {
        emit.send(Event::Changed);
    }
    Ok(())
}

fn commit_media(
    db: &mut Connection,
    pending: &mut Vec<(Video, Option<crate::model::Media>)>,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let tx = db.transaction()?;
    for (video, fields) in pending.drain(..) {
        if let Some(fields) = fields {
            catalog::set_media(&tx, &video, &fields)?;
        } else {
            catalog::reject_media(&tx, &video)?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_theme_migrates_explicit_styles_and_preserves_opaque_on_reopen() -> Result<()> {
        use crate::player::options::InterfaceTheme;
        let dir = tempfile::tempdir()?;
        let (path, _lock) = prepare_at(dir.path())?;
        let db = catalog::open(&path)?;
        let fresh = settings(&db)?.player;
        assert_eq!(catalog::appearance(&db)?, Appearance::System);
        let expected = if cfg!(target_os = "macos") {
            InterfaceTheme::LiquidGlass
        } else {
            InterfaceTheme::Classic
        };
        assert_eq!(fresh.interface_theme, expected);
        assert_eq!(fresh.progress_bar_style, None);
        assert_eq!(fresh.glass_strength, 62);
        for (legacy, expected) in [
            (true, InterfaceTheme::LiquidGlass),
            (false, InterfaceTheme::Classic),
        ] {
            let options =
                serde_json::json!({"liquid_glass":legacy, "glass_osd":false, "glass_opacity":67});
            db.execute(
                "UPDATE player_settings SET options=?",
                [options.to_string()],
            )?;
            catalog::initialize(&db)?;
            let mut prefs = settings(&db)?.player;
            assert_eq!(prefs.interface_theme, expected);
            assert_eq!(prefs.classic_layout, ControlBarLayout::Classic);
            assert_eq!(prefs.glass_strength, 33);
            prefs.glass_strength = 73;
            prefs.interface_theme = InterfaceTheme::GlassOpaque;
            prefs.progress_bar_style = Some(InterfaceTheme::Classic);
            save_player_settings(&db, &prefs)?;
            catalog::save_appearance(&db, Appearance::Dark)?;
            let reopened = catalog::open(&path)?;
            catalog::initialize(&reopened)?;
            assert_eq!(catalog::appearance(&reopened)?, Appearance::Dark);
            let saved = settings(&reopened)?.player;
            assert_eq!(saved.interface_theme, InterfaceTheme::GlassOpaque);
            assert_eq!(saved.progress_bar_style, Some(InterfaceTheme::Classic));
            assert_eq!(saved.glass_strength, 73);
            assert!(!saved.glass_osd);
        }
        Ok(())
    }

    #[test]
    fn library_lock_prevents_a_second_open_and_releases_on_close() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (path, lock) = prepare_at(dir.path())?;
        let db = catalog::open(&path)?;
        catalog::save_control_bar_layout(&db, ControlBarLayout::Top)?;
        assert!(prepare_at(dir.path()).is_err());
        drop(db);
        drop(lock);
        let (reopened, _lock) = prepare_at(dir.path())?;
        assert_eq!(reopened, path);
        assert_eq!(
            settings(&catalog::open(&reopened)?)?.control_bar_layout,
            ControlBarLayout::Top
        );
        Ok(())
    }

    #[test]
    fn control_hide_delay_upgrades_the_old_default_only_once() -> Result<()> {
        for (old, expected) in [
            (400, crate::player::options::DEFAULT_HIDE_DELAY_MS),
            (1250, 1250),
        ] {
            let db = Connection::open_in_memory()?;
            catalog::initialize(&db)?;
            db.execute(
                "UPDATE player_settings SET options=json_set(options,'$.hide_delay_ms',?)",
                [old],
            )?;
            db.pragma_update(None, "user_version", 1)?;
            catalog::initialize(&db)?;
            let mut prefs = settings(&db)?.player;
            assert_eq!(prefs.hide_delay_ms, expected);
            prefs.hide_delay_ms = 400;
            save_player_settings(&db, &prefs)?;
            catalog::initialize(&db)?;
            assert_eq!(settings(&db)?.player.hide_delay_ms, 400);
        }
        Ok(())
    }

    #[test]
    fn player_settings_upgrade_and_reopen() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("library.sqlite3");
        let db = catalog::open(&path)?;
        catalog::initialize(&db)?;
        assert_eq!(settings(&db)?.player, PlayerPreferences::default());
        db.execute(
            "UPDATE player_settings SET options=?",
            [r#"{"glass":false}"#],
        )?;
        let mut prefs = settings(&db)?.player;
        assert!(!prefs.glass);
        assert_eq!(prefs.toolbar, PlayerPreferences::default().toolbar);
        prefs.toolbar.reverse();
        prefs.eq_presets.insert("Speech".into(), [3; 10]);
        prefs.watched_properties.push("width".into());
        prefs.engine.insert("sub-delay".into(), "0.25".into());
        save_player_settings(&db, &prefs)?;
        drop(db);
        let db = catalog::open(&path)?;
        catalog::initialize(&db)?;
        assert_eq!(settings(&db)?.player, prefs);
        Ok(())
    }

    #[test]
    fn breadcrumbs_list_only_subfolders_of_the_current_folder() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("sdb2");
        let sibling = temp.path().join("sdb3");
        let movie = source.join("movie");
        let nested = movie.join("series");
        let other = source.join("Archive");
        fs::create_dir_all(&nested)?;
        fs::create_dir(&sibling)?;
        fs::create_dir(&other)?;
        fs::write(source.join("video.mp4"), b"fixture")?;

        assert_eq!(subfolders(&source)?, [other, movie.clone()]);
        assert_eq!(subfolders(&movie)?, [nested]);
        Ok(())
    }

    #[test]
    fn finder_deletion_updates_an_open_library() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let data = temp.path().join("data");
        let source = temp.path().join("videos");
        fs::create_dir(&source)?;
        let deleted = source.join("deleted.mp4");
        let clicked = source.join("clicked.mp4");
        let kept = source.join("kept.mp4");
        fs::write(&deleted, "first video")?;
        fs::write(&clicked, "second video")?;
        fs::write(&kept, "third video")?;

        let (path, lock) = prepare_at(&data)?;
        let mut db = catalog::open(&path)?;
        catalog::set_auto_scan(&db, false)?;
        catalog::set_source(&db, &source, false)?;
        scan::refresh(
            &mut db,
            &scan::Control::default(),
            &Mutex::new(()),
            0,
            true,
            |_| {},
        )?;
        db.execute("UPDATE videos SET probe=1 WHERE probe=0", [])?;
        drop(lock);

        let service = Service::start_with(Gui::default(), move || prepare_at(&data));
        let deadline = Instant::now() + Duration::from_secs(3);
        let next_event = || {
            let event = service.events.recv_deadline(deadline)?;
            match event {
                Event::StartupFailed(error) | Event::PageFailed(_, error) => {
                    anyhow::bail!("{error}")
                }
                event => Ok(event),
            }
        };
        while !matches!(next_event()?, Event::Ready(..)) {}

        service.search(Search {
            id: 1,
            filter: Filter::default(),
            index: 0,
            count: true,
            anchor: None,
        });
        let (deleted_id, clicked_video, kept_video) = loop {
            if let Event::Page {
                request: 1,
                total,
                rows,
                ..
            } = next_event()?
            {
                assert_eq!(total, Some(3));
                let find = |path: &Path| rows.iter().find(|video| video.path == path).unwrap();
                service.watch_visible(rows.clone());
                break (
                    find(&deleted).id,
                    find(&clicked).clone(),
                    find(&kept).clone(),
                );
            }
        };

        let kept_id = kept_video.id;
        service.play(kept_video);
        while !matches!(next_event()?, Event::PlaybackReady(..)) {}

        fs::remove_file(&deleted)?;
        let timeout = Instant::now() + Duration::from_secs(2);
        loop {
            match service.events.recv_deadline(timeout) {
                Ok(Event::VideosMissing(ids)) => {
                    assert_eq!(ids, vec![deleted_id]);
                    break;
                }
                Ok(_) => {}
                Err(error) => anyhow::bail!("no library update after external deletion: {error}"),
            }
        }

        fs::remove_file(&clicked)?;
        service.play(clicked_video.clone());
        loop {
            match next_event()? {
                Event::VideosMissing(ids) => {
                    assert_eq!(ids, vec![clicked_video.id]);
                    break;
                }
                Event::PlaybackReady(..) => anyhow::bail!("deleted video opened in the player"),
                _ => {}
            }
        }

        service.search(Search {
            id: 2,
            filter: Filter::default(),
            index: 1,
            count: true,
            anchor: Some(kept_id),
        });
        loop {
            if let Event::Page {
                request: 2,
                index,
                total,
                rows,
                anchor_position,
            } = next_event()?
            {
                assert_eq!(index, 0);
                assert_eq!(total, Some(1));
                assert_eq!(anchor_position, Some(0));
                assert_eq!(rows[0].path, kept);
                return Ok(());
            }
        }
    }

    #[test]
    fn missing_video_is_kept_when_its_drive_is_unmounted() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let drive = temp.path().join("external");
        let folder = drive.join("footage");
        fs::create_dir_all(&folder)?;
        fs::write(folder.join("video.mp4"), "fixture")?;
        let (path, _lock) = prepare_at(&temp.path().join("data"))?;
        let mut db = catalog::open(&path)?;
        catalog::register_drives(&mut db, std::slice::from_ref(&drive))?;
        let writes = Mutex::new(());
        scan::refresh(&mut db, &scan::Control::default(), &writes, 0, true, |_| {})?;
        db.execute("UPDATE videos SET probe=1 WHERE probe=0", [])?;
        let (_, rows) = catalog::page(&mut db, &Filter::default(), 0, true)?;
        let id = rows[0].id;
        catalog::add_to_collection(&mut db, CollectionTarget::New("Favorites".into()), &[id])?;
        fs::rename(folder, temp.path().join("elsewhere"))?;

        assert!(remove_missing(&mut db, &writes, &rows, Vec::new)?.is_empty());
        assert!(catalog::get_visible(&db, id)?.is_some());
        assert_eq!(
            remove_missing(&mut db, &writes, &rows, || vec![drive])?,
            vec![id]
        );
        assert!(catalog::get_visible(&db, id)?.is_none());
        assert_eq!(catalog::memberships(&db, id)?.len(), 1);
        Ok(())
    }
}
