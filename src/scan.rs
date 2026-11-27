use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use rusqlite::Connection;
use walkdir::WalkDir;

use crate::{
    catalog,
    model::{Source, Stamp, now, path_key, video_ext},
};

pub const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub running: bool,
    pub paused: bool,
    pub current: PathBuf,
    pub source: usize,
    pub sources: usize,
    pub checked: u64,
    pub found: u64,
    pub skipped: u64,
    pub skipped_paths: Arc<Vec<(PathBuf, String)>>,
}

impl Progress {
    fn record_skip(&mut self, path: &Path, reason: impl std::fmt::Display) {
        self.skipped += 1;
        if self.skipped_paths.len() < 256 {
            Arc::make_mut(&mut self.skipped_paths).push((path.to_owned(), reason.to_string()));
        }
    }
}

#[derive(Default)]
pub struct Control {
    state: Mutex<State>,
    signal: Condvar,
}

#[derive(Default)]
struct State {
    generation: u64,
    progress: Progress,
}

impl Control {
    pub fn restart(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.progress.paused = false;
        self.signal.notify_all();
    }

    pub fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }

    pub fn progress(&self) -> Progress {
        self.state.lock().unwrap().progress.clone()
    }

    pub fn set_paused(&self, paused: bool) {
        self.state.lock().unwrap().progress.paused = paused;
        self.signal.notify_all();
    }

    fn active(&self, generation: u64) -> bool {
        let state = self
            .signal
            .wait_while(self.state.lock().unwrap(), |state| {
                state.generation == generation && state.progress.paused
            })
            .unwrap();
        state.generation == generation
    }

    pub fn publish(&self, progress: &Progress) {
        let mut state = self.state.lock().unwrap();
        let paused = state.progress.paused && progress.running;
        state.progress = progress.clone();
        state.progress.paused = paused;
    }
}

pub fn refresh(
    db: &mut Connection,
    control: &Control,
    writes: &Mutex<()>,
    generation: u64,
    force: bool,
    notify: impl Fn(bool),
) -> Result<()> {
    let roots = catalog::scan_sources(db, (!force).then(|| now() - CACHE_TTL.as_millis() as i64))?;
    if roots.is_empty() && !force {
        return Ok(());
    }
    let exclusions = catalog::sources(db, "exclusions")?;
    let scan_id = catalog::next_generation(db)?;
    let mut progress = Progress {
        running: !roots.is_empty(),
        sources: roots.len(),
        ..Progress::default()
    };
    control.publish(&progress);
    notify(false);

    let result = (|| {
        for (index, root) in roots.iter().enumerate() {
            if !control.active(generation) {
                break;
            }
            progress.source = index + 1;
            progress.current.clone_from(&root.path);
            control.publish(&progress);
            notify(false);
            walk_source(
                db,
                &Walk {
                    root,
                    exclusions: &exclusions,
                    scan_id,
                    generation,
                    control,
                    writes,
                    notify: &notify,
                },
                &mut progress,
            )?;
        }
        if control.generation() == generation {
            refresh_managed(db, writes)?;
        }
        Ok(())
    })();
    progress.running = false;
    progress.current.clear();
    control.publish(&progress);
    notify(true);
    result
}

struct Walk<'a> {
    root: &'a Source,
    exclusions: &'a [Source],
    scan_id: i64,
    generation: u64,
    control: &'a Control,
    writes: &'a Mutex<()>,
    notify: &'a dyn Fn(bool),
}

fn walk_source(db: &mut Connection, walk: &Walk<'_>, progress: &mut Progress) -> Result<()> {
    let root = &walk.root.path;
    let excluded = |path: &Path| {
        walk.exclusions
            .iter()
            .any(|dir| path.starts_with(&dir.path))
            || system_directory(root, path)
            || (walk.root.automatic && path != root && scan_cache(path))
    };
    if !excluded(root) {
        match fs::metadata(root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                progress.record_skip(root, "Source is no longer a folder");
                return Ok(());
            }
            Err(error) => {
                progress.record_skip(root, error);
                return Ok(());
            }
        }
    }

    let mut protected = Vec::new();
    let mut batch = Vec::with_capacity(128);
    let mut last = Instant::now();
    let mut dirty = false;
    let priority = if walk.root.automatic {
        preferred_directories(root, excluded)
    } else {
        Vec::new()
    };
    for start in priority.iter().chain(std::iter::once(root)) {
        let mut entries = WalkDir::new(start)
            .follow_links(false)
            .follow_root_links(false)
            .same_file_system(!cfg!(target_os = "macos"))
            .max_open(24)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || (!excluded(entry.path())
                        && (start != root || !priority.iter().any(|path| path == entry.path())))
            });

        while walk.control.active(walk.generation) {
            let Some(entry) = entries.next() else {
                break;
            };
            match entry {
                Ok(entry) if entry.file_type().is_file() => {
                    progress.checked += 1;
                    if let Some(ext) = video_ext(entry.path()) {
                        batch.push((entry.into_path(), ext));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    let path = error.path().unwrap_or(root);
                    if let Some(cause) = error.io_error() {
                        progress.record_skip(path, cause);
                    } else {
                        progress.record_skip(path, &error);
                    }
                    protected.push(path.to_path_buf());
                }
            }
            let report = last.elapsed() >= Duration::from_millis(170);
            if batch.len() == batch.capacity() || report {
                dirty |= flush(db, walk, &mut batch, &mut protected, progress)?;
            }
            if report {
                walk.control.publish(progress);
                (walk.notify)(dirty);
                dirty = false;
                last = Instant::now();
            }
        }
        if start != root && walk.control.generation() == walk.generation {
            dirty |= flush(db, walk, &mut batch, &mut protected, progress)?;
            walk.control.publish(progress);
            (walk.notify)(dirty);
            dirty = false;
            last = Instant::now();
        }
    }
    if walk.control.generation() == walk.generation {
        flush(db, walk, &mut batch, &mut protected, progress)?;
        let _guard = walk.writes.lock().unwrap();
        if walk.control.generation() == walk.generation {
            catalog::reconcile(db, walk.root.id, walk.scan_id, &protected)?;
        }
    }
    walk.control.publish(progress);
    (walk.notify)(true);
    Ok(())
}

fn preferred_directories(root: &Path, excluded: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Some(user) = directories::UserDirs::new() else {
        return Vec::new();
    };
    let mut preferred = Vec::new();
    for path in [
        user.video_dir(),
        user.download_dir(),
        user.desktop_dir(),
        user.picture_dir(),
    ]
    .into_iter()
    .flatten()
    {
        let Some(path) = available_directory(path.to_path_buf()) else {
            continue;
        };
        if path == root || !path.starts_with(root) || excluded(&path) || !same_device(root, &path) {
            continue;
        }
        if preferred.iter().any(|dir: &PathBuf| path.starts_with(dir)) {
            continue;
        }
        preferred.retain(|dir| !dir.starts_with(&path));
        preferred.push(path);
    }
    preferred
}

fn same_device(_root: &Path, _path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(_root)
            .ok()
            .zip(fs::metadata(_path).ok())
            .is_some_and(|(root, path)| root.dev() == path.dev())
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn flush(
    db: &mut Connection,
    walk: &Walk<'_>,
    batch: &mut Vec<(PathBuf, String)>,
    protected: &mut Vec<PathBuf>,
    progress: &mut Progress,
) -> Result<bool> {
    if batch.is_empty() {
        return Ok(false);
    }
    let _guard = walk.writes.lock().unwrap();
    if walk.control.generation() != walk.generation {
        batch.clear();
        return Ok(false);
    }
    let tx = db.transaction()?;
    let added = now();
    let mut updated = false;
    for (path, ext) in batch.drain(..) {
        match Stamp::inspect(&path) {
            Ok((stamp, created)) => {
                if matches!(ext.as_str(), "ts" | "mts" | "m2ts" | "mod")
                    && catalog::known_file(&tx, &path_key(&path))?.is_none()
                {
                    match candidate_content(&path, &ext) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(error) => {
                            progress.record_skip(&path, error);
                            protected.push(path);
                            continue;
                        }
                    }
                }
                let (id, changed) = catalog::upsert(&tx, &path, &ext, stamp, created, added)?;
                catalog::sight(&tx, walk.root.id, id, walk.scan_id)?;
                progress.found += 1;
                updated |= changed;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                progress.record_skip(&path, error);
                protected.push(path);
            }
        }
    }
    tx.commit()?;
    Ok(updated)
}

fn transport_stream(path: &Path) -> io::Result<bool> {
    // TypeScript commonly shares these extensions with MPEG transport streams.
    let mut bytes = Vec::with_capacity(614);
    fs::File::open(path)?.take(614).read_to_end(&mut bytes)?;
    Ok([188, 192, 204].iter().any(|stride| {
        (0..*stride)
            .any(|start| (0..3).all(|packet| bytes.get(start + packet * stride) == Some(&0x47)))
    }))
}

pub fn candidate_content(path: &Path, ext: &str) -> io::Result<bool> {
    match ext {
        "ts" | "mts" | "m2ts" => transport_stream(path),
        "mod" => {
            // Camera MOD files use an MPEG program stream. Go module files are text.
            let mut head = Vec::with_capacity(4096);
            fs::File::open(path)?.take(4096).read_to_end(&mut head)?;
            Ok(head.windows(4).any(|bytes| bytes == [0, 0, 1, 0xba]))
        }
        _ => Ok(true),
    }
}

fn refresh_managed(db: &Connection, writes: &Mutex<()>) -> Result<()> {
    let mut query = db.prepare("SELECT id,path,ext FROM videos WHERE pinned=1")?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                crate::model::from_key(row.get_ref(1)?.as_blob()?),
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, path, ext) in rows {
        let _guard = writes.lock().unwrap();
        if catalog::get(db, id)?.is_none_or(|video| video.path != path) {
            continue;
        }
        match Stamp::inspect(&path) {
            Ok((stamp, created)) => {
                catalog::upsert(db, &path, &ext, stamp, created, now())?;
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && path.parent().is_some_and(Path::is_dir) =>
            {
                db.execute("UPDATE videos SET pinned=0 WHERE id=?", [id])?;
            }
            // An unavailable containing folder may be on an offline drive.
            Err(_) => continue,
        };
    }
    let _guard = writes.lock().unwrap();
    catalog::prune_orphans(db)?;
    Ok(())
}

fn system_directory(root: &Path, path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    if root == Path::new("/")
        && ["/dev", "/proc", "/sys", "/run"]
            .iter()
            .any(|dir| path == Path::new(dir))
    {
        return true;
    }
    #[cfg(target_os = "macos")]
    if root == Path::new("/")
        && ["/Volumes", "/System/Volumes", "/dev"]
            .iter()
            .any(|dir| path == Path::new(dir))
    {
        return true;
    }
    if let Some(name) = path.file_name() {
        let name = name.to_string_lossy();
        return path != root
            && (name == ".Trash"
                || name == ".Trashes"
                || name == "$RECYCLE.BIN"
                || name.starts_with(".Trash-")
                || path.ends_with("Trash/files"));
    }
    false
}

fn network_fs(fs: &std::ffi::OsStr) -> bool {
    matches!(
        fs.to_str(),
        Some(
            "nfs"
                | "nfs4"
                | "smbfs"
                | "cifs"
                | "smb3"
                | "afpfs"
                | "webdav"
                | "fuse.sshfs"
                | "fuse.rclone"
        )
    )
}

pub fn remote_file(path: &Path) -> bool {
    #[cfg(windows)]
    if matches!(path.components().next(), Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::UNC(_, _) | std::path::Prefix::VerbatimUNC(_, _)))
    {
        return true;
    }
    sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .is_some_and(|disk| network_fs(disk.file_system()))
}

pub fn mounted_drives() -> Vec<PathBuf> {
    let mut roots: Vec<_> = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|disk| {
            let fs = disk.file_system();
            !network_fs(fs)
                && !matches!(
                    fs.to_str(),
                    Some(
                        "autofs"
                            | "devfs"
                            | "proc"
                            | "sysfs"
                            | "tmpfs"
                            | "fuse.portal"
                            | "squashfs"
                    )
                )
                && !disk.name().as_encoded_bytes().starts_with(b"/dev/loop")
        })
        .map(|disk| disk.mount_point().to_path_buf())
        .filter(|path| !system_mount(path))
        .filter_map(available_directory)
        .collect();
    #[cfg(unix)]
    roots.push(PathBuf::from("/"));
    roots.sort();
    roots.dedup();
    roots
}

pub fn discover_drives() -> Vec<PathBuf> {
    if let Some(paths) = crate::setting("SCAN_ROOTS") {
        return std::env::split_paths(&paths)
            .filter_map(available_directory)
            .collect();
    }
    mounted_drives()
}

fn available_directory(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let path = dunce::canonicalize(path).ok()?;
    path.is_dir().then_some(path)
}

fn system_mount(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    if ["/proc", "/sys", "/run", "/dev", "/boot", "/snap"]
        .iter()
        .any(|dir| path.starts_with(dir))
    {
        return true;
    }
    #[cfg(target_os = "macos")]
    if path.starts_with("/System/Volumes") {
        return true;
    }
    scan_cache(path)
}

fn scan_cache(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if matches!(name, ".git" | ".cache") {
        return true;
    }
    #[cfg(target_os = "linux")]
    if [
        "/var/cache",
        "/var/lib/docker",
        "/var/lib/containers",
        "/var/lib/snapd",
        "/var/lib/flatpak",
        "/snap",
    ]
    .iter()
    .any(|dir| path.starts_with(dir))
    {
        return true;
    }
    #[cfg(target_os = "macos")]
    if path.ends_with("Library/Caches") || path.starts_with("/private/var/folders") {
        return true;
    }
    #[cfg(windows)]
    if path.ends_with("AppData/Local/Temp")
        || path.ends_with("Windows/WinSxS")
        || name.eq_ignore_ascii_case("System Volume Information")
    {
        return true;
    }
    false
}
