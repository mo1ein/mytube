use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, params};
use tempfile::NamedTempFile;

use crate::{
    catalog,
    model::{Stamp, Video, now, path_key},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeleteMode {
    Trash,
    Permanent,
}

pub enum Operation {
    Delete(DeleteMode),
    Transfer { destination: PathBuf, moving: bool },
}

impl Operation {
    pub fn progress_label(&self) -> &'static str {
        match self {
            Self::Delete(_) => "Deleting",
            Self::Transfer { moving: true, .. } => "Moving",
            Self::Transfer { moving: false, .. } => "Copying",
        }
    }
}

pub fn check_file(video: &Video) -> Result<()> {
    let stamp = Stamp::read(&video.path)
        .with_context(|| format!("Could not open {}", video.path.display()))?;
    ensure!(
        stamp == video.stamp,
        "This video changed on disk. Refresh your sources and try again."
    );
    Ok(())
}

fn check_mutation(video: &Video) -> Result<()> {
    check_file(video)?;
    ensure!(
        dunce::canonicalize(&video.path)? == video.path,
        "The path to this video changed. Refresh your sources and try again."
    );
    Ok(())
}

fn check_record(db: &Connection, selected: &Video) -> Result<()> {
    ensure!(
        catalog::get_visible(db, selected.id)?
            .is_some_and(|record| { record.rev == selected.rev && record.path == selected.path }),
        "The library entry changed. Select the video again."
    );
    Ok(())
}

pub fn delete(
    db: &Connection,
    writes: &Mutex<()>,
    selected: &Video,
    mode: DeleteMode,
) -> Result<()> {
    let _guard = writes.lock().unwrap();
    check_record(db, selected)?;
    check_mutation(selected)?;
    match mode {
        DeleteMode::Trash => {
            trash::delete(&selected.path).context("System trash could not accept this video")?
        }
        DeleteMode::Permanent => {
            fs::remove_file(&selected.path).context("Could not permanently delete this video")?
        }
    }
    db.execute("DELETE FROM videos WHERE id=?", [selected.id])?;
    Ok(())
}

pub fn transfer(
    db: &mut Connection,
    writes: &Mutex<()>,
    selected: &Video,
    folder: &Path,
    moving: bool,
) -> Result<PathBuf> {
    check_record(db, selected)?;
    check_mutation(selected)?;
    let folder = dunce::canonicalize(folder).context("Could not open the destination folder")?;
    ensure!(folder.is_dir(), "Choose a destination folder");
    let destination = folder.join(selected.path.file_name().context("Video has no filename")?);
    ensure!(
        !destination.try_exists()?,
        "A file already exists at {}",
        destination.display()
    );

    let mut original = fs::File::open(&selected.path)?;
    let metadata = original.metadata()?;
    ensure!(
        Stamp::from_metadata(&metadata) == selected.stamp,
        "This video changed before the transfer started"
    );
    let mut staged = NamedTempFile::with_prefix_in(".mytube-", &folder)?;
    let written = io::copy(&mut original, &mut staged).context("Could not copy the video data")?;
    ensure!(
        written == selected.stamp.bytes as u64,
        "The video size changed during transfer"
    );
    if let Ok(time) = metadata.modified() {
        staged.as_file().set_modified(time)?;
    }
    #[cfg(unix)]
    staged.as_file().set_permissions(metadata.permissions())?;
    staged
        .as_file()
        .sync_all()
        .context("Could not write the destination file")?;
    drop(original);

    let _guard = writes.lock().unwrap();
    check_record(db, selected)?;
    check_mutation(selected)?;
    ensure!(
        dunce::canonicalize(&folder)? == folder,
        "The destination folder changed during transfer"
    );
    let destination_file = staged
        .persist_noclobber(&destination)
        .context("Could not save a new file at the destination")?;
    let metadata = destination_file.metadata()?;
    let stamp = Stamp::from_metadata(&metadata);
    let created = crate::model::creation_time(&metadata);
    drop(destination_file);

    if moving {
        let tx = db.transaction()?;
        let (title, filename, folder, search) = catalog::path_fields(&destination);
        // A previous offline record at this path may carry collections of its own.
        tx.execute("INSERT OR IGNORE INTO collection_videos(collection,video)             SELECT c.collection,? FROM collection_videos c JOIN videos v ON v.id=c.video WHERE v.path=?",
            params![selected.id, path_key(&destination)])?;
        tx.execute("DELETE FROM videos WHERE path=?", [path_key(&destination)])?;
        tx.execute("UPDATE videos SET path=?,title=?,filename=?,folder=?,search=?,modified=?,created=?,pinned=1,rev=rev+1 WHERE id=?",
            params![path_key(&destination), title, filename, folder, search, stamp.modified, created, selected.id])?;
        tx.execute("DELETE FROM sightings WHERE video=?", [selected.id])?;
        tx.execute(
            "UPDATE thumbs SET revision=revision+1 WHERE video=?",
            [selected.id],
        )?;
        match fs::remove_file(&selected.path) {
            Ok(()) => tx.commit()?,
            Err(error) => {
                tx.rollback()?;
                index_copy(db, &destination, selected, stamp, created)?;
                bail!(
                    "Copied the video to {}. The original remains in place: {error}",
                    destination.display()
                );
            }
        }
    } else {
        index_copy(db, &destination, selected, stamp, created)?;
    }
    Ok(destination)
}

fn index_copy(
    db: &mut Connection,
    path: &Path,
    source: &Video,
    stamp: Stamp,
    created: Option<i64>,
) -> Result<()> {
    let tx = db.transaction()?;
    let (id, _) = catalog::upsert(&tx, path, &source.ext, stamp, created, now())?;
    tx.execute("UPDATE videos SET pinned=1 WHERE id=?", [id])?;
    if let Some(copy) = catalog::get(&tx, id)? {
        catalog::set_media(&tx, &copy, &source.media)?;
    }
    tx.commit()?;
    Ok(())
}

pub fn reveal(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "This video has no containing folder"
    );
    #[cfg(target_os = "linux")]
    if reveal_on_linux(path).is_ok() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-R").arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        use std::{
            ffi::OsString,
            os::windows::{ffi::OsStrExt, process::CommandExt},
        };

        let path = dunce::simplified(path).as_os_str();
        ensure!(
            !path.encode_wide().any(|unit| unit == u16::from(b'"')),
            "This video has an invalid path"
        );
        // Explorer parses the comma and quotes as part of its command syntax.
        let mut arg = OsString::from("/select,\"");
        arg.push(path);
        arg.push("\"");
        let mut command = Command::new("explorer.exe");
        command.raw_arg(arg);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        let mut command = Command::new("xdg-open");
        command.arg(
            path.parent()
                .context("This video has no containing folder")?,
        );
        command
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not open the system file manager")?;
    // The file manager can stay open while additional file jobs run.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn reveal_on_linux(path: &Path) -> zbus::Result<()> {
    use std::time::Duration;
    use zbus::blocking::{Proxy, connection::Builder};

    const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
    let connection = Builder::session()?
        .method_timeout(Duration::from_secs(2))
        .build()?;
    let manager = Proxy::new(
        &connection,
        FILE_MANAGER,
        "/org/freedesktop/FileManager1",
        FILE_MANAGER,
    )?;
    manager.call("ShowItems", &(vec![file_uri(path)], ""))
}

#[cfg(target_os = "linux")]
fn file_uri(path: &Path) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_encode};
    use std::os::unix::ffi::OsStrExt;

    const PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'/')
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    let path = percent_encode(path.as_os_str().as_bytes(), PATH_ENCODE_SET);
    format!("file://{path}")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    #[test]
    fn file_manager_uri_preserves_the_exact_filename() {
        let path = Path::new(OsStr::from_bytes(b"/videos/Clip #1%?\xc3\xa9\xff.mp4"));
        assert_eq!(
            file_uri(path),
            "file:///videos/Clip%20%231%25%3F%C3%A9%FF.mp4"
        );
    }
}
