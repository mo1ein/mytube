use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolbarItem {
    Autoplay,
    Theater,
    Mini,
    Captions,
    Playlist,
    Settings,
    Fullscreen,
    Music,
    Screenshot,
    Inspector,
    Text,
}
impl ToolbarItem {
    pub const GLASS_TRAILING: [Self; 4] = [
        Self::Captions,
        Self::Settings,
        Self::Theater,
        Self::Fullscreen,
    ];

    pub fn controller_items(saved: &[Self], glass: bool, fullscreen: bool) -> Vec<Self> {
        let mut items = vec![Self::Autoplay];
        items.extend(saved.iter().copied().filter(|item| {
            *item != Self::Autoplay
                && (!glass
                    || ((fullscreen || *item != Self::Playlist)
                        && !Self::GLASS_TRAILING.contains(item)))
        }));
        if glass {
            items.extend(
                Self::GLASS_TRAILING
                    .into_iter()
                    .filter(|item| saved.contains(item)),
            );
        }
        items
    }

    pub const DEFAULTS: [Self; 6] = [
        Self::Theater,
        Self::Mini,
        Self::Captions,
        Self::Playlist,
        Self::Settings,
        Self::Fullscreen,
    ];
    pub fn all() -> impl Iterator<Item = Self> {
        Self::DEFAULTS
            .into_iter()
            .chain([Self::Music, Self::Screenshot, Self::Inspector, Self::Text])
            .filter(|item| *item != Self::Text || cfg!(target_os = "macos"))
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Autoplay => "Autoplay",
            Self::Theater => "Theater",
            Self::Mini => "Mini player",
            Self::Captions => "Captions",
            Self::Playlist => "Playlist",
            Self::Settings => "Settings",
            Self::Fullscreen => "Fullscreen",
            Self::Music => "Music mode",
            Self::Screenshot => "Save screenshot",
            Self::Inspector => "Inspector",
            Self::Text => "Read text from frame",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub key: String,
    pub alt: bool,
    pub shift: bool,
    pub control: bool,
    pub command: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterfaceTheme {
    #[cfg_attr(not(target_os = "macos"), default)]
    Classic,
    #[cfg_attr(target_os = "macos", default)]
    LiquidGlass,
    GlassOpaque,
}

impl InterfaceTheme {
    pub const ALL: [Self; 3] = [Self::Classic, Self::LiquidGlass, Self::GlassOpaque];

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "YouTube",
            Self::LiquidGlass => "Liquid Glass",
            Self::GlassOpaque => "Glass UI (Opaque)",
        }
    }

    pub fn glass_ui(self) -> bool {
        self != Self::Classic
    }
    pub fn translucent(self) -> bool {
        self == Self::LiquidGlass
    }
}

pub const DEFAULT_GLASS_STRENGTH: u8 = 62;
pub const DEFAULT_HIDE_DELAY_MS: u32 = 700;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerPreferences {
    pub window: WindowPreferences,
    pub interface_theme: InterfaceTheme,
    pub progress_bar_style: Option<InterfaceTheme>,
    pub classic_layout: crate::model::ControlBarLayout,
    pub glass_strength: u8,
    pub glass: bool,
    pub glass_osd: bool,
    pub glass_sidebar: bool,
    pub compact_interface: bool,
    pub edge_to_edge: bool,
    pub lock_aspect: bool,
    pub settings_left: bool,
    pub plugins_left: bool,
    pub snap_controller: bool,
    pub speed_adjusted_remaining: bool,
    pub reduce_motion: bool,
    pub fullscreen_clock: bool,
    pub thumbnail_width: u16,
    pub thumbnail_cache_mb: u16,
    pub remote_thumbnails: bool,
    pub wheel_seek: bool,
    pub feedback_duration_ms: u32,
    pub feedback_size: u8,
    pub transport: Transport,
    pub toolbar: Vec<ToolbarItem>,
    pub playlist_left: bool,
    pub sidebar_width: u32,
    pub caption_position: u8,
    pub pause_on_inactive: bool,
    pub pause_leaving_fullscreen: bool,
    pub play_entering_fullscreen: bool,
    pub show_chapters: bool,
    pub auto_hide: bool,
    pub hide_delay_ms: u32,
    pub playback_feedback: bool,
    pub feedback_on_open: bool,
    pub feedback_on_pause: bool,
    pub feedback_on_seek: bool,
    pub feedback_on_speed: bool,
    pub hover_previews: bool,
    pub resume: bool,
    pub pause_on_open: bool,
    pub pause_on_minimize: bool,
    pub detached_mini: bool,
    pub pip_on_os_minimize: bool,
    pub pip_video_only: bool,
    pub pip_parent: PipParent,
    pub always_on_top: bool,
    pub float_playing: bool,
    pub show_float_status: bool,
    pub autoplay: bool,
    pub loop_playlist: bool,
    pub record_history: bool,
    pub remaining_time: bool,
    pub arrow_seek_seconds: u32,
    pub wheel_volume: bool,
    pub screenshot_subtitles: bool,
    pub screenshot_format: String,
    pub equalizer: [i8; 10],
    pub eq_presets: BTreeMap<String, [i8; 10]>,
    pub watched_properties: Vec<String>,
    pub filters: Vec<FilterPreset>,
    pub next_filter_id: u64,
    pub click: ClickAction,
    pub double_click: ClickAction,
    pub middle_click: ClickAction,
    pub bindings: BTreeMap<String, Binding>,
    pub engine: BTreeMap<String, String>,
}

impl Default for PlayerPreferences {
    fn default() -> Self {
        Self {
            window: WindowPreferences::default(),
            interface_theme: InterfaceTheme::default(),
            progress_bar_style: None,
            classic_layout: crate::model::ControlBarLayout::Classic,
            glass_strength: DEFAULT_GLASS_STRENGTH,
            glass: true,
            glass_osd: true,
            glass_sidebar: true,
            compact_interface: false,
            edge_to_edge: true,
            lock_aspect: false,
            settings_left: false,
            plugins_left: true,
            snap_controller: true,
            speed_adjusted_remaining: false,
            reduce_motion: false,
            fullscreen_clock: false,
            thumbnail_width: 176,
            thumbnail_cache_mb: 16,
            remote_thumbnails: true,
            wheel_seek: false,
            feedback_duration_ms: 650,
            feedback_size: 14,
            transport: Transport::Playlist,
            toolbar: ToolbarItem::DEFAULTS.to_vec(),
            playlist_left: false,
            sidebar_width: 280,
            caption_position: 100,
            pause_on_inactive: false,
            pause_leaving_fullscreen: false,
            play_entering_fullscreen: false,
            show_chapters: true,
            auto_hide: true,
            hide_delay_ms: DEFAULT_HIDE_DELAY_MS,
            playback_feedback: true,
            feedback_on_open: false,
            feedback_on_pause: true,
            feedback_on_seek: false,
            feedback_on_speed: true,
            hover_previews: true,
            resume: true,
            pause_on_open: false,
            pause_on_minimize: false,
            detached_mini: false,
            pip_on_os_minimize: false,
            pip_video_only: true,
            pip_parent: PipParent::Keep,
            always_on_top: false,
            float_playing: false,
            show_float_status: false,
            autoplay: false,
            loop_playlist: false,
            record_history: false,
            remaining_time: false,
            arrow_seek_seconds: 5,
            wheel_volume: true,
            screenshot_subtitles: true,
            screenshot_format: "png".into(),
            equalizer: [0; 10],
            eq_presets: BTreeMap::new(),
            watched_properties: Vec::new(),
            filters: Vec::new(),
            next_filter_id: 1,
            click: ClickAction::Toggle,
            double_click: ClickAction::Fullscreen,
            middle_click: ClickAction::None,
            bindings: BTreeMap::new(),
            engine: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPreferences {
    pub initial_size: bool,
    pub size_percent: bool,
    pub size: [u16; 2],
    pub initial_position: bool,
    pub offset_percent: bool,
    pub offset: [u16; 2],
    pub right: bool,
    pub bottom: bool,
}

impl Default for WindowPreferences {
    fn default() -> Self {
        Self {
            initial_size: false,
            size_percent: false,
            size: [1280, 720],
            initial_position: false,
            offset_percent: false,
            offset: [0, 0],
            right: false,
            bottom: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Interface,
    Playback,
    Video,
    Audio,
    Subtitles,
    Tools,
    Inspector,
    Controls,
    Filters,
    Plugins,
}
impl Section {
    pub const ALL: [Self; 10] = [
        Self::Interface,
        Self::Playback,
        Self::Video,
        Self::Audio,
        Self::Subtitles,
        Self::Controls,
        Self::Tools,
        Self::Inspector,
        Self::Filters,
        Self::Plugins,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Interface => "General",
            Self::Playback => "Playback",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Subtitles => "Subtitles",
            Self::Tools => "Tools",
            Self::Inspector => "Inspector",
            Self::Controls => "Controls",
            Self::Filters => "Filters",
            Self::Plugins => "Plugins",
        }
    }
}

pub enum Editor {
    Choice(&'static [(&'static str, &'static str)]),
    Number(f64, f64, f64),
    Text,
    Color,
}

pub struct EngineOption {
    pub section: Section,
    pub key: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    pub editor: Editor,
}

impl EngineOption {
    pub fn value<'a>(&self, prefs: &'a PlayerPreferences) -> &'a str {
        prefs
            .engine
            .get(self.key)
            .map_or(self.default, String::as_str)
    }
    pub fn accepts(&self, value: &str) -> bool {
        match self.editor {
            Editor::Choice(choices) => choices.iter().any(|(key, _)| *key == value),
            Editor::Number(min, max, _) => {
                value.parse::<f64>().is_ok_and(|v| (min..=max).contains(&v))
            }
            Editor::Color => value
                .strip_prefix('#')
                .is_some_and(|v| v.len() == 8 && v.bytes().all(|c| c.is_ascii_hexdigit())),
            Editor::Text => value.len() <= 512 && !value.contains('\0'),
        }
    }
}

const SWITCH: &[(&str, &str)] = &[("yes", "On"), ("no", "Off")];
const fn option(
    section: Section,
    key: &'static str,
    label: &'static str,
    default: &'static str,
    editor: Editor,
) -> EngineOption {
    EngineOption {
        section,
        key,
        label,
        default,
        editor,
    }
}
use Editor::{Choice, Color, Number, Text};
use Section::{Audio, Subtitles, Video};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipParent {
    Keep,
    Minimize,
    Hide,
}

impl PipParent {
    pub fn title(self) -> &'static str {
        match self {
            Self::Keep => "Keep library visible",
            Self::Minimize => "Minimize library",
            Self::Hide => "Hide library",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    Playlist,
    Seek,
    Speed,
}

impl Transport {
    pub fn title(self) -> &'static str {
        match self {
            Self::Playlist => "Previous / next video",
            Self::Seek => "Rewind / fast forward",
            Self::Speed => "Decrease / increase speed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterPreset {
    pub id: u64,
    pub name: String,
    pub expression: String,
    pub audio: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClickAction {
    None,
    Toggle,
    Fullscreen,
    Mini,
    Mute,
    Loop,
}
impl ClickAction {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Toggle,
        Self::Fullscreen,
        Self::Mini,
        Self::Mute,
        Self::Loop,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::None => "No action",
            Self::Toggle => "Play / pause",
            Self::Fullscreen => "Fullscreen",
            Self::Mini => "Mini player",
            Self::Mute => "Mute",
            Self::Loop => "Set A / B loop point",
        }
    }
}
pub const EQ_FREQUENCIES: [u16; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
pub const INSPECT_PROPERTIES: &[&str] = &[
    "media-title",
    "file-format",
    "video-codec",
    "video-params/w",
    "video-params/h",
    "video-params/pixelformat",
    "video-params/primaries",
    "video-params/gamma",
    "hwdec-current",
    "container-fps",
    "video-bitrate",
    "audio-codec-name",
    "audio-params/samplerate",
    "audio-params/channels",
    "audio-bitrate",
    "current-ao",
    "avsync",
    "frame-drop-count",
    "vf",
    "af",
];
pub static OPTIONS: &[EngineOption] = &[
    option(
        Video,
        "video-aspect-override",
        "Aspect ratio",
        "-1",
        Choice(&[
            ("-1", "Original"),
            ("1.333333", "4:3"),
            ("1.777778", "16:9"),
            ("1.6", "16:10"),
            ("1", "Square"),
            ("2.35", "2.35:1"),
            ("2.39", "2.39:1"),
        ]),
    ),
    option(
        Video,
        "video-rotate",
        "Rotation",
        "0",
        Choice(&[
            ("0", "Original"),
            ("90", "90°"),
            ("180", "180°"),
            ("270", "270°"),
        ]),
    ),
    option(Video, "video-zoom", "Zoom", "0", Number(-2., 3., 0.05)),
    option(
        Video,
        "video-pan-x",
        "Pan horizontally",
        "0",
        Number(-1., 1., 0.01),
    ),
    option(
        Video,
        "video-pan-y",
        "Pan vertically",
        "0",
        Number(-1., 1., 0.01),
    ),
    option(
        Video,
        "brightness",
        "Brightness",
        "0",
        Number(-100., 100., 1.),
    ),
    option(Video, "contrast", "Contrast", "0", Number(-100., 100., 1.)),
    option(
        Video,
        "saturation",
        "Saturation",
        "0",
        Number(-100., 100., 1.),
    ),
    option(Video, "gamma", "Gamma", "0", Number(-100., 100., 1.)),
    option(Video, "hue", "Hue", "0", Number(-100., 100., 1.)),
    option(Video, "sharpen", "Sharpness", "0", Number(-1., 1., 0.05)),
    option(
        Video,
        "hwdec",
        "Hardware decoding",
        "auto-safe",
        Choice(&[
            ("auto-safe", "Automatic"),
            ("auto-copy", "Hardware with copy-back"),
            ("no", "Software"),
        ]),
    ),
    option(
        Video,
        "deinterlace",
        "Deinterlacing",
        "no",
        Choice(&[("no", "Off"), ("yes", "On"), ("auto", "Automatic")]),
    ),
    option(
        Video,
        "tone-mapping",
        "HDR tone mapping",
        "auto",
        Choice(&[
            ("auto", "Automatic"),
            ("clip", "Clip"),
            ("mobius", "Mobius"),
            ("reinhard", "Reinhard"),
            ("hable", "Hable"),
            ("bt.2390", "BT.2390"),
            ("linear", "Linear"),
            ("gamma", "Gamma"),
        ]),
    ),
    option(
        Audio,
        "audio-delay",
        "Audio delay (s)",
        "0",
        Number(-10., 10., 0.05),
    ),
    option(
        Audio,
        "audio-pitch-correction",
        "Preserve pitch at different speeds",
        "yes",
        Choice(SWITCH),
    ),
    option(
        Audio,
        "audio-channels",
        "Output channels",
        "auto-safe",
        Choice(&[
            ("auto-safe", "Automatic"),
            ("mono", "Mono"),
            ("stereo", "Stereo"),
            ("5.1", "5.1 surround"),
            ("7.1", "7.1 surround"),
        ]),
    ),
    option(
        Audio,
        "replaygain",
        "ReplayGain",
        "no",
        Choice(&[("no", "Off"), ("track", "Track"), ("album", "Album")]),
    ),
    option(
        Audio,
        "replaygain-preamp",
        "ReplayGain preamp (dB)",
        "0",
        Number(-15., 15., 0.5),
    ),
    option(Audio, "alang", "Preferred audio languages", "", Text),
    option(
        Audio,
        "audio-exclusive",
        "Exclusive audio output",
        "no",
        Choice(SWITCH),
    ),
    option(
        Audio,
        "gapless-audio",
        "Gapless audio",
        "weak",
        Choice(&[
            ("no", "Off"),
            ("weak", "When compatible"),
            ("yes", "Always"),
        ]),
    ),
    option(Audio, "audio-spdif", "Passthrough codecs", "", Text),
    option(
        Subtitles,
        "sub-align-x",
        "Horizontal alignment",
        "center",
        Choice(&[("left", "Left"), ("center", "Center"), ("right", "Right")]),
    ),
    option(
        Subtitles,
        "sub-align-y",
        "Vertical alignment",
        "bottom",
        Choice(&[("top", "Top"), ("center", "Center"), ("bottom", "Bottom")]),
    ),
    option(Subtitles, "sub-color", "Text color", "#FFFFFFFF", Color),
    option(
        Subtitles,
        "sub-border-color",
        "Outline color",
        "#FF000000",
        Color,
    ),
    option(
        Subtitles,
        "sub-back-color",
        "Background color",
        "#AF000000",
        Color,
    ),
    option(
        Subtitles,
        "sub-border-style",
        "Border style",
        "outline-and-shadow",
        Choice(&[
            ("outline-and-shadow", "Outline and shadow"),
            ("opaque-box", "Opaque box"),
            ("background-box", "Background box"),
        ]),
    ),
    option(
        Subtitles,
        "sub-auto",
        "Load subtitles beside the video",
        "exact",
        Choice(&[
            ("no", "Off"),
            ("exact", "Matching name"),
            ("fuzzy", "Similar names"),
            ("all", "All subtitles"),
        ]),
    ),
    option(Subtitles, "sub-codepage", "Subtitle encoding", "auto", Text),
    option(
        Subtitles,
        "secondary-sub-delay",
        "Secondary subtitle delay (s)",
        "0",
        Number(-10., 10., 0.05),
    ),
    option(Subtitles, "slang", "Preferred subtitle languages", "", Text),
    option(
        Subtitles,
        "sub-delay",
        "Subtitle delay (s)",
        "0",
        Number(-10., 10., 0.05),
    ),
    option(
        Subtitles,
        "sub-scale",
        "Subtitle scale",
        "1",
        Number(0.1, 5., 0.05),
    ),
    option(Subtitles, "sub-font", "Font family", "sans-serif", Text),
    option(
        Subtitles,
        "sub-font-size",
        "Font size",
        "38",
        Number(8., 120., 1.),
    ),
    option(Subtitles, "sub-bold", "Bold", "no", Choice(SWITCH)),
    option(Subtitles, "sub-italic", "Italic", "no", Choice(SWITCH)),
    option(
        Subtitles,
        "sub-border-size",
        "Outline width",
        "3",
        Number(0., 10., 0.25),
    ),
    option(
        Subtitles,
        "sub-shadow-offset",
        "Shadow offset",
        "0",
        Number(0., 10., 0.25),
    ),
    option(
        Subtitles,
        "sub-scale-with-window",
        "Scale text with the window",
        "yes",
        Choice(SWITCH),
    ),
    option(
        Subtitles,
        "sub-ass-override",
        "ASS subtitle styles",
        "scale",
        Choice(&[
            ("no", "Use embedded styles"),
            ("scale", "Scale embedded styles"),
            ("force", "Use player style"),
            ("strip", "Plain text"),
        ]),
    ),
];
