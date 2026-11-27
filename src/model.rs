use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub const PAGE: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub bytes: i64,
    pub modified: i64,
}

impl Stamp {
    pub fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            bytes: metadata.len().min(i64::MAX as u64) as i64,
            modified: metadata.modified().ok().map_or(0, unix_nanos),
        }
    }

    pub fn read(path: &Path) -> std::io::Result<Self> {
        Self::inspect(path).map(|(stamp, _)| stamp)
    }

    pub fn inspect(path: &Path) -> std::io::Result<(Self, Option<i64>)> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            return Err(std::io::Error::other(
                "The path is no longer a regular file",
            ));
        }
        Ok((Self::from_metadata(&metadata), creation_time(&metadata)))
    }
}

pub fn creation_time(metadata: &fs::Metadata) -> Option<i64> {
    metadata.created().ok().map(unix_nanos)
}

fn unix_nanos(time: SystemTime) -> i64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(age) => age.as_nanos().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_nanos().min(i64::MAX as u128) as i64),
    }
}

#[derive(Clone, Debug)]
pub struct Video {
    pub id: i64,
    pub rev: i64,
    pub path: PathBuf,
    pub title: String,
    pub ext: String,
    pub stamp: Stamp,
    pub added: i64,
    pub created: Option<i64>,
    pub media: Media,
}

#[derive(Clone, Debug, Default)]
pub struct Media {
    pub duration: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub codec: Option<String>,
    pub audio: Option<String>,
    pub fps: Option<f64>,
}

impl Video {
    pub fn thumbnail_key(&self) -> (i64, i64) {
        (self.id, self.rev)
    }
}

#[derive(Clone, Debug)]
pub struct Collection {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub id: i64,
    pub path: PathBuf,
    pub automatic: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Recent,
    AddedOldest,
    CreatedNewest,
    CreatedOldest,
    ModifiedNewest,
    ModifiedOldest,
    Title,
    Duration,
    Size,
    Smallest,
}

impl Sort {
    pub const ALL: [Self; 10] = [
        Self::Recent,
        Self::AddedOldest,
        Self::CreatedNewest,
        Self::CreatedOldest,
        Self::ModifiedNewest,
        Self::ModifiedOldest,
        Self::Title,
        Self::Duration,
        Self::Size,
        Self::Smallest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "Added · Latest",
            Self::AddedOldest => "Added · Earliest",
            Self::CreatedNewest => "Created · Latest",
            Self::CreatedOldest => "Created · Earliest",
            Self::ModifiedNewest => "Modified · Latest",
            Self::ModifiedOldest => "Modified · Earliest",
            Self::Title => "Title A–Z",
            Self::Duration => "Longest first",
            Self::Size => "Size · Largest",
            Self::Smallest => "Size · Smallest",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub text: String,
    pub filename: String,
    pub folder: String,
    pub ext: String,
    pub codec: String,
    pub min_seconds: Option<i64>,
    pub max_seconds: Option<i64>,
    pub min_height: Option<i64>,
    pub max_height: Option<i64>,
    pub added_since: Option<i64>,
    pub dates: [DateRange; 3],
    pub min_bytes: Option<i64>,
    pub directory: Option<PathBuf>,
    pub collection: Option<i64>,
    pub watch_later: bool,
    pub source: Option<i64>,
    pub sort: Sort,
}

impl Filter {
    pub fn narrowing(&self) -> bool {
        self != &Self {
            sort: self.sort,
            collection: self.collection,
            watch_later: self.watch_later,
            source: self.source,
            directory: self.directory.clone(),
            ..Self::default()
        }
    }
}

/// An empty field disables the threshold. A bare number is expressed in MB.
pub fn minimum_size(input: &str) -> Option<Option<i64>> {
    let input = input.trim();
    if input.is_empty() {
        return Some(None);
    }
    let split = input
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(input.len());
    let (amount, unit) = input.split_at(split);
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1,
        "kb" => 1_000,
        "" | "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" => 1024,
        "mib" => 1024 * 1024,
        "gib" => 1024 * 1024 * 1024,
        _ => return None,
    };
    let amount: String = amount
        .trim()
        .chars()
        .map(|character| match character {
            '۰'..='۹' => (b'0' + (character as u32 - '۰' as u32) as u8) as char,
            '٠'..='٩' => (b'0' + (character as u32 - '٠' as u32) as u8) as char,
            '٫' => '.',
            other => other,
        })
        .collect();
    let (whole, fraction) = amount.split_once('.').unwrap_or((&amount, ""));
    if amount.is_empty() || (whole.is_empty() && fraction.is_empty()) || fraction.len() > 9 {
        return None;
    }
    if !whole
        .bytes()
        .chain(fraction.bytes())
        .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction_units: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    let bytes = whole
        .checked_mul(multiplier)?
        .checked_add((fraction_units * multiplier).div_ceil(10_u128.pow(fraction.len() as u32)))?;
    Some(Some(bytes.try_into().ok()?))
}

/// Local calendar dates, inclusive at both ends. Databases store UTC timestamps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateRange {
    pub from: Option<chrono::NaiveDate>,
    pub through: Option<chrono::NaiveDate>,
}

impl DateRange {
    pub fn bounds(self, nanos: bool) -> (Option<i64>, Option<i64>) {
        let bound = |date: chrono::NaiveDate| {
            use chrono::TimeZone;
            // A few time zones have moved their clocks at midnight.
            let midnight = date.and_hms_opt(0, 0, 0)?;
            let instant = (0..=24 * 60).find_map(|minute| {
                let time = midnight.checked_add_signed(chrono::Duration::minutes(minute))?;
                chrono::Local.from_local_datetime(&time).earliest()
            })?;
            Some(if nanos {
                instant.timestamp_nanos_opt().unwrap_or_else(|| {
                    if instant.timestamp() < 0 {
                        i64::MIN
                    } else {
                        i64::MAX
                    }
                })
            } else {
                instant.timestamp_millis()
            })
        };
        (
            self.from.and_then(bound),
            self.through
                .and_then(|date| date.succ_opt())
                .and_then(bound),
        )
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn video_ext(path: &Path) -> Option<String> {
    let raw = path.extension()?.as_encoded_bytes();
    let mut lower = [0; 5];
    let lower = lower.get_mut(..raw.len())?;
    for (byte, input) in lower.iter_mut().zip(raw) {
        *byte = input.to_ascii_lowercase();
    }
    let ext = std::str::from_utf8(lower).ok()?;
    matches!(
        ext,
        "mp4"
            | "mkv"
            | "mov"
            | "webm"
            | "avi"
            | "m4v"
            | "wmv"
            | "mpeg"
            | "mpg"
            | "mts"
            | "m2ts"
            | "ts"
            | "flv"
            | "3gp"
            | "3g2"
            | "ogv"
            | "vob"
            | "asf"
            | "mxf"
            | "rm"
            | "rmvb"
            | "divx"
            | "f4v"
            | "mpv"
            | "m2v"
            | "mpe"
            | "dv"
            | "nut"
            | "mod"
            | "tod"
            | "mjpeg"
            | "mjpg"
            | "hevc"
            | "h264"
    )
    .then(|| ext.to_owned())
}

pub fn path_key(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        path.as_os_str().as_encoded_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

pub fn from_key(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let (pairs, _) = bytes.as_chunks::<2>();
        let wide: Vec<_> = pairs.iter().copied().map(u16::from_le_bytes).collect();
        PathBuf::from(std::ffi::OsString::from_wide(&wide))
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LibraryView {
    pub list: bool,
    pub size: usize,
}

impl Default for LibraryView {
    fn default() -> Self {
        Self {
            list: false,
            size: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub const fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "Auto",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ControlBarLayout {
    #[default]
    Floating,
    Compact,
    Docked,
    Top,
    Classic,
}

impl ControlBarLayout {
    pub const ALL: [Self; 5] = [
        Self::Floating,
        Self::Compact,
        Self::Docked,
        Self::Top,
        Self::Classic,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Floating => "Floating",
            Self::Compact => "Compact",
            Self::Docked => "Docked",
            Self::Top => "Top",
            Self::Classic => "Inline Bottom",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgressBarPosition {
    #[default]
    FollowLayout,
    Top,
    Bottom,
}

impl ProgressBarPosition {
    pub const ALL: [Self; 3] = [Self::FollowLayout, Self::Top, Self::Bottom];

    pub const fn label(self) -> &'static str {
        match self {
            Self::FollowLayout => "Follow layout",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        }
    }
}

pub enum CollectionTarget {
    Existing(i64),
    New(String),
}
