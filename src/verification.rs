use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::{
    catalog, files,
    model::{
        CollectionTarget, ControlBarLayout, Filter, LibraryView, Media, PAGE, ProgressBarPosition,
        Sort, Stamp, now,
    },
    scan::{self, Control},
};

fn database(root: &Path) -> Result<Connection> {
    let db = catalog::open(&root.join("test.sqlite3"))?;
    catalog::initialize(&db)?;
    Ok(db)
}

#[test]
fn control_bar_layout_defaults_and_survives_reopening() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = database(temp.path())?;
    assert_eq!(
        catalog::control_bar_layout(&db)?,
        ControlBarLayout::Floating
    );
    catalog::save_control_bar_layout(&db, ControlBarLayout::Classic)?;
    drop(db);
    let db = database(temp.path())?;
    assert_eq!(catalog::control_bar_layout(&db)?, ControlBarLayout::Classic);
    catalog::save_control_bar_layout(&db, ControlBarLayout::Compact)?;
    assert_eq!(catalog::control_bar_layout(&db)?, ControlBarLayout::Compact);
    Ok(())
}

#[test]
fn progress_bar_preference_migrates_without_overwriting_layout() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = catalog::open(&temp.path().join("test.sqlite3"))?;
    db.execute_batch(
        "CREATE TABLE player_preferences(id INTEGER PRIMARY KEY CHECK(id=1),control_bar TEXT NOT NULL);
         INSERT INTO player_preferences VALUES(1,'Classic');",
    )?;
    catalog::initialize(&db)?;
    assert_eq!(catalog::control_bar_layout(&db)?, ControlBarLayout::Classic);
    assert_eq!(
        catalog::progress_bar_position(&db)?,
        ProgressBarPosition::FollowLayout
    );
    catalog::save_progress_bar_position(&db, ProgressBarPosition::Top)?;
    drop(db);
    let db = database(temp.path())?;
    assert_eq!(catalog::control_bar_layout(&db)?, ControlBarLayout::Classic);
    assert_eq!(
        catalog::progress_bar_position(&db)?,
        ProgressBarPosition::Top
    );
    Ok(())
}

fn scan(db: &mut Connection) -> Result<()> {
    scan_unverified(db)?;
    verify_fixtures(db)
}

fn verify_fixtures(db: &Connection) -> Result<()> {
    // Catalog and file-operation fixtures use synthetic bytes instead of real media.
    db.execute("UPDATE videos SET probe=1 WHERE probe=0", [])?;
    Ok(())
}

fn scan_unverified(db: &mut Connection) -> Result<()> {
    scan::refresh(db, &Control::default(), &Mutex::new(()), 0, true, |_| {})
}

#[test]
fn playlist_export_matches_catalog_filter_and_order() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("clips");
    fs::create_dir(&source)?;
    for name in ["Scene B.mp4", "Intro.mp4", "Scene A.mp4"] {
        fs::write(source.join(name), "fixture")?;
    }
    let mut db = database(temp.path())?;
    catalog::set_source(&db, &source, false)?;
    scan(&mut db)?;
    let filter = Filter {
        text: "Scene".into(),
        sort: Sort::Title,
        ..Filter::default()
    };
    let dest = temp.path().join("scene.m3u8");
    catalog::export_playlist(&db, &filter, &dest)?;
    let text = fs::read_to_string(dest)?;
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            "#EXTM3U",
            source.join("Scene A.mp4").to_str().unwrap(),
            source.join("Scene B.mp4").to_str().unwrap()
        ]
    );
    Ok(())
}

#[test]
fn warm_scan_reuses_checkpoints_and_refreshes_reconnected_drives() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let one = temp.path().join("one");
    let two = temp.path().join("two");
    fs::create_dir(&one)?;
    fs::create_dir(&two)?;
    fs::write(one.join("first.mp4"), "one")?;
    fs::write(two.join("second.mp4"), "two")?;
    let mut db = database(temp.path())?;
    catalog::register_drives(&mut db, &[one.clone(), two.clone()])?;
    scan(&mut db)?;
    drop(db);
    let mut db = database(temp.path())?;
    fs::write(one.join("new.mp4"), "new")?;
    fs::write(two.join("new.mp4"), "new")?;
    let control = Control::default();
    let writes = Mutex::new(());
    catalog::register_drives(&mut db, &[one.clone(), two.clone()])?;
    scan::refresh(&mut db, &control, &writes, 0, false, |_| {
        panic!("fresh sources were scanned")
    })?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(2)
    );

    catalog::register_drives(&mut db, std::slice::from_ref(&one))?;
    catalog::register_drives(&mut db, &[one, two])?;
    scan::refresh(&mut db, &control, &writes, 0, false, |_| {})?;
    verify_fixtures(&db)?;
    assert_eq!(control.progress().sources, 1);
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(3)
    );
    scan(&mut db)?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(4)
    );
    Ok(())
}

#[test]
fn watch_later_survives_restart_and_filters_the_catalog() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let media = temp.path().join("media");
    fs::create_dir(&media)?;
    fs::write(media.join("saved.mp4"), "video")?;
    fs::write(media.join("unsaved.mp4"), "video")?;
    let mut db = database(temp.path())?;
    catalog::register_drives(&mut db, &[media])?;
    scan(&mut db)?;
    let (_, videos) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    let id = videos
        .iter()
        .find(|video| video.title == "saved")
        .unwrap()
        .id;
    assert!(catalog::set_watch_later(&db, id, true)?);
    assert!(catalog::set_watch_later(&db, id, true)?);
    assert!(!catalog::set_watch_later(&db, i64::MAX, true)?);
    drop(db);

    let mut db = database(temp.path())?;
    assert_eq!(catalog::watch_later(&db)?, vec![id]);
    let filter = Filter {
        watch_later: true,
        ..Filter::default()
    };
    let (total, videos) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(total, Some(1));
    assert_eq!(videos[0].id, id);
    assert!(!catalog::set_watch_later(&db, id, false)?);
    assert_eq!(catalog::page(&mut db, &filter, 0, true)?.0, Some(0));
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(2)
    );
    Ok(())
}

#[test]
fn bulk_collection_membership_and_view_preferences_survive_restart() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut db = database(temp.path())?;
    fs::write(temp.path().join("one.mp4"), "one")?;
    fs::write(temp.path().join("two.mp4"), "two")?;
    catalog::set_source(&db, temp.path(), false)?;
    scan(&mut db)?;
    let (_, videos) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    let one = videos[0].id;
    let two = videos[1].id;
    assert_eq!(
        catalog::add_to_collection(&mut db, CollectionTarget::New("Favorites".into()), &[one])?,
        1
    );
    let collection = catalog::collections(&db)?[0].id;
    let target = CollectionTarget::Existing(collection);
    assert_eq!(
        catalog::add_to_collection(&mut db, target, &[one, two, two, i64::MAX])?,
        1
    );
    let view = LibraryView {
        list: true,
        size: 3,
    };
    catalog::save_view(&db, view)?;
    drop(db);
    let db = database(temp.path())?;
    assert!(catalog::library_view(&db)? == view);
    assert_eq!(catalog::memberships(&db, one)?, vec![collection]);
    assert_eq!(catalog::memberships(&db, two)?, vec![collection]);
    Ok(())
}

#[test]
fn scan_reuses_metadata_and_protects_offline_roots() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("Footage");
    let excluded = root.join("Private");
    fs::create_dir_all(&excluded)?;
    let main = root.join("Hike 100%_edited.MP4");
    let other = root.join("Hike 100aZedited.MP4");
    fs::write(&main, "sample bytes")?;
    fs::write(&other, "another video")?;
    fs::write(excluded.join("Skipped.mov"), "private")?;
    fs::write(
        root.join("source.ts"),
        "export const filename = 'not a video';",
    )?;

    let mut db = database(temp.path())?;
    catalog::set_source(&db, &root, false)?;
    catalog::set_source(&db, &excluded, true)?;
    scan(&mut db)?;
    let (total, _) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    assert_eq!(total, Some(2));
    let filter = Filter {
        filename: "100%_".into(),
        folder: "footage".into(),
        ext: ".MP4".into(),
        ..Filter::default()
    };
    let (count, rows) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(count, Some(1));
    let indexed = &rows[0];
    assert_eq!(indexed.path, main);
    db.execute("UPDATE videos SET probe=0 WHERE id=?", [indexed.id])?;
    catalog::set_media(
        &db,
        indexed,
        &Media {
            duration: Some(18.0),
            width: Some(1080),
            height: Some(1920),
            codec: Some("h264".into()),
            ..Media::default()
        },
    )?;
    db.execute(
        "INSERT INTO thumbs(video,revision,cached_at,jpeg) VALUES(?,?,?,?)",
        params![indexed.id, indexed.rev, now(), &[1u8, 2, 3][..]],
    )?;
    scan(&mut db)?;
    let updated = catalog::get_visible(&db, indexed.id)?.unwrap();
    assert_eq!(updated.rev, indexed.rev);
    assert_eq!(updated.media.duration, Some(18.0));
    let (total, _) = catalog::page(
        &mut db,
        &Filter {
            min_height: Some(1080),
            max_height: Some(2160),
            min_seconds: Some(10),
            max_seconds: Some(20),
            codec: "H264".into(),
            added_since: Some(indexed.added),
            ..Filter::default()
        },
        0,
        true,
    )?;
    assert_eq!(total, Some(1));

    fs::rename(&root, temp.path().join("Offline"))?;
    let control = Control::default();
    scan::refresh(&mut db, &control, &Mutex::new(()), 0, true, |_| {})?;
    let report = control.progress();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.skipped_paths[0].0, root);
    assert!(!report.skipped_paths[0].1.is_empty());
    assert!(catalog::get_visible(&db, indexed.id)?.is_some());
    fs::rename(temp.path().join("Offline"), &root)?;
    fs::write(&main, "new video content, longer than the original")?;
    scan_unverified(&mut db)?;
    assert!(catalog::get_visible(&db, indexed.id)?.is_none());
    let updated = catalog::get(&db, indexed.id)?.unwrap();
    assert_eq!(updated.rev, indexed.rev + 1);
    assert_eq!(updated.added, indexed.added);
    assert!(updated.media.duration.is_none());
    let thumbnails: i64 = db.query_row("SELECT count(*) FROM thumbs", [], |row| row.get(0))?;
    assert_eq!(thumbnails, 0);
    Ok(())
}

#[test]
fn pause_and_resume_scan() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let folder = temp.path().join("incoming");
    fs::create_dir(&folder)?;
    fs::write(folder.join("video.mp4"), "media")?;
    let mut db = database(temp.path())?;
    catalog::set_source(&db, &folder, false)?;
    let control = Control::default();
    control.set_paused(true);
    let (started, waiting) = crossbeam_channel::bounded(1);
    std::thread::scope(|scope| -> Result<()> {
        let control = &control;
        let handle = scope.spawn(move || -> Result<()> {
            scan::refresh(&mut db, control, &Mutex::new(()), 0, true, |_| {
                let _ = started.try_send(());
            })
        });
        let acknowledged = waiting.recv_timeout(Duration::from_secs(5));
        let paused = control.progress();
        control.set_paused(false);
        acknowledged?;
        handle.join().unwrap()?;
        assert!(paused.running && paused.paused);
        assert_eq!(paused.found, 0);
        assert_eq!(control.progress().found, 1);
        assert!(!control.progress().running);
        Ok(())
    })
}

#[test]
fn moves_keep_collections_and_transfers_preserve_existing_files() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("source");
    let dest = temp.path().join("destination");
    let copied = temp.path().join("other");
    let filename = "clip with spaces.mp4";
    for path in [&root, &dest, &copied] {
        fs::create_dir(path)?;
    }
    let root = dunce::canonicalize(root)?;
    let dest = dunce::canonicalize(dest)?;
    let copied = dunce::canonicalize(copied)?;
    let path = root.join(filename);
    fs::write(&path, "video data")?;
    fs::write(dest.join(filename), "previous file")?;
    let mut db = database(temp.path())?;
    catalog::set_source(&db, &root, false)?;
    scan(&mut db)?;
    catalog::add_to_collection(&mut db, CollectionTarget::New("Trips".into()), &[])?;
    let album = &catalog::collections(&db)?[0];
    let (_, videos) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    let original = &videos[0];
    catalog::set_membership(&db, original.id, album.id, true)?;
    let writes = Mutex::new(());

    assert!(files::transfer(&mut db, &writes, original, &dest, true).is_err());
    assert_eq!(fs::read(dest.join(filename))?, b"previous file");
    assert_eq!(fs::read(&path)?, b"video data");
    let copy = files::transfer(&mut db, &writes, original, &copied, false)?;
    assert_eq!(fs::read(&copy)?, b"video data");
    assert!(path.is_file());
    fs::rename(dest.join(filename), dest.join("previous.mp4"))?;
    let moved = files::transfer(&mut db, &writes, original, &dest, true)?;
    assert!(!path.exists());
    assert_eq!(fs::read(&moved)?, b"video data");
    let filter = Filter {
        collection: Some(album.id),
        ..Filter::default()
    };
    let (count, videos) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(count, Some(1));
    let selected = &videos[0];
    assert_eq!(selected.id, original.id);
    assert_eq!(selected.path, moved);
    assert!(files::delete(&db, &writes, original, files::DeleteMode::Permanent).is_err());
    fs::write(&moved, "changed since selection")?;
    assert!(files::delete(&db, &writes, selected, files::DeleteMode::Permanent).is_err());
    assert_eq!(fs::read(&moved)?, b"changed since selection");
    scan(&mut db)?;
    let fresh = catalog::get_visible(&db, selected.id)?.unwrap();
    files::delete(&db, &writes, &fresh, files::DeleteMode::Permanent)?;
    assert!(!moved.exists());
    assert!(catalog::memberships(&db, selected.id)?.is_empty());
    Ok(())
}

#[test]
fn large_catalog_returns_only_the_requested_page() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut db = database(temp.path())?;
    catalog::set_source(&db, temp.path(), false)?;
    let source = catalog::sources(&db, "sources")?[0].id;
    let tx = db.transaction()?;
    for index in 0..50_000 {
        let name = format!("Video {index:05}.mp4");
        let (id, _) = catalog::upsert(
            &tx,
            &temp.path().join(name),
            "mp4",
            Stamp {
                bytes: index,
                modified: 0,
            },
            None,
            index,
        )?;
        catalog::sight(&tx, source, id, 1)?;
    }
    verify_fixtures(&tx)?;
    tx.commit()?;
    let start = Instant::now();
    let filter = Filter {
        sort: Sort::Title,
        ..Filter::default()
    };
    let (count, page) = catalog::page(&mut db, &filter, 490, true)?;
    let browse_time = start.elapsed();
    assert_eq!(count, Some(50_000));
    assert_eq!(page.len(), PAGE);
    assert_eq!(page[0].title, "Video 47040");
    let start = Instant::now();
    let filter = Filter {
        text: "499".into(),
        ..filter
    };
    let (count, found) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(count, Some(200));
    assert_eq!(found.len(), PAGE);
    let (_, next_page) = catalog::page(&mut db, &filter, 1, false)?;
    let anchor = found[4].id;
    let endpoint = next_page[9].id;
    let range = catalog::range_ids(&db, &filter, endpoint, anchor)?;
    let expected: Vec<_> = found[4..]
        .iter()
        .chain(&next_page[..10])
        .map(|video| video.id)
        .collect();
    assert_eq!(range, expected);
    assert!(catalog::range_ids(&db, &filter, anchor, i64::MAX).is_err());
    println!(
        "50,000 records: deep page + count {browse_time:?}, substring search + count {:?}",
        start.elapsed()
    );
    Ok(())
}

#[test]
fn bulk_file_operations_continue_safely_after_a_collision() -> Result<()> {
    use crate::files::{DeleteMode, Operation};
    use crate::service::manage_files;

    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let destination = temp.path().join("dest");
    fs::create_dir(&source)?;
    fs::create_dir(&destination)?;
    let source = dunce::canonicalize(source)?;
    let destination = dunce::canonicalize(destination)?;
    fs::write(source.join("collision.mp4"), b"from source")?;
    fs::write(source.join("transfer.mp4"), b"video")?;
    fs::write(destination.join("collision.mp4"), b"keep this")?;
    let mut db = database(temp.path())?;
    catalog::set_source(&db, &source, false)?;
    scan(&mut db)?;
    let (_, videos) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    let writes = Mutex::new(());
    let copied = manage_files(
        &mut db,
        &writes,
        videos.clone(),
        Operation::Transfer {
            destination: destination.clone(),
            moving: false,
        },
        |_| {},
    );
    assert_eq!(copied.succeeded.len(), 1);
    assert_eq!(copied.failed[0].0, source.join("collision.mp4"));
    assert_eq!(fs::read(destination.join("collision.mp4"))?, b"keep this");
    assert_eq!(fs::read(destination.join("transfer.mp4"))?, b"video");
    assert!(source.join("transfer.mp4").is_file());

    fs::write(
        source.join("collision.mp4"),
        b"new contents after confirmation",
    )?;
    let deleted = manage_files(
        &mut db,
        &writes,
        videos,
        Operation::Delete(DeleteMode::Permanent),
        |_| {},
    );
    assert_eq!(deleted.succeeded.len(), 1);
    assert_eq!(deleted.failed[0].0, source.join("collision.mp4"));
    assert!(source.join("collision.mp4").is_file());
    assert!(!source.join("transfer.mp4").exists());
    Ok(())
}

#[test]
fn mounted_drives_are_default_sources_and_offline_mounts_keep_their_index() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let main = temp.path().join("system-drive");
    let external = temp.path().join("external-drive");
    let manual = temp.path().join("manually-added");
    for dir in [&main, &external, &manual] {
        fs::create_dir(dir)?;
        fs::write(dir.join("sample.mp4"), "fixture")?;
    }
    let mut db = database(temp.path())?;
    assert!(catalog::auto_scan(&db)?);
    catalog::set_source(&db, &manual, false)?;
    catalog::register_drives(&mut db, &[main.clone(), external.clone()])?;
    scan(&mut db)?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(3)
    );
    assert_eq!(
        catalog::sources(&db, "sources")?
            .iter()
            .filter(|source| source.automatic)
            .count(),
        2
    );

    // When a drive is unplugged, its mountpoint can still be a real empty directory.
    fs::rename(external.join("sample.mp4"), temp.path().join("offline.mp4"))?;
    catalog::register_drives(&mut db, std::slice::from_ref(&main))?;
    scan(&mut db)?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(3)
    );

    catalog::set_auto_scan(&db, false)?;
    fs::write(main.join("another.mp4"), "automatic fixture")?;
    fs::write(manual.join("another.mp4"), "manual fixture")?;
    scan(&mut db)?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(4)
    );
    assert!(!catalog::auto_scan(&database(temp.path())?)?);

    catalog::set_auto_scan(&db, true)?;
    catalog::set_source(&db, &main, true)?;
    catalog::register_drives(&mut db, &[main, external])?;
    scan(&mut db)?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(2)
    );
    Ok(())
}

#[test]
fn hover_preview_decodes_motion_in_a_small_sprite() -> Result<()> {
    let Some(ffmpeg) = crate::media::Tools::find().ffmpeg else {
        eprintln!(
            "FFmpeg unavailable. Skipping the media fixture; Linux desktop CI installs FFmpeg."
        );
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let clip = temp.path().join("motion.mp4");
    let status = std::process::Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=16",
            "-t",
            "3",
            "-c:v",
            "mpeg4",
            "-threads",
            "1",
        ])
        .arg(&clip)
        .status()?;
    assert!(status.success());

    let mut db = database(temp.path())?;
    catalog::set_source(&db, temp.path(), false)?;
    scan(&mut db)?;
    let (_, videos) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    let video = &videos[0];
    let frame = crate::media::preview_segment(&db, Some(&ffmpeg), video, 0, &|| true)?.unwrap();
    assert_eq!(frame.size, [640, 180]);
    let changed = (0..90).any(|row| {
        let offset = row * frame.size[0];
        frame.pixels[offset..offset + 160] != frame.pixels[offset + 160..offset + 320]
    });
    assert!(changed, "consecutive preview frames should contain motion");
    assert_eq!(
        crate::media::preview_segment(&db, None, video, 0, &|| true)?.unwrap(),
        frame,
    );
    assert!(crate::media::preview_segment(&db, Some(&ffmpeg), video, 1, &|| false).is_err());
    let key = (video.id, video.rev, 2250);
    let (_, draft) = crate::media::timeline_preview(&ffmpeg, video, 2.25, false, &|| true)?;
    let (jpeg, exact) = crate::media::timeline_preview(&ffmpeg, video, 2.25, true, &|| true)?;
    let times = [163, 500, 1063, 1500, 2063, 2500];
    let frames = crate::media::filmstrip_frames(&ffmpeg, video, &times, &|| true)?;
    for (ms, (_, batch)) in times.into_iter().zip(frames) {
        let (_, single) =
            crate::media::timeline_preview(&ffmpeg, video, ms as f64 / 1000.0, true, &|| true)?;
        // JPEG rate control can differ across a sequence. Compare the actual picture.
        let error: usize = single
            .pixels
            .iter()
            .zip(&batch.pixels)
            .map(|(a, b)| a.r().abs_diff(b.r()) as usize)
            .sum();
        assert!(error < batch.pixels.len() * 3, "wrong frame at {ms} ms");
    }
    assert_eq!(draft.size, exact.size);
    crate::media::store_timeline(&db, key, &jpeg)?;
    assert_eq!(crate::media::cached_timeline(&db, key)?, Some(exact));
    assert!(crate::media::cached_timeline(&db, (video.id, video.rev + 1, 2250))?.is_none());
    // Replacing an existing frame must count its bytes only once. A lowered
    // budget evicts frames and stays accurate even when the cache is empty.
    crate::media::store_timeline(&db, key, &jpeg)?;
    let used: i64 = db.query_row("SELECT bytes_used FROM timeline_budget", [], |row| {
        row.get(0)
    })?;
    let actual: i64 = db.query_row("SELECT sum(length(jpeg)) FROM timeline_frames", [], |row| {
        row.get(0)
    })?;
    assert_eq!(used, actual);
    db.execute("UPDATE timeline_budget SET bytes_allowed=1", [])?;
    crate::media::trim_timeline(&db)?;
    assert!(crate::media::cached_timeline(&db, key)?.is_none());
    assert_eq!(
        db.query_row("SELECT bytes_used FROM timeline_budget", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    let mut clocked = video.clone();
    clocked.media.duration = Some(3.0);
    clocked.media.fps = Some(16.0);
    let (_, last_frame) = crate::media::timeline_preview(&ffmpeg, &clocked, 2.937, true, &|| true)?;
    let (_, end_preview) =
        crate::media::timeline_preview(&ffmpeg, &clocked, 2.999, true, &|| true)?;
    assert_eq!(end_preview, last_frame);
    Ok(())
}

#[test]
fn only_verified_videos_are_searchable_in_new_and_upgraded_catalogs() -> Result<()> {
    let tools = crate::media::Tools::find();
    let (Some(ffmpeg), Some(ffprobe)) = (tools.ffmpeg, tools.ffprobe) else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let old_module = temp.path().join("legacy.module.mod");
    let new_module = temp.path().join("v1.8.15.mod");
    let still = temp.path().join("still.mp4");
    let text = temp.path().join("renamed.mp4");
    let video = temp.path().join("camera.mod");
    fs::write(&old_module, "module example.com/legacy\n")?;
    fs::write(&new_module, "module example.com/new\n")?;
    fs::write(&text, "plain text")?;
    for (path, format, codec, count) in [
        (&still, "image2", "png", "1"),
        (&video, "mpeg", "mpeg2video", "12"),
    ] {
        let status = std::process::Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x96:rate=25",
                "-frames:v",
                count,
                "-threads",
                "1",
                "-c:v",
                codec,
                "-f",
                format,
            ])
            .arg(path)
            .status()?;
        assert!(status.success());
    }
    let mut db = database(temp.path())?;
    catalog::set_source(&db, temp.path(), false)?;
    let source = catalog::sources(&db, "sources")?[0].id;
    let (legacy, _) = catalog::upsert(
        &db,
        &old_module,
        "mod",
        Stamp::read(&old_module)?,
        None,
        now(),
    )?;
    catalog::sight(&db, source, legacy, 1)?;
    catalog::set_media(&db, &catalog::get(&db, legacy)?.unwrap(), &Media::default())?;
    db.pragma_update(None, "user_version", 0)?;
    catalog::initialize(&db)?;
    scan_unverified(&mut db)?;
    assert_eq!(
        catalog::page(&mut db, &Filter::default(), 0, true)?.0,
        Some(0)
    );
    assert_eq!(catalog::path_probe(&db, &new_module)?, None);
    for item in catalog::pending(&db, 0)? {
        match crate::media::probe(&ffprobe, &item.path, None) {
            Ok(info) => {
                assert_eq!(item.path, video);
                catalog::set_media(&db, &item, &info)?;
            }
            Err(error) => {
                assert_ne!(item.path, video, "{error:#}");
                catalog::reject_media(&db, &item)?;
            }
        }
    }
    let (count, rows) = catalog::page(&mut db, &Filter::default(), 0, true)?;
    assert_eq!(count, Some(1));
    assert_eq!(rows[0].path, video);
    assert_eq!(catalog::path_probe(&db, &old_module)?, Some(2));
    assert_eq!(catalog::path_probe(&db, &still)?, Some(2));
    assert_eq!(catalog::path_probe(&db, &text)?, Some(2));
    Ok(())
}

#[test]
fn date_filter_and_sort_preserve_an_existing_catalog() -> Result<()> {
    use crate::model::DateRange;
    use chrono::NaiveDate;
    let temp = tempfile::tempdir()?;
    let mut db = catalog::open(&temp.path().join("older.sqlite3"))?;
    let previous_schema = include_str!("schema.sql").replace("    created INTEGER,\n", "");
    db.execute_batch(&previous_schema)?;
    catalog::set_source(&db, temp.path(), false)?;
    let source = catalog::sources(&db, "sources")?[0].id;
    let legacy: i64 = db.query_row(
        "INSERT INTO videos(path,title,filename,folder,search,ext,bytes,modified,added,probe,codec) VALUES(?,'Legacy','legacy.mp4','','legacy','mp4',0,0,100,1,'h264') RETURNING id",
        [crate::model::path_key(&temp.path().join("legacy.mp4"))], |row| row.get(0),
    )?;
    catalog::sight(&db, source, legacy, 1)?;

    catalog::initialize(&db)?;
    assert!(
        catalog::get_visible(&db, legacy)?
            .unwrap()
            .created
            .is_none()
    );
    let day = NaiveDate::from_ymd_opt(2024, 2, 14).unwrap();
    let date = DateRange {
        from: Some(day),
        through: Some(day),
    };
    let (first, next) = date.bounds(true);
    let first = first.unwrap();
    let next = next.unwrap();
    let samples = [
        ("Early", first - 1, first + 25),
        ("Late", next - 1, next),
        ("Newest", next, first - 1),
    ];
    for (name, created, modified) in samples {
        let (id, _) = catalog::upsert(
            &db,
            &temp.path().join(format!("{name}.mp4")),
            "mp4",
            Stamp {
                bytes: 100,
                modified,
            },
            Some(created),
            created / 1_000_000,
        )?;
        catalog::sight(&db, source, id, 1)?;
    }
    verify_fixtures(&db)?;

    let filter = Filter {
        dates: [DateRange::default(), date, DateRange::default()],
        ..Filter::default()
    };
    let (_, matches) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(
        matches
            .iter()
            .map(|video| video.title.as_str())
            .collect::<Vec<_>>(),
        ["Late"]
    );
    let filter = Filter {
        dates: [DateRange::default(), DateRange::default(), date],
        ..Filter::default()
    };
    let (_, matches) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(
        matches
            .iter()
            .map(|video| video.title.as_str())
            .collect::<Vec<_>>(),
        ["Early"]
    );

    for (sort, expected) in [
        (Sort::CreatedNewest, ["Newest", "Late", "Early", "Legacy"]),
        (Sort::CreatedOldest, ["Early", "Late", "Newest", "Legacy"]),
        (Sort::ModifiedNewest, ["Late", "Early", "Newest", "Legacy"]),
        (Sort::ModifiedOldest, ["Legacy", "Newest", "Early", "Late"]),
        (Sort::Recent, ["Newest", "Late", "Early", "Legacy"]),
        (Sort::AddedOldest, ["Legacy", "Early", "Late", "Newest"]),
    ] {
        let (_, matches) = catalog::page(
            &mut db,
            &Filter {
                sort,
                ..Filter::default()
            },
            0,
            true,
        )?;
        assert_eq!(
            matches
                .iter()
                .map(|video| video.title.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
    Ok(())
}

#[test]
fn title_and_duration_sorts_use_the_displayed_metadata() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut db = database(temp.path())?;
    catalog::set_source(&db, temp.path(), false)?;
    let source = catalog::sources(&db, "sources")?[0].id;
    for (file, ext, seconds) in [
        ("Clip.part.mp4", "mp4", Some(12.0)),
        ("clip.mkv", "mkv", Some(6.0)),
        ("Next.mov", "mov", None),
    ] {
        let (id, _) = catalog::upsert(
            &db,
            &temp.path().join(file),
            ext,
            Stamp {
                bytes: 1,
                modified: 0,
            },
            None,
            1,
        )?;
        catalog::sight(&db, source, id, 1)?;
        db.execute(
            "UPDATE videos SET probe=1,duration=? WHERE id=?",
            params![seconds, id],
        )?;
    }
    for (sort, expected) in [
        (Sort::Title, ["clip", "Clip.part", "Next"]),
        (Sort::Duration, ["Clip.part", "clip", "Next"]),
    ] {
        let (_, matches) = catalog::page(
            &mut db,
            &Filter {
                sort,
                ..Filter::default()
            },
            0,
            false,
        )?;
        assert_eq!(
            matches
                .iter()
                .map(|video| video.title.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
    Ok(())
}

#[test]
fn sizes_and_unicode_folder_search_use_exact_boundaries() -> Result<()> {
    use crate::model::minimum_size;
    assert_eq!(minimum_size(""), Some(None));
    assert_eq!(minimum_size("300 KB"), Some(Some(300_000)));
    assert_eq!(minimum_size("۰٫۵ MB"), Some(Some(500_000)));
    assert_eq!(minimum_size("2.5 MiB"), Some(Some(2_621_440)));
    assert_eq!(minimum_size("0.0000001 MB"), Some(Some(1)));
    assert_eq!(minimum_size("-2 MB"), None);
    assert_eq!(minimum_size("nonsense"), None);

    let temp = tempfile::tempdir()?;
    let dir = temp.path().join("Media Folder");
    let child = dir.join("فیلم ها");
    let neighbor = temp.path().join("Media Folder-old");
    let mut db = database(temp.path())?;
    catalog::set_source(&db, temp.path(), false)?;
    let source = catalog::sources(&db, "sources")?[0].id;
    for (folder, title, bytes) in [
        (&dir, "سفر تهران", 299_999),
        (&child, "中文视频", 300_000),
        (&neighbor, "Another clip", 800_000),
    ] {
        let (id, _) = catalog::upsert(
            &db,
            &folder.join(format!("{title}.mp4")),
            "mp4",
            Stamp { bytes, modified: 0 },
            None,
            1,
        )?;
        catalog::sight(&db, source, id, 1)?;
    }
    verify_fixtures(&db)?;

    let filter = Filter {
        directory: Some(dir.clone()),
        min_bytes: minimum_size("300 KB").flatten(),
        ..Filter::default()
    };
    let (count, rows) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(count, Some(1));
    assert_eq!(rows[0].title, "中文视频");
    let filter = Filter {
        text: child.display().to_string(),
        ..Filter::default()
    };
    let (count, rows) = catalog::page(&mut db, &filter, 0, true)?;
    assert_eq!(count, Some(1));
    assert_eq!(rows[0].title, "中文视频");

    for term in ["تهران", "中文"] {
        let (count, _) = catalog::page(
            &mut db,
            &Filter {
                text: term.into(),
                ..Filter::default()
            },
            0,
            true,
        )?;
        assert_eq!(count, Some(1), "{term}");
    }
    for (sort, expected) in [
        (Sort::Size, [800_000, 300_000, 299_999]),
        (Sort::Smallest, [299_999, 300_000, 800_000]),
    ] {
        let (_, results) = catalog::page(
            &mut db,
            &Filter {
                sort,
                ..Filter::default()
            },
            0,
            true,
        )?;
        assert_eq!(
            results
                .iter()
                .map(|video| video.stamp.bytes)
                .collect::<Vec<_>>(),
            expected
        );
    }
    Ok(())
}

#[test]
fn resume_checkpoint_survives_restart_and_ignores_replaced_files() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = database(temp.path())?;
    let path = temp.path().join("movie.mp4");
    let (id, _) = catalog::upsert(
        &db,
        &path,
        "mp4",
        Stamp {
            bytes: 10,
            modified: 1,
        },
        None,
        1,
    )?;
    let original = (id, 1);
    catalog::save_playback_position(&db, original, Some(31.25))?;
    drop(db);

    let db = database(temp.path())?;
    assert_eq!(catalog::playback_position(&db, original)?, Some(31.25));
    catalog::save_playback_position(&db, original, None)?;
    assert_eq!(catalog::playback_position(&db, original)?, None);
    catalog::save_playback_position(&db, original, Some(5.0))?;
    catalog::upsert(
        &db,
        &path,
        "mp4",
        Stamp {
            bytes: 20,
            modified: 2,
        },
        None,
        2,
    )?;
    let replacement = (id, 2);
    assert_eq!(catalog::playback_position(&db, original)?, None);
    assert_eq!(catalog::playback_position(&db, replacement)?, None);
    catalog::save_playback_position(&db, replacement, Some(16.0))?;
    catalog::save_playback_position(&db, original, Some(97.0))?;
    catalog::save_playback_position(&db, original, None)?;
    assert_eq!(catalog::playback_position(&db, replacement)?, Some(16.0));
    Ok(())
}

#[test]
fn marquee_and_clipboard_resolve_catalog_entries_across_pages() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut db = database(temp.path())?;
    let paths: Vec<_> = (0..PAGE + 10)
        .map(|index| temp.path().join(format!("clip {index:03}.mp4")))
        .collect();
    let ids: Vec<_> = paths
        .iter()
        .rev()
        .enumerate()
        .map(|(index, path)| {
            catalog::upsert(
                &db,
                path,
                "mp4",
                Stamp {
                    bytes: 10,
                    modified: 1,
                },
                None,
                index as i64,
            )
            .map(|(id, _)| id)
        })
        .collect::<Result<_>>()?;
    db.execute("UPDATE videos SET pinned=1,probe=1", [])?;
    let filter = Filter {
        sort: Sort::default(),
        ..Filter::default()
    };
    let grid = catalog::CatalogRegion {
        columns: 4,
        rows: PAGE / 4 - 1..PAGE / 4 + 2,
        cells: 1..3,
    };
    let expected: Vec<_> = (PAGE - 4..PAGE + 8)
        .filter(|index| grid.contains(*index))
        .map(|index| ids[ids.len() - 1 - index])
        .collect();
    assert_eq!(catalog::region_ids(&db, &filter, grid)?, expected);
    let list = catalog::CatalogRegion {
        columns: 1,
        rows: PAGE - 2..PAGE + 2,
        cells: 0..1,
    };
    assert_eq!(
        catalog::region_ids(&db, &filter, list)?,
        (PAGE - 2..PAGE + 2)
            .map(|index| ids[ids.len() - 1 - index])
            .collect::<Vec<_>>()
    );

    assert_eq!(
        catalog::ids_at_positions(&db, &filter, vec![PAGE + 3, 0, PAGE, PAGE - 1, PAGE])?,
        [0, PAGE - 1, PAGE, PAGE + 3].map(|index| ids[ids.len() - 1 - index])
    );

    let videos = catalog::selection_files(&mut db, vec![ids[1], ids[PAGE + 2], ids[0]])?;
    let expected = [
        paths[7].as_path(),
        paths[PAGE + 8].as_path(),
        paths[PAGE + 9].as_path(),
    ]
    .map(|path| path.to_str().unwrap())
    .join("\n");
    assert_eq!(
        crate::service::clipboard_paths(videos.iter().map(|video| video.path.as_path())),
        expected
    );
    Ok(())
}
