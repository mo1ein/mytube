mod ffi;
mod glass;
pub mod options;
mod precise;
#[cfg(target_os = "macos")]
mod recognition;
mod render;
pub mod subtitles;
pub(crate) mod thumbnail;

use std::{
    env,
    ffi::{CStr, CString, c_char, c_void},
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::{Arc, Mutex},
    thread,
};

use anyhow::{Context as _, Result, bail, ensure};
use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};
use eframe::{egui, glow};

use crate::{files, model::Video};

use ffi::Api;
pub use glass::Glass;
pub use precise::{PreciseFinish, PreciseId};
use precise::{PrecisePlayback, Target};
use render::Renderer;

/// The player outline in page coordinates, including any letterbox bars.
#[derive(Clone, Copy)]
pub struct VideoMask {
    pub frame: egui::Rect,
    pub radius: u8,
}

impl VideoMask {
    /// A docked control shares only the outer corners it reaches.
    pub fn corners(self, rect: egui::Rect) -> egui::CornerRadius {
        let rect = rect.expand(0.01);
        let corner = |pos| if rect.contains(pos) { self.radius } else { 0 };
        egui::CornerRadius {
            nw: corner(self.frame.left_top()),
            ne: corner(self.frame.right_top()),
            sw: corner(self.frame.left_bottom()),
            se: corner(self.frame.right_bottom()),
        }
    }
}

pub type GlProc = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;

pub const MIN_SPEED: f64 = 0.25;
pub const MAX_SPEED: f64 = 4.0;
pub const SPEED_STEP: f64 = 0.05;

pub fn stepped_speed(value: f64) -> f64 {
    (value.clamp(MIN_SPEED, MAX_SPEED) / SPEED_STEP).round() * SPEED_STEP
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: i64,
    pub kind: TrackKind,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct Chapter {
    pub at: f64,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct Playback {
    pub position: f64,
    pub confirmed_position: Option<f64>,
    pub duration: f64,
    pub volume: f64,
    pub paused: bool,
    pub muted: bool,
    pub ended: bool,
    pub precise_seeking: bool,
    pub ready: bool,
    frame_ready: bool,
    pub error: Option<String>,
    tool_notice: Option<(String, bool)>,
    pub scripts: Arc<[PathBuf]>,
    pub inspection: Arc<[(String, String)]>,
    #[cfg(target_os = "macos")]
    pub recognized_text: Option<Arc<str>>,
    #[cfg(target_os = "macos")]
    pub recognizing: bool,
    pub speed: f64,
    pub captions: bool,
    pub audio: Option<i64>,
    pub subtitle: Option<i64>,
    pub secondary_subtitle: Option<i64>,
    pub video_track: Option<i64>,
    pub seeks: u64,
    pub tracks: Arc<[Track]>,
    pub chapters: Arc<[Chapter]>,
    video_aspect: Option<f32>,
    video_rotated: bool,
    load_id: u64,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            position: 0.0,
            confirmed_position: None,
            duration: 0.0,
            volume: 100.0,
            paused: false,
            muted: false,
            ended: false,
            precise_seeking: false,
            ready: false,
            frame_ready: false,
            error: None,
            tool_notice: None,
            scripts: Arc::from([]),
            inspection: Arc::from([]),
            #[cfg(target_os = "macos")]
            recognized_text: None,
            #[cfg(target_os = "macos")]
            recognizing: false,
            speed: 1.0,
            captions: true,
            audio: None,
            subtitle: None,
            secondary_subtitle: None,
            video_track: None,
            seeks: 0,
            tracks: Arc::from([]),
            chapters: Arc::from([]),
            video_aspect: None,
            video_rotated: false,
            load_id: 0,
        }
    }
}

impl Playback {
    pub fn display_aspect(&self, video: &Video) -> f32 {
        if let Some(aspect) = self.video_aspect {
            return if self.video_rotated {
                1.0 / aspect
            } else {
                aspect
            };
        }
        match (video.media.width, video.media.height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => width as f32 / height as f32,
            _ => 16.0 / 9.0,
        }
    }
}

pub enum Control {
    Load(Load),
    Toggle,
    SetPaused(bool),
    Seek(f64),
    BeginPrecise(PreciseId, f64),
    PreviewPrecise(PreciseId),
    FinishPrecise(PreciseId, PreciseFinish),
    Volume(f64),
    AdjustVolume(i32),
    Mute,
    Speed(f64),
    Captions(bool),
    Configure(&'static str, String),
    Equalizer([i8; 10]),
    Inspect(u64, Vec<String>),
    Tool(u64, PlayerTool),
    SubtitlePosition(u8),
    Track(TrackKind, Option<i64>),
    Repeat(bool),
    Frame(bool),
    Stop,
    Exit,
}

pub enum PlayerTool {
    Delogo(Option<[u32; 4]>),
    Chapter(bool),
    SecondarySubtitle(Option<i64>),
    #[cfg(target_os = "macos")]
    ReadText,
    LoopPoint,
    ClearLoop,
    Screenshot(PathBuf, bool),
    ExternalTrack(TrackKind, PathBuf),
    LoadScript(PathBuf),
    SubtitleStep(bool),
    Flip(bool),
    Crop(Option<(u32, u32)>),
    Filter(options::FilterPreset, bool),
}

impl Core {
    fn equalizer(&self, gains: [i8; 10]) -> Result<()> {
        let bands: Vec<_> = options::EQ_FREQUENCIES
            .into_iter()
            .zip(gains)
            .filter(|(_, gain)| *gain != 0)
            .map(|(frequency, gain)| {
                format!("equalizer=f={frequency}:t=o:w=1:g={}", gain.clamp(-12, 12))
            })
            .collect();
        if bands.is_empty() {
            if self
                .property("af")
                .unwrap_or_default()
                .contains("@mytube-eq")
            {
                self.command_sync(&[c"af", c"remove", c"@mytube-eq"])?;
            }
        } else {
            let filter = CString::new(format!("@mytube-eq:lavfi=[{}]", bands.join(",")))?;
            self.command_sync(&[c"af", c"add", &filter])?;
        }
        Ok(())
    }
    fn tool(&self, tool: PlayerTool) -> Result<Option<String>> {
        match tool {
            PlayerTool::Delogo(region) => {
                if let Some([x, y, w, h]) = region {
                    let width: u32 = self
                        .property("width")
                        .context("Video width unavailable")?
                        .parse()?;
                    let height: u32 = self
                        .property("height")
                        .context("Video height unavailable")?
                        .parse()?;
                    ensure!(
                        x > 0
                            && y > 0
                            && w > 0
                            && h > 0
                            && x.saturating_add(w) < width
                            && y.saturating_add(h) < height,
                        "Choose a rectangle inside the video, with a one-pixel margin"
                    );
                    let filter = CString::new(format!(
                        "@mytube-delogo:lavfi=[delogo=x={x}:y={y}:w={w}:h={h}]"
                    ))?;
                    self.command_sync(&[c"vf", c"add", &filter])?;
                } else if self
                    .property("vf")
                    .unwrap_or_default()
                    .contains("mytube-delogo")
                {
                    self.command_sync(&[c"vf", c"remove", c"@mytube-delogo"])?;
                }
            }
            PlayerTool::SecondarySubtitle(id) => self.command_sync(&[
                c"set",
                c"secondary-sid",
                &CString::new(id.map_or_else(|| "no".into(), |id| id.to_string()))?,
            ])?,
            PlayerTool::Chapter(next) => {
                self.command_sync(&[c"add", c"chapter", if next { c"1" } else { c"-1" }])?
            }
            #[cfg(target_os = "macos")]
            PlayerTool::ReadText => unreachable!("text recognition uses a background worker"),
            PlayerTool::LoopPoint => {
                self.command_sync(&[c"ab-loop"])?;
                let a = self.property("ab-loop-a").unwrap_or_default();
                let b = self.property("ab-loop-b").unwrap_or_default();
                return Ok(Some(if a == "no" {
                    "A–B loop cleared".into()
                } else if b == "no" {
                    format!("Loop starts at {a} s")
                } else {
                    format!("Loop from {a} to {b} s")
                }));
            }
            PlayerTool::ClearLoop => {
                self.command_sync(&[c"set", c"ab-loop-a", c"no"])?;
                self.command_sync(&[c"set", c"ab-loop-b", c"no"])?;
            }
            PlayerTool::Screenshot(path, subtitles) => {
                let file = CString::new(path.as_os_str().as_encoded_bytes())?;
                self.command_sync(&[
                    c"screenshot-to-file",
                    &file,
                    if subtitles { c"subtitles" } else { c"video" },
                ])?;
                return Ok(Some(format!("Saved screenshot to {}", path.display())));
            }
            PlayerTool::LoadScript(path) => {
                let file = CString::new(path.as_os_str().as_encoded_bytes())?;
                self.command_sync(&[c"load-script", &file])?;
                return Ok(Some(format!("Loaded {}", path.display())));
            }
            PlayerTool::ExternalTrack(kind, path) => {
                let path = CString::new(path.as_os_str().as_encoded_bytes())?;
                self.command_sync(&[
                    match kind {
                        TrackKind::Subtitle => c"sub-add",
                        TrackKind::Video => c"video-add",
                        TrackKind::Audio => c"audio-add",
                    },
                    &path,
                    c"select",
                ])?;
                if kind == TrackKind::Subtitle {
                    self.command_sync(&[c"set", c"sub-visibility", c"yes"])?;
                }
            }
            PlayerTool::SubtitleStep(next) => {
                self.command_sync(&[c"sub-seek", if next { c"1" } else { c"-1" }])?
            }
            PlayerTool::Filter(filter, apply) => {
                let chain = if filter.audio { c"af" } else { c"vf" };
                let label = format!("@mytube-user-{}", filter.id);
                if apply {
                    self.command_sync(&[
                        chain,
                        c"add",
                        &CString::new(format!("{label}:{}", filter.expression))?,
                    ])?;
                } else {
                    self.command_sync(&[chain, c"remove", &CString::new(label)?])?;
                }
                return Ok(Some(format!(
                    "{} {}",
                    if apply { "Applied" } else { "Removed" },
                    filter.name
                )));
            }
            PlayerTool::Crop(aspect) => {
                if let Some((w, h)) = aspect {
                    let source_width = self
                        .property("width")
                        .and_then(|s| s.parse::<u32>().ok())
                        .context("Video width unavailable")?;
                    let source_height = self
                        .property("height")
                        .and_then(|s| s.parse::<u32>().ok())
                        .context("Video height unavailable")?;
                    let width = source_width.min(source_height.saturating_mul(w) / h) & !1;
                    let height = source_height.min(source_width.saturating_mul(h) / w) & !1;
                    self.command_sync(&[
                        c"vf",
                        c"add",
                        &CString::new(format!("@mytube-crop:crop={width}:{height}"))?,
                    ])?;
                } else if self
                    .property("vf")
                    .unwrap_or_default()
                    .contains("@mytube-crop")
                {
                    self.command_sync(&[c"vf", c"remove", c"@mytube-crop"])?;
                }
            }
            PlayerTool::Flip(horizontal) => self.command_sync(&[
                c"vf",
                c"toggle",
                if horizontal {
                    c"@mytube-mirror:hflip"
                } else {
                    c"@mytube-flip:vflip"
                },
            ])?,
        }
        Ok(None)
    }
}
pub struct Load {
    video: Box<Video>,
    start: Option<f64>,
    paused: bool,
    id: u64,
}

struct Core {
    api: Api,
    handle: NonNull<c_void>,
}

// mpv's client handle can be used by concurrent callers. Event reads are
// confined to one thread and the GL render context is owned separately.
unsafe impl Send for Core {}
unsafe impl Sync for Core {}

impl Core {
    fn open() -> Result<Self> {
        let api = library()?;
        let handle =
            NonNull::new(unsafe { (api.mpv_create)() }).context("Could not start libmpv")?;
        let core = Self { api, handle };
        let set_option = |key: &CStr, value: &CStr| {
            core.check(unsafe {
                (core.api.mpv_set_option_string)(core.handle.as_ptr(), key.as_ptr(), value.as_ptr())
            })
            .with_context(|| format!("Setting mpv option {}", key.to_string_lossy()))
        };
        // Older libmpv releases use the original name for the software-friendly profile.
        set_option(c"profile", c"fast").or_else(|_| set_option(c"profile", c"sw-fast"))?;
        for (key, value) in [
            (c"vo", c"libmpv"),
            (c"idle", c"yes"),
            (c"keep-open", c"yes"),
            (c"config", c"no"),
            (c"load-scripts", c"no"),
            (c"ytdl", c"no"),
            (c"access-references", c"no"),
            (c"hwdec", c"auto-safe"),
            (c"terminal", c"no"),
            (c"osc", c"no"),
            (c"volume-max", c"100"),
            (c"input-default-bindings", c"no"),
        ] {
            set_option(key, value)?;
        }
        core.check(unsafe { (core.api.mpv_initialize)(core.handle.as_ptr()) })?;
        Ok(core)
    }

    fn check(&self, code: i32) -> Result<()> {
        ensure!(code >= 0, "{}", self.error(code));
        Ok(())
    }

    fn error(&self, code: i32) -> String {
        unsafe { CStr::from_ptr((self.api.mpv_error_string)(code)) }
            .to_string_lossy()
            .into_owned()
    }

    fn supports_option(&self, key: &str) -> bool {
        self.property(&format!("option-info/{key}/name")).is_some()
    }

    fn property(&self, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        let mut text: *mut c_char = std::ptr::null_mut();
        let code = unsafe {
            (self.api.mpv_get_property)(
                self.handle.as_ptr(),
                name.as_ptr(),
                ffi::STRING,
                std::ptr::from_mut(&mut text).cast(),
            )
        };
        if code < 0 || text.is_null() {
            return None;
        }
        let value = unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.api.mpv_free)(text.cast()) };
        Some(value)
    }

    fn tracks(&self) -> Arc<[Track]> {
        let count = self
            .property("track-list/count")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or_default();
        (0..count.min(128))
            .filter_map(|index| {
                let get = |field| self.property(&format!("track-list/{index}/{field}"));
                let kind = match get("type")?.as_str() {
                    "video" => TrackKind::Video,
                    "audio" => TrackKind::Audio,
                    "sub" => TrackKind::Subtitle,
                    _ => return None,
                };
                let id = get("id")?.parse().ok()?;
                let title = get("title")
                    .or_else(|| get("lang"))
                    .unwrap_or_else(|| format!("Track {id}"));
                Some(Track { id, kind, title })
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn chapters(&self) -> Arc<[Chapter]> {
        let count = self
            .property("chapter-list/count")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or_default();
        (0..count.min(256))
            .filter_map(|index| {
                let at = self
                    .property(&format!("chapter-list/{index}/time"))?
                    .parse::<f64>()
                    .ok()?;
                if !at.is_finite() || at < 0.0 {
                    return None;
                }
                let title = self
                    .property(&format!("chapter-list/{index}/title"))
                    .unwrap_or_else(|| format!("Chapter {}", index + 1));
                Some(Chapter { at, title })
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn command(&self, arguments: &[&CStr], reply: u64) -> Result<()> {
        let mut pointers: Vec<_> = arguments.iter().map(|argument| argument.as_ptr()).collect();
        pointers.push(std::ptr::null());
        self.check(unsafe {
            (self.api.mpv_command_async)(self.handle.as_ptr(), reply, pointers.as_ptr())
        })
    }

    fn command_sync(&self, arguments: &[&CStr]) -> Result<()> {
        let mut pointers: Vec<_> = arguments.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(std::ptr::null());
        self.check(unsafe { (self.api.mpv_command)(self.handle.as_ptr(), pointers.as_ptr()) })
    }

    fn load(&self, load: Load) -> Result<i64> {
        files::check_file(&load.video)?;
        let path = if cfg!(windows) {
            CString::new(load.video.path.to_string_lossy().as_bytes())?
        } else {
            CString::new(load.video.path.as_os_str().as_encoded_bytes())?
        };
        if let Some(at) = load.start.filter(|at| at.is_finite() && *at > 0.0) {
            let options = CString::new(format!("start={at:.3}"))?;
            // mpv 0.38 added a playlist index before the per-file options.
            if let Err(error) =
                self.command_sync(&[c"loadfile", &path, c"replace", c"-1", &options])
            {
                self.command_sync(&[c"loadfile", &path, c"replace", &options])
                    .map_err(|_| error)?;
            }
        } else {
            self.command_sync(&[c"loadfile", &path, c"replace"])?;
        }
        self.command_sync(&[c"set", c"pause", if load.paused { c"yes" } else { c"no" }])?;
        self.property("playlist/0/id")
            .and_then(|id| id.parse().ok())
            .context("Could not identify the loaded video")
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        unsafe { (self.api.mpv_terminate_destroy)(self.handle.as_ptr()) };
    }
}

fn library() -> Result<Api> {
    if let Some(path) = crate::setting("LIBMPV").map(PathBuf::from) {
        ensure!(
            path.is_absolute(),
            "MYTUBE_LIBMPV must point to an absolute path"
        );
        return Api::open(&path);
    }
    let path = Path::new(if cfg!(windows) {
        "libmpv-2.dll"
    } else if cfg!(target_os = "macos") {
        "libmpv.2.dylib"
    } else {
        "libmpv.so.2"
    });
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let bundled = dir.join(path);
        if bundled.is_file() {
            return Api::open(&bundled);
        }
        #[cfg(target_os = "macos")]
        if let Some(contents) = dir.parent()
            && let Ok(location) = std::fs::read_to_string(contents.join("Resources/libmpv-path"))
        {
            let library = PathBuf::from(location.trim());
            if library.is_absolute() && library.is_file() {
                return Api::open(&library);
            }
        }
    }
    #[cfg(target_os = "macos")]
    for prefix in ["/opt/homebrew", "/usr/local"] {
        let library = Path::new(prefix).join("opt/mpv/lib").join(path);
        if library.is_file() {
            return Api::open(&library);
        }
    }
    #[cfg(not(windows))]
    if let Ok(library) = Api::open(path) {
        return Ok(library);
    }
    bail!("MyTube couldn't find the libmpv playback engine. Run the MyTube installer again.")
}

// One wake in the command queue is enough to deliver the latest position.
type PreviewMailbox = Arc<Mutex<Option<(PreciseId, f64)>>>;

pub struct Session {
    unavailable_options: Vec<&'static str>,
    commands: Sender<Control>,
    preview: PreviewMailbox,
    state: Arc<Mutex<Playback>>,
    renderer: Arc<Mutex<Renderer>>,
}

impl Session {
    pub fn open(ctx: &egui::Context, loader: GlProc) -> Result<Self> {
        let core = Arc::new(Core::open()?);
        let unavailable_options = options::OPTIONS
            .iter()
            .filter(|option| !core.supports_option(option.key))
            .map(|option| option.key)
            .collect();
        let (commands, incoming) = unbounded();
        let state = Arc::new(Mutex::new(Playback::default()));
        let renderer = Renderer::new(
            core.clone(),
            loader,
            commands.clone(),
            state.clone(),
            ctx.clone(),
        );
        let worker_state = state.clone();
        let gui = ctx.clone();
        let preview = PreviewMailbox::default();
        let worker_preview = preview.clone();
        thread::spawn(move || control_loop(core, incoming, worker_preview, worker_state, gui));
        Ok(Self {
            unavailable_options,
            commands,
            preview,
            state,
            renderer: Arc::new(Mutex::new(renderer)),
        })
    }

    pub fn load(&self, video: Video, start: Option<f64>, paused: bool) {
        let mut state = self.state.lock().unwrap();
        let id = state.load_id.wrapping_add(1);
        let (volume, muted, speed, captions) =
            (state.volume, state.muted, state.speed, state.captions);
        *state = Playback {
            scripts: state.scripts.clone(),
            duration: video.media.duration.unwrap_or_default(),
            volume,
            muted,
            speed,
            captions,
            load_id: id,
            ..Playback::default()
        };
        drop(state);
        self.preview.lock().unwrap().take();
        self.renderer.lock().unwrap().load(Load {
            video: Box::new(video),
            start,
            paused,
            id,
        });
    }

    pub fn supports_option(&self, key: &str) -> bool {
        !self.unavailable_options.contains(&key)
    }

    pub fn send(&self, control: Control) {
        let _ = self.commands.send(control);
    }

    pub fn speed_menu_control(&self) -> (f64, Sender<Control>) {
        (self.state.lock().unwrap().speed, self.commands.clone())
    }

    pub fn inspect(&self, properties: &[String]) {
        self.send(Control::Inspect(
            self.state.lock().unwrap().load_id,
            properties.to_vec(),
        ));
    }
    pub fn load_id(&self) -> u64 {
        self.state.lock().unwrap().load_id
    }
    pub fn tool(&self, action: PlayerTool) {
        self.tool_for(self.load_id(), action);
    }
    pub fn tool_for(&self, id: u64, action: PlayerTool) {
        self.send(Control::Tool(id, action));
    }

    pub fn pick_script(&self) {
        let load = self.state.lock().unwrap().load_id;
        let sender = self.commands.clone();
        thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Load local mpv script")
                .add_filter("Lua script", &["lua"])
                .pick_file()
            {
                let _ = sender.send(Control::Tool(load, PlayerTool::LoadScript(path)));
            }
        });
    }

    pub fn pick_tool_file(&self, kind: Option<TrackKind>, subtitles: bool, format: &str) {
        let load = self.state.lock().unwrap().load_id;
        let sender = self.commands.clone();
        let extension = match format {
            "jpg" => "jpg",
            "webp" => "webp",
            "avif" => "avif",
            "jxl" => "jxl",
            _ => "png",
        };
        thread::spawn(move || {
            let dialog = rfd::FileDialog::new();
            let tool = match kind {
                Some(kind) => dialog
                    .set_title(match kind {
                        TrackKind::Audio => "Load external audio",
                        TrackKind::Subtitle => "Load subtitles",
                        TrackKind::Video => "Load external video",
                    })
                    .pick_file()
                    .map(|path| PlayerTool::ExternalTrack(kind, path)),
                None => dialog
                    .set_title("Save screenshot")
                    .add_filter(extension.to_uppercase(), &[extension])
                    .set_file_name(format!("frame.{extension}"))
                    .save_file()
                    .map(|path| PlayerTool::Screenshot(path, subtitles)),
            };
            if let Some(tool) = tool {
                let _ = sender.send(Control::Tool(load, tool));
            }
        });
    }

    pub fn take_tool_notice(&self) -> Option<(String, bool)> {
        self.state.lock().unwrap().tool_notice.take()
    }

    pub fn preview_precise(&self, id: PreciseId, at: f64) {
        let mut preview = self.preview.lock().unwrap();
        let queued = preview.replace((id, at));
        if !queued.is_some_and(|(pending, _)| pending == id) {
            self.send(Control::PreviewPrecise(id));
        }
    }

    pub fn stop(&self) {
        self.renderer.lock().unwrap().cancel_pending();
        let mut state = self.state.lock().unwrap();
        state.load_id = state.load_id.wrapping_add(1);
        state.frame_ready = false;
        drop(state);
        self.preview.lock().unwrap().take();
        self.send(Control::Stop);
    }

    pub fn state(&self) -> Playback {
        self.state.lock().unwrap().clone()
    }

    pub fn video_surface(
        &self,
        rect: egui::Rect,
        glass: Vec<Glass>,
        mask: VideoMask,
    ) -> egui::PaintCallback {
        let renderer = self.renderer.clone();
        let load_id = self.load_id();
        egui::PaintCallback {
            rect,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                renderer
                    .lock()
                    .unwrap()
                    .draw(info, painter, &glass, load_id, mask);
            })),
        }
    }

    pub fn shutdown(&self, gl: Option<&glow::Context>) {
        self.renderer.lock().unwrap().destroy(gl);
        self.send(Control::Exit);
    }
}

extern "C" fn wake_client(data: *mut c_void) {
    // The sender remains allocated until mpv's callback has been detached.
    let signal = unsafe { &*data.cast::<Sender<()>>() };
    let _ = signal.try_send(());
}

#[cfg(target_os = "macos")]
fn recognize_frame(
    core: &Core,
    id: u64,
    state: &Arc<Mutex<Playback>>,
    gui: &egui::Context,
) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("frame.png");
    core.tool(PlayerTool::Screenshot(path.clone(), false))?;
    {
        let mut current = state.lock().unwrap();
        if current.load_id != id {
            return Ok(());
        }
        current.recognizing = true;
        current.recognized_text = None;
    }
    let state = state.clone();
    let gui = gui.clone();
    thread::spawn(move || {
        let result = recognition::text(&path);
        drop(dir);
        let mut current = state.lock().unwrap();
        if current.load_id == id {
            current.recognizing = false;
            match result {
                Ok(text) => current.recognized_text = Some(text.into()),
                Err(error) => current.tool_notice = Some((error.to_string(), true)),
            }
            gui.request_repaint_of(egui::ViewportId::ROOT);
        }
    });
    Ok(())
}

fn apply_tool(
    core: &Core,
    id: u64,
    tool: PlayerTool,
    state: &Arc<Mutex<Playback>>,
    gui: &egui::Context,
) {
    #[cfg(target_os = "macos")]
    if matches!(tool, PlayerTool::ReadText) {
        let busy = state.lock().unwrap().recognizing;
        if !busy && let Err(error) = recognize_frame(core, id, state, gui) {
            state.lock().unwrap().tool_notice = Some((error.to_string(), true));
        }
        gui.request_repaint_of(egui::ViewportId::ROOT);
        return;
    }
    let refresh = matches!(tool, PlayerTool::ExternalTrack(..));
    let script = if let PlayerTool::LoadScript(path) = &tool {
        Some(path.clone())
    } else {
        None
    };
    let result = core.tool(tool);
    let tracks = (result.is_ok() && refresh).then(|| core.tracks());
    let mut current = state.lock().unwrap();
    if result.is_ok()
        && let Some(path) = script
        && !current.scripts.contains(&path)
    {
        let mut scripts = current.scripts.to_vec();
        scripts.push(path);
        current.scripts = scripts.into();
    }
    if current.load_id == id {
        if let Some(tracks) = tracks {
            current.tracks = tracks;
        }
        match result {
            Ok(Some(message)) => current.tool_notice = Some((message, false)),
            Err(error) => current.tool_notice = Some((error.to_string(), true)),
            Ok(None) => {}
        }
        gui.request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn control_loop(
    core: Arc<Core>,
    commands: Receiver<Control>,
    preview: PreviewMailbox,
    state: Arc<Mutex<Playback>>,
    gui: egui::Context,
) {
    let (wake, events) = bounded::<()>(1);
    let mut wake = Box::new(wake);
    let handle = core.handle.as_ptr();
    unsafe {
        (core.api.mpv_set_wakeup_callback)(
            handle,
            Some(wake_client),
            std::ptr::from_mut(&mut *wake).cast(),
        );
        for (tag, property, format) in [
            (1, c"time-pos", ffi::DOUBLE),
            (2, c"duration", ffi::DOUBLE),
            (3, c"pause", ffi::FLAG),
            (4, c"volume", ffi::DOUBLE),
            (5, c"mute", ffi::FLAG),
            (6, c"eof-reached", ffi::FLAG),
            (7, c"speed", ffi::DOUBLE),
            (8, c"sub-visibility", ffi::FLAG),
            (9, c"aid", ffi::STRING),
            (10, c"sid", ffi::STRING),
            (13, c"vid", ffi::STRING),
            (14, c"secondary-sid", ffi::STRING),
            (11, c"video-out-params/aspect", ffi::DOUBLE),
            (12, c"video-out-params/rotate", ffi::DOUBLE),
        ] {
            (core.api.mpv_observe_property)(handle, tag, property.as_ptr(), format);
        }
    }
    let mut expected = None;
    let mut started = None;
    let mut restarted = false;
    let mut precise: Option<PrecisePlayback> = None;
    loop {
        select! {
            recv(commands) -> command => {
                let Ok(command) = command else { break; };
                if matches!(command, Control::Exit) { break; }
                let snapshot = state.lock().unwrap().clone();
                let load_id = snapshot.load_id;
                if precise.as_ref().is_some_and(|seek| seek.id.load != load_id) {
                    precise = None;
                }
                let result = match command {
                    Control::Inspect(id, custom) => {
                        if id == load_id && snapshot.ready {
                            let rows: Vec<_> = options::INSPECT_PROPERTIES.iter().copied()
                                .chain(custom.iter().map(String::as_str))
                                .map(|k| (k.to_owned(),core.property(k).unwrap_or_else(|| "—".into())))
                                .collect();
                            let mut current = state.lock().unwrap();
                            if current.load_id == id { current.inspection = rows.into(); }
                            gui.request_repaint_of(egui::ViewportId::ROOT);
                        }
                        Ok(())
                    }
                    Control::Equalizer(gains) => {
                        if let Err(error) = core.equalizer(gains) {
                            state.lock().unwrap().tool_notice = Some((format!("Equalizer: {error}"),true));
                            gui.request_repaint_of(egui::ViewportId::ROOT);
                        }
                        Ok(())
                    }
                    Control::Configure(key, value) => {
                        let result = CString::new(value).map_err(anyhow::Error::from).and_then(|value| {
                            let property = CString::new(key)?;
                            core.command_sync(&[c"set", &property, &value])
                        });
                        if let Err(error) = result { state.lock().unwrap().tool_notice = Some((format!("{key}: {error}"), true)); gui.request_repaint_of(egui::ViewportId::ROOT); }
                        Ok(())
                    }
                    Control::Tool(id, tool) => {
                        if id == load_id && snapshot.ready && !snapshot.precise_seeking {
                            apply_tool(&core, id, tool, &state, &gui);
                        }
                        Ok(())
                    }
                    Control::Load(load) if load.id == load_id => {
                        expected = None;
                        precise = None;
                        core.load(load).map(|entry| expected = Some((entry, load_id)))
                    }
                    Control::Load(_) => Ok(()),
                    Control::BeginPrecise(id, at)
                        if id.load == load_id && snapshot.ready && precise.is_none() =>
                    {
                        let mut current = state.lock().unwrap();
                        if current.load_id != id.load {
                            continue;
                        }
                        current.precise_seeking = true;
                        precise = Some(PrecisePlayback::new(id, &current, at));
                        drop(current);
                        set_paused(&core, true)
                    }
                    Control::PreviewPrecise(id) => {
                        let pending = preview.lock().unwrap().take_if(|(queued, _)| *queued == id);
                        if let Some((seek, (_, at))) = precise.as_mut()
                            .filter(|seek| seek.id == id).zip(pending)
                        {
                            seek.preview(at);
                        }
                        Ok(())
                    }
                    Control::FinishPrecise(id, commit) => {
                        if let Some(seek) = precise.as_mut().filter(|seek| seek.id == id) {
                            seek.finish(commit);
                        }
                        Ok(())
                    }
                    Control::Stop => {
                        precise = None;
                        run_control(&core, command, snapshot.ended)
                    }
                    Control::Toggle | Control::SetPaused(_) | Control::Seek(_) | Control::Frame(_)
                        if precise.is_some() => Ok(()),
                    command => run_control(&core, command, snapshot.ended),
                };
                if let Err(error) = result {
                    playback_error(&state, load_id, error);
                    gui.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            recv(events) -> _ => {
                let mut changed = false;
                loop {
                    // Each event's data belongs to mpv until the next call on this handle.
                    let event = unsafe { &*(core.api.mpv_wait_event)(handle, 0.0) };
                    if event.id == ffi::EVENT_NONE { break; }
                    if event.id == ffi::EVENT_START_FILE && !event.data.is_null() {
                        started = Some(unsafe { *event.data.cast::<i64>() });
                        restarted = false;
                        continue;
                    }
                    let file = expected.filter(|(entry, _)| match event.id {
                        ffi::EVENT_END_FILE if !event.data.is_null() => {
                            unsafe { (*event.data.cast::<ffi::EndFile>()).playlist_entry_id == *entry }
                        }
                        _ => started == Some(*entry),
                    });
                    // Reading a media property may need the core thread; never hold the UI state lock.
                    let metadata = (file.is_some() && event.id == ffi::EVENT_FILE_LOADED)
                        .then(|| (core.tracks(), core.chapters()));
                    let position = (file.is_some() && event.id == ffi::EVENT_PLAYBACK_RESTART)
                        .then(|| core.property("time-pos").and_then(|at| at.parse::<f64>().ok()))
                        .flatten().filter(|at| at.is_finite());
                    let mut current = state.lock().unwrap();
                    let matches_file = file.is_some_and(|(_, id)| id == current.load_id);
                    let seek = precise.as_mut().filter(|seek| seek.id.load == current.load_id);
                    let previewing = seek.is_some();
                    let mut finish = None;
                    if matches_file {
                        if let Some((tracks, chapters)) = metadata {
                            current.tracks = tracks;
                            current.chapters = chapters;
                        }
                        if event.id == ffi::EVENT_SEEK {
                            if !previewing && restarted { current.seeks = current.seeks.wrapping_add(1); }
                            restarted = false;
                            if let Some(seek) = seek {
                                seek.seeking();
                            }
                        } else if event.id == ffi::EVENT_PLAYBACK_RESTART {
                            restarted = true;
                            if !current.frame_ready {
                                current.frame_ready = true;
                                changed = true;
                            }
                            finish = seek.and_then(PrecisePlayback::restarted);
                            if (!previewing || matches!(finish, Some(Target::Finish { .. })))
                                && let Some(at) = position
                            {
                                current.position = at;
                                current.confirmed_position = Some(at);
                            }
                        }
                    }
                    changed |= position.is_some() && matches_file;
                    changed |= update_state(&core, &mut current, event, matches_file, restarted, previewing);
                    if let Some(Target::Finish { paused, commit, .. }) = finish {
                        let load = current.load_id;
                        current.precise_seeking = false;
                        current.paused = paused;
                        if commit {
                            current.ended = false;
                        }
                        precise = None;
                        drop(current);
                        if let Err(error) = set_paused(&core, paused) {
                            playback_error(&state, load, error);
                        }
                        changed = true;
                    }
                }
                if changed { gui.request_repaint_of(egui::ViewportId::ROOT); }
            }
        }
        if let Some(seek) = &mut precise
            && state.lock().unwrap().load_id == seek.id.load
            && let Some(target) = seek.next_seek()
            && let Err(error) = seek_exact(&core, target.position(), true)
        {
            playback_error(&state, seek.id.load, error);
            precise = None;
            gui.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
    unsafe { (core.api.mpv_set_wakeup_callback)(handle, None, std::ptr::null_mut()) };
}

fn playback_error(state: &Mutex<Playback>, load: u64, error: anyhow::Error) {
    let mut current = state.lock().unwrap();
    if current.load_id == load {
        current.precise_seeking = false;
        current.error = Some(format!("{error:#}"));
    }
}

fn set_paused(core: &Core, paused: bool) -> Result<()> {
    core.command_sync(&[c"set", c"pause", if paused { c"yes" } else { c"no" }])
}

fn seek_exact(core: &Core, position: f64, sync: bool) -> Result<()> {
    let position = CString::new(format!("{:.3}", position.max(0.0)))?;
    let args = [c"seek", position.as_c_str(), c"absolute+exact"];
    if sync {
        core.command_sync(&args)
    } else {
        core.command(&args, 0)
    }
}

fn run_control(core: &Core, command: Control, ended: bool) -> Result<()> {
    let restart = || {
        core.command(&[c"seek", c"0", c"absolute"], 0)?;
        core.command(&[c"set", c"pause", c"no"], 0)
    };
    match command {
        Control::Toggle if ended => restart(),
        Control::Toggle => core.command(&[c"cycle", c"pause"], 0),
        Control::SetPaused(false) if ended => restart(),
        Control::SetPaused(paused) => {
            core.command(&[c"set", c"pause", if paused { c"yes" } else { c"no" }], 0)
        }
        Control::Seek(position) => seek_exact(core, position, false),
        Control::Volume(volume) => {
            let volume = CString::new(format!("{:.0}", volume.clamp(0.0, 100.0)))?;
            core.command(&[c"set", c"volume", &volume], 0)
        }
        Control::AdjustVolume(amount) => {
            let amount = CString::new(amount.to_string())?;
            core.command(&[c"add", c"volume", &amount], 0)
        }
        Control::Mute => core.command(&[c"cycle", c"mute"], 0),
        Control::Speed(speed) => {
            let speed = CString::new(format!("{:.2}", stepped_speed(speed)))?;
            core.command(&[c"set", c"speed", &speed], 0)
        }
        Control::Captions(visible) => core.command(
            &[
                c"set",
                c"sub-visibility",
                if visible { c"yes" } else { c"no" },
            ],
            0,
        ),
        Control::SubtitlePosition(position) => {
            let position = CString::new(position.to_string())?;
            core.command(&[c"set", c"sub-pos", &position], 0)
        }
        Control::Track(kind, id) => {
            let value = CString::new(id.map_or_else(|| "no".into(), |id| id.to_string()))?;
            let property = match kind {
                TrackKind::Audio => c"aid",
                TrackKind::Video => c"vid",
                TrackKind::Subtitle => c"sid",
            };
            core.command(&[c"set", property, &value], 0)?;
            if kind == TrackKind::Subtitle && id.is_some() {
                core.command(&[c"set", c"sub-visibility", c"yes"], 0)?;
            }
            Ok(())
        }
        Control::Repeat(enabled) => core.command(
            &[c"set", c"loop-file", if enabled { c"inf" } else { c"no" }],
            0,
        ),
        Control::Frame(forward) => core.command(
            &[if forward {
                c"frame-step"
            } else {
                c"frame-back-step"
            }],
            0,
        ),
        Control::Stop => core.command_sync(&[c"stop"]),
        Control::Load(_)
        | Control::Tool(..)
        | Control::Inspect(..)
        | Control::Equalizer(_)
        | Control::Configure(..)
        | Control::BeginPrecise(..)
        | Control::PreviewPrecise(_)
        | Control::FinishPrecise(..)
        | Control::Exit => Ok(()),
    }
}

fn update_state(
    core: &Core,
    state: &mut Playback,
    event: &ffi::Event,
    matches_file: bool,
    restarted: bool,
    previewing: bool,
) -> bool {
    match event.id {
        ffi::EVENT_FILE_LOADED if matches_file => state.ready = true,
        ffi::EVENT_END_FILE if matches_file && !event.data.is_null() => {
            let ended = unsafe { &*event.data.cast::<ffi::EndFile>() };
            if ended.reason == 0 {
                if !previewing {
                    state.ended = true;
                }
            } else if ended.reason == 4 {
                state.error = Some(format!("Playback stopped: {}", core.error(ended.error)));
            }
        }
        ffi::EVENT_SHUTDOWN => state.error = Some("The playback engine stopped.".into()),
        ffi::EVENT_PROPERTY_CHANGE if !event.data.is_null() => {
            if !matches_file && !matches!(event.reply, 4 | 5 | 7 | 8) {
                return false;
            }
            let property = unsafe { &*event.data.cast::<ffi::Property>() };
            if property.data.is_null() {
                return false;
            }
            if property.format == ffi::DOUBLE {
                let value = unsafe { *property.data.cast::<f64>() };
                if !value.is_finite() {
                    return false;
                }
                match event.reply {
                    1 if !previewing => {
                        state.position = value;
                        if restarted && state.ready {
                            state.confirmed_position = Some(value);
                        }
                    }
                    2 => state.duration = value.max(0.0),
                    4 => state.volume = value,
                    7 => state.speed = value,
                    11 => state.video_aspect = (value > 0.0).then_some(value as f32),
                    12 => state.video_rotated = value.rem_euclid(180.0) == 90.0,
                    _ => return false,
                }
            } else if property.format == ffi::FLAG {
                let value = unsafe { *property.data.cast::<i32>() != 0 };
                match event.reply {
                    3 => state.paused = value,
                    5 => state.muted = value,
                    6 if !previewing => state.ended = value,
                    8 => state.captions = value,
                    _ => return false,
                }
            } else if property.format == ffi::STRING {
                let ptr = unsafe { *property.data.cast::<*const c_char>() };
                let value = if ptr.is_null() {
                    None
                } else {
                    unsafe { CStr::from_ptr(ptr) }
                        .to_str()
                        .ok()
                        .and_then(|name| name.parse().ok())
                };
                match event.reply {
                    9 => state.audio = value,
                    10 => state.subtitle = value,
                    13 => state.video_track = value,
                    14 => state.secondary_subtitle = value,
                    _ => return false,
                }
            } else {
                return false;
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    #[ignore = "requires the installed FFmpeg and libmpv runtime"]
    fn native_tools_apply_to_decoded_video() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let clip = dir.path().join("tools.mkv");
        let encoded = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440",
                "-t",
                "2",
                "-c:v",
                "mpeg4",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&clip)
            .status()?;
        ensure!(encoded.success(), "could not generate fixture");
        let core = Core::open()?;
        core.command_sync(&[c"set", c"vo", c"null"])?;
        core.command_sync(&[c"set", c"ao", c"null"])?;
        core.command_sync(&[c"set", c"pause", c"yes"])?;
        core.command_sync(&[
            c"loadfile",
            &CString::new(clip.as_os_str().as_encoded_bytes())?,
        ])?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            ensure!(Instant::now() < deadline, "fixture did not start");
            let event = unsafe { &*(core.api.mpv_wait_event)(core.handle.as_ptr(), 0.05) };
            if event.id == ffi::EVENT_PLAYBACK_RESTART {
                break;
            }
        }
        for option in options::OPTIONS {
            assert!(
                option.accepts(option.default),
                "invalid default for {}",
                option.key
            );
            // Debian 12's libmpv predates these two subtitle properties.
            if !core.supports_option(option.key) {
                ensure!(
                    matches!(option.key, "sub-border-style" | "secondary-sub-delay"),
                    "missing required playback option {}",
                    option.key
                );
                continue;
            }
            core.command_sync(&[
                c"set",
                &CString::new(option.key)?,
                &CString::new(option.default)?,
            ])
            .with_context(|| format!("applying the default for {}", option.key))?;
        }
        let filter = |key: &str| core.property(key).unwrap_or_default();
        for speed in [1.75, 0.25, 1.45, 2.0, 3.65, 4.0, 1.0] {
            run_control(&core, Control::Speed(speed), false)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while filter("speed").parse::<f64>().ok() != Some(speed) {
                ensure!(
                    Instant::now() < deadline,
                    "playback speed did not reach {speed}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        core.equalizer([3; 10])?;
        assert!(filter("af").contains("mytube-eq"));
        core.equalizer([0; 10])?;
        assert!(!filter("af").contains("mytube-eq"));
        core.tool(PlayerTool::Flip(true))?;
        assert!(filter("vf").contains("mytube-mirror"));
        core.tool(PlayerTool::Flip(true))?;
        assert!(!filter("vf").contains("mytube-mirror"));
        core.tool(PlayerTool::Crop(Some((1, 1))))?;
        assert!(filter("vf").contains("mytube-crop"));
        core.tool(PlayerTool::Crop(None))?;
        core.tool(PlayerTool::Delogo(Some([5, 5, 10, 10])))?;
        assert!(filter("vf").contains("mytube-delogo"));
        core.tool(PlayerTool::Delogo(None))?;
        assert!(!filter("vf").contains("mytube-delogo"));
        assert!(!filter("vf").contains("mytube-crop"));
        let script = dir.path().join("layout_test.lua");
        fs::write(&script, r#"mp.set_property_number("volume", 42)"#)?;
        core.tool(PlayerTool::LoadScript(script))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while core.property("volume").and_then(|v| v.parse::<f64>().ok()) != Some(42.0) {
            ensure!(
                Instant::now() < deadline,
                "The local mpv script did not run"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let captions = dir.path().join("captions.srt");
        fs::write(&captions, "1\n00:00:00,000 --> 00:00:02,000\nSubtitle\n")?;
        core.tool(PlayerTool::ExternalTrack(TrackKind::Subtitle, captions))?;
        assert!(core.tracks().iter().any(|t| t.kind == TrackKind::Subtitle));
        let image = dir.path().join("shot.png");
        core.tool(PlayerTool::Screenshot(image.clone(), false))?;
        assert!(fs::read(image)?.starts_with(b"\x89PNG\r\n\x1a\n"));
        Ok(())
    }
}
