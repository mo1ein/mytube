use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use eframe::egui::ColorImage;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use wait_timeout::ChildExt;

use crate::{
    files,
    model::{Media, Stamp, Video, now},
};

#[derive(Default)]
pub struct Tools {
    pub ffprobe: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

impl Tools {
    pub fn find() -> Self {
        Self {
            ffprobe: locate("ffprobe"),
            ffmpeg: locate("ffmpeg"),
        }
    }

    pub fn missing(&self) -> Vec<&'static str> {
        [
            (self.ffprobe.is_none(), "ffprobe"),
            (self.ffmpeg.is_none(), "ffmpeg"),
        ]
        .into_iter()
        .filter_map(|(missing, name)| missing.then_some(name))
        .collect()
    }
}

fn locate(name: &str) -> Option<PathBuf> {
    let key = name.to_uppercase();
    if let Some(path) = crate::setting(&key).map(PathBuf::from) {
        return path.is_absolute().then_some(path).filter(|p| p.is_file());
    }
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let path = env::var_os("PATH").unwrap_or_default();
    let sibling = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    #[cfg(target_os = "macos")]
    let brew_bin = sibling
        .as_deref()
        .and_then(Path::parent)
        .and_then(|contents| std::fs::read_to_string(contents.join("Resources/media-bin")).ok())
        .map(|path| PathBuf::from(path.trim()));
    let folders = sibling.into_iter().chain(env::split_paths(&path));
    #[cfg(target_os = "macos")]
    let folders = folders
        .chain(brew_bin)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map(PathBuf::from));
    #[cfg(windows)]
    let folders = folders.chain(
        [
            ("LOCALAPPDATA", r"Microsoft\WinGet\Links"),
            ("ProgramFiles", r"WinGet\Links"),
            ("ProgramFiles", "MPV Player"),
        ]
        .into_iter()
        .filter_map(|(variable, folder)| {
            env::var_os(variable).map(|path| PathBuf::from(path).join(folder))
        }),
    );
    folders
        .filter(|p| p.is_absolute())
        .map(|dir| dir.join(&exe))
        .find(|p| p.is_file())
}

fn quiet_command(executable: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn capture_until(
    mut command: Command,
    limit: u64,
    timeout: Duration,
    active: Option<&dyn Fn() -> bool>,
) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child
        .stdout
        .take()
        .context("Media output pipe unavailable")?;
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut bytes = Vec::new();
            output.take(limit + 1).read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = if active.is_some() {
                remaining.min(Duration::from_millis(16))
            } else {
                remaining
            };
            if !remaining.is_zero() && active.is_none_or(|check| check()) {
                match child.wait_timeout(wait) {
                    Ok(Some(status)) => break status,
                    Ok(None) => continue,
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error.into());
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("Media processing ended before a frame was available");
        };
        let bytes = reader
            .join()
            .map_err(|_| anyhow::anyhow!("Media output reader stopped"))??;
        ensure!(
            status.success() && bytes.len() as u64 <= limit,
            "Media tool could not read the video"
        );
        Ok(bytes)
    })
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    attached_pic: Option<i32>,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<i64>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
    format_name: String,
    #[serde(default)]
    tags: FormatTags,
}

#[derive(Default, Deserialize)]
struct FormatTags {
    major_brand: Option<String>,
}

pub fn probe(tool: &Path, path: &Path, active: Option<&dyn Fn() -> bool>) -> Result<Media> {
    const FORMATS: &str = "mov,matroska,webm,avi,asf,mpeg,mpegts,mpegvideo,flv,ogg,mxf,rm,dv,nut,mjpeg,h264,hevc,h263,vc1,av1";
    let mut command = quiet_command(tool);
    command.args(["-v", "error", "-protocol_whitelist", "file", "-format_whitelist", FORMATS, "-print_format", "json", "-show_entries",
        "format=duration,format_name:format_tags=major_brand:stream=codec_type,codec_name,width,height,avg_frame_rate,duration:stream_disposition=attached_pic:stream_side_data=rotation",
        "-i"]).arg(path);
    let raw = capture_until(command, 512 * 1024, Duration::from_secs(10), active)?;
    let result: Probe = serde_json::from_slice(&raw)?;
    let format = result.format.as_ref().context("Missing media format")?;
    ensure!(
        !format.format_name.split(',').any(|name| {
            name.starts_with("image2")
                || name.ends_with("_pipe")
                || matches!(name, "gif" | "apng" | "ico" | "avif" | "svg" | "jxl")
        }) && !format.tags.major_brand.as_deref().is_some_and(|brand| {
            matches!(
                brand.trim(),
                "avif" | "heic" | "heix" | "hevc" | "hevx" | "mif1" | "msf1"
            )
        }),
        "Image formats are unsupported"
    );
    let track = result
        .streams
        .iter()
        .find(|stream| {
            stream.codec_type.as_deref() == Some("video")
                && stream
                    .disposition
                    .as_ref()
                    .and_then(|item| item.attached_pic)
                    != Some(1)
        })
        .context("No video stream")?;
    let duration =
        positive(track.duration.as_deref()).or_else(|| positive(format.duration.as_deref()));
    let fps = track.avg_frame_rate.as_deref().and_then(|value| {
        let (frames, seconds) = value.split_once('/')?;
        let rate = frames.parse::<f64>().ok()? / seconds.parse::<f64>().ok()?;
        (rate.is_finite() && rate > 0.0).then_some(rate)
    });
    let (mut width, mut height) = (
        track.width.filter(|&w| w > 0),
        track.height.filter(|&h| h > 0),
    );
    if track
        .side_data_list
        .iter()
        .any(|item| item.rotation.is_some_and(|r| r.rem_euclid(180) == 90))
    {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(Media {
        duration,
        width,
        height,
        fps,
        codec: track.codec_name.clone(),
        audio: result
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("audio"))
            .and_then(|stream| stream.codec_name.clone()),
    })
}

fn positive(value: Option<&str>) -> Option<f64> {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn frame_command(tool: &Path, path: &Path, seek: f64, filter: &str, exact: bool) -> Command {
    frame_sequence(tool, path, seek, filter, exact, 1)
}

fn frame_sequence(
    tool: &Path,
    path: &Path,
    seek: f64,
    filter: &str,
    exact: bool,
    count: usize,
) -> Command {
    let mut command = quiet_command(tool);
    if count > 1 {
        command.args(["-copyts", "-start_at_zero"]);
    }
    if !exact {
        command.args(["-noaccurate_seek", "-skip_frame", "nokey"]);
    }
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file",
            "-ss",
        ])
        .arg(format!("{seek:.3}"))
        .args(["-threads", "1", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:V:0",
            "-an",
            "-sn",
            "-dn",
            "-filter_threads",
            "1",
            "-vf",
            filter,
            "-frames:v",
        ])
        .arg(count.to_string());
    if count > 1 {
        command.args(["-fps_mode", "passthrough"]);
    }
    command.args([
        "-threads",
        "1",
        "-c:v",
        "mjpeg",
        "-q:v",
        "4",
        "-f",
        if count == 1 { "image2pipe" } else { "mpjpeg" },
        "pipe:1",
    ]);
    command
}

fn generate(tool: &Path, path: &Path, seek: f64, active: &dyn Fn() -> bool) -> Result<Vec<u8>> {
    let command = frame_command(
        tool,
        path,
        seek,
        "scale=352:198:force_original_aspect_ratio=decrease:force_divisible_by=2",
        true,
    );
    let bytes = capture_until(command, 160 * 1024, Duration::from_secs(10), Some(active))?;
    ensure!(!bytes.is_empty(), "No frame at this timestamp");
    Ok(bytes)
}

pub const PREVIEW_COLUMNS: usize = 4;
pub const PREVIEW_ROWS: usize = 2;
pub const PREVIEW_FRAME: Duration = Duration::from_millis(250);
const PREVIEW_WIDTH: u32 = 160;
const PREVIEW_HEIGHT: u32 = 90;

pub fn preview_count(video: &Video) -> usize {
    match video.media.duration.unwrap_or_default() {
        duration if duration >= 18.0 => 4,
        duration if duration >= 10.0 => 3,
        duration if duration >= 5.0 => 2,
        _ => 1,
    }
}

pub fn preview_segment(
    db: &Connection,
    tool: Option<&Path>,
    video: &Video,
    segment: usize,
    active: &dyn Fn() -> bool,
) -> Result<Option<ColorImage>> {
    let width = PREVIEW_WIDTH * PREVIEW_COLUMNS as u32;
    let height = PREVIEW_HEIGHT * PREVIEW_ROWS as u32;
    let slot = segment as i64;
    let cached: Option<Vec<u8>> = db
        .prepare_cached(
            "SELECT jpeg FROM preview_segments WHERE video=? AND revision=? AND segment=?",
        )?
        .query_row(params![video.id, video.rev, slot], |row| row.get(0))
        .optional()?;
    if let Some(image) = cached.and_then(|bytes| decode_with_limit(&bytes, width, height).ok()) {
        return Ok(Some(image));
    }
    let Some(tool) = tool else { return Ok(None) };
    files::check_file(video)?;
    let duration = video.media.duration.unwrap_or_default();
    let fraction = segment as f64 / preview_count(video).saturating_sub(1).max(1) as f64;
    let seek = (duration - 2.1).max(0.0) * fraction.min(0.96);
    let filter = format!(
        "fps=4,scale={PREVIEW_WIDTH}:{PREVIEW_HEIGHT}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={PREVIEW_WIDTH}:{PREVIEW_HEIGHT}:(ow-iw)/2:(oh-ih)/2,tpad=stop_mode=clone:stop_duration=2,tile={PREVIEW_COLUMNS}x{PREVIEW_ROWS}"
    );
    let command = frame_command(tool, &video.path, seek, &filter, true);
    let bytes = capture_until(command, 240 * 1024, Duration::from_secs(5), Some(active))?;
    let image = decode_with_limit(&bytes, width, height)?;
    ensure!(
        image.size == [width as usize, height as usize],
        "Incomplete preview frames"
    );
    files::check_file(video)?;
    db.execute(
        "INSERT INTO preview_segments(video,revision,segment,cached_at,jpeg)
         SELECT id,rev,?,?,? FROM videos WHERE id=? AND rev=?
         ON CONFLICT(video,segment) DO UPDATE SET revision=excluded.revision,
             cached_at=excluded.cached_at,jpeg=excluded.jpeg",
        params![slot, now(), bytes, video.id, video.rev],
    )?;
    db.execute(
        "DELETE FROM preview_segments WHERE (video,segment) IN (
            SELECT video,segment FROM preview_segments
            ORDER BY cached_at DESC,video,segment LIMIT -1 OFFSET 480
         )",
        [],
    )?;
    Ok(Some(image))
}

pub fn timeline_preview(
    tool: &Path,
    video: &Video,
    at: f64,
    exact: bool,
    active: &dyn Fn() -> bool,
) -> Result<(Vec<u8>, ColorImage)> {
    files::check_file(video)?;
    let extract = |position: f64| -> Result<(Vec<u8>, ColorImage)> {
        let command = frame_command(
            tool,
            &video.path,
            position.max(0.0),
            "scale=192:144:force_original_aspect_ratio=decrease:force_divisible_by=2",
            exact,
        );
        let bytes = capture_until(command, 100 * 1024, Duration::from_secs(4), Some(active))?;
        let frame = decode_with_limit(&bytes, 192, 144)?;
        Ok((bytes, frame))
    };
    let (bytes, frame) = extract(at).or_else(|error| {
        let frame_interval = video
            .media
            .fps
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .unwrap_or(30.0)
            .recip()
            .clamp(0.001, 0.5);
        // Seeking after the last frame's presentation time can return no image
        // even though the player still displays it until the end of the clip.
        if exact
            && active()
            && at > frame_interval
            && video
                .media
                .duration
                .is_some_and(|end| (end - at).abs() <= frame_interval)
        {
            return extract(at - frame_interval - 0.001).or(Err(error));
        }
        Err(error)
    })?;
    files::check_file(video)?;
    Ok((bytes, frame))
}

// Decode a short, chronological window in one pass. As with an accurate -ss
// request, select the first presented frame at or after each timestamp.
pub const FILMSTRIP_BATCH: usize = 6;
pub fn filmstrip_frames(
    tool: &Path,
    video: &Video,
    times: &[i64],
    active: &dyn Fn() -> bool,
) -> Result<Vec<(Vec<u8>, ColorImage)>> {
    ensure!(
        (2..=FILMSTRIP_BATCH).contains(&times.len()) && times.windows(2).all(|w| w[0] < w[1]),
        "Invalid filmstrip window"
    );
    files::check_file(video)?;
    let mut select = String::from("isnan(prev_t)");
    for &time in &times[1..] {
        let timestamp = time as f64 / 1000.0;
        select.push_str(&format!("+gte(t,{timestamp:.3})*lt(prev_t,{timestamp:.3})"));
    }
    let stop = *times.last().unwrap() as f64 / 1000.0 + 1.0;
    let filter = format!(
        "trim=end={stop:.3},select='{select}',scale=192:144:force_original_aspect_ratio=decrease:force_divisible_by=2"
    );
    let command = frame_sequence(
        tool,
        &video.path,
        times[0] as f64 / 1000.0,
        &filter,
        true,
        times.len(),
    );
    let bytes = capture_until(
        command,
        times.len() as u64 * 100 * 1024,
        Duration::from_secs(5),
        Some(active),
    )?;
    files::check_file(video)?;

    // FFmpeg's multipart JPEG muxer prefixes each image with its byte count.
    // Reject a partial window (e.g. sparse VFR or EOF) before assigning times.
    let mut data = bytes.as_slice();
    let mut frames = Vec::with_capacity(times.len());
    while let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
        let header = std::str::from_utf8(&data[..end])?;
        let size = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-length: "))
            .context("Missing filmstrip frame size")?
            .parse::<usize>()?;
        data = &data[end + 4..];
        let jpeg = data.get(..size).context("Incomplete filmstrip frame")?;
        frames.push((jpeg.to_vec(), decode_with_limit(jpeg, 192, 144)?));
        data = &data[size..];
    }
    ensure!(frames.len() == times.len(), "Incomplete filmstrip window");
    Ok(frames)
}

pub fn timeline_step(video: &Video, end: f64) -> f64 {
    let duration = video
        .media
        .duration
        .filter(|&seconds| seconds > 0.0)
        .unwrap_or(end);
    ((duration / 30.0).ceil() * 0.25).clamp(0.25, 10.0)
}

pub fn timeline_sample(video: &Video, end: f64, position: f64) -> f64 {
    let step = timeline_step(video, end);
    ((position / step).round() * step).min((end - 0.15).max(0.0))
}

pub fn cached_timeline(
    db: &Connection,
    (video, rev, at): (i64, i64, i64),
) -> Result<Option<ColorImage>> {
    let jpeg: Option<Vec<u8>> = db
        .prepare_cached(
            "SELECT jpeg FROM timeline_frames WHERE video=? AND revision=? AND position_ms=?",
        )?
        .query_row(params![video, rev, at], |row| row.get(0))
        .optional()?;
    jpeg.as_deref()
        .map(|bytes| decode_with_limit(bytes, 192, 144))
        .transpose()
}

pub fn recent_timeline(
    db: &Connection,
    (video, rev): (i64, i64),
) -> Result<Vec<(i64, ColorImage)>> {
    let mut stmt = db.prepare_cached(
        "SELECT position_ms,jpeg FROM timeline_frames WHERE video=? AND revision=?
         ORDER BY cached_at DESC LIMIT 24",
    )?;
    let mut rows = stmt.query(params![video, rev])?;
    let mut frames = Vec::new();
    while let Some(row) = rows.next()? {
        if let Ok(image) = decode_with_limit(row.get_ref(1)?.as_blob()?, 192, 144) {
            frames.push((row.get(0)?, image));
        }
    }
    frames.reverse();
    Ok(frames)
}

pub fn store_timeline(db: &Connection, key: (i64, i64, i64), jpeg: &[u8]) -> Result<()> {
    store_timeline_batch(db, [(key, jpeg)])
}

pub fn store_timeline_batch<'a>(
    db: &Connection,
    frames: impl IntoIterator<Item = ((i64, i64, i64), &'a [u8])>,
) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    {
        let mut insert = tx.prepare_cached("INSERT INTO timeline_frames(video,revision,position_ms,cached_at,jpeg)
            SELECT id,rev,?,?,? FROM videos WHERE id=? AND rev=? AND probe=1
            ON CONFLICT(video,revision,position_ms) DO UPDATE SET jpeg=excluded.jpeg,cached_at=excluded.cached_at")?;
        let cached = now();
        for ((video, rev, at), jpeg) in frames {
            insert.execute(params![at, cached, jpeg, video, rev])?;
        }
    }
    trim_timeline(&tx)?;
    tx.commit()?;
    Ok(())
}

pub fn trim_timeline(db: &Connection) -> Result<()> {
    let (used, allowed): (i64, i64) = db.query_row(
        "SELECT bytes_used,bytes_allowed FROM timeline_budget WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if used > allowed {
        // Evict a batch, so ordinary inserts only need the indexed budget lookup.
        db.execute("DELETE FROM timeline_frames WHERE (video,revision,position_ms) IN (
            SELECT video,revision,position_ms FROM (
                SELECT video,revision,position_ms,
                    sum(length(jpeg)) OVER (ORDER BY cached_at DESC,video,revision,position_ms) AS cumulative
                FROM timeline_frames
            ) WHERE cumulative>?
        )", [allowed - allowed / 20])?;
    }
    Ok(())
}

pub fn cached_thumbnail(db: &Connection, video: &Video) -> Result<Option<Option<ColorImage>>> {
    let cached: Option<Option<Vec<u8>>> = db
        .prepare_cached("SELECT jpeg FROM thumbs WHERE video=? AND revision=?")?
        .query_row(params![video.id, video.rev], |row| row.get(0))
        .optional()?;
    cached
        .map(|data| data.as_deref().map(decode).transpose())
        .transpose()
}

pub fn generate_thumbnail(
    db: &Connection,
    tool: Option<&Path>,
    video: &Video,
    active: &dyn Fn() -> bool,
) -> Result<Option<ColorImage>> {
    let Some(tool) = tool else {
        return Ok(None);
    };
    if Stamp::read(&video.path).ok() != Some(video.stamp) {
        return Ok(None);
    }

    let seek = video
        .media
        .duration
        .filter(|&seconds| seconds > 0.3)
        .map_or(0.0, |seconds| (seconds * 0.12).min(30.0));
    let data = generate(tool, &video.path, seek, active)
        .or_else(|error| {
            if seek > 0.0 && active() {
                generate(tool, &video.path, 0.0, active)
            } else {
                Err(error)
            }
        })
        .ok();
    if !active() || Stamp::read(&video.path).ok() != Some(video.stamp) {
        return Ok(None);
    }
    db.execute("INSERT INTO thumbs(video,revision,cached_at,jpeg)         SELECT id,rev,?,? FROM videos WHERE id=? AND rev=?         ON CONFLICT(video) DO UPDATE SET revision=excluded.revision,cached_at=excluded.cached_at,jpeg=excluded.jpeg",
        params![now(), data, video.id, video.rev])?;
    data.as_deref().map(decode).transpose()
}

pub fn trim_thumbnails(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM thumbs WHERE video IN (SELECT video FROM thumbs ORDER BY cached_at DESC,video DESC LIMIT -1 OFFSET 4096)", [])?;
    Ok(())
}

fn decode(data: &[u8]) -> Result<ColorImage> {
    decode_with_limit(data, 416, 234)
}

pub(crate) fn decode_with_limit(data: &[u8], width: u32, height: u32) -> Result<ColorImage> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(data), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    reader.limits(limits);
    let image = reader.decode()?.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}
