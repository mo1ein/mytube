use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};

use crate::model::{
    Appearance, Collection, CollectionTarget, ControlBarLayout, Filter, LibraryView, Media, PAGE,
    ProgressBarPosition, Sort, Source, Stamp, Video, from_key, path_key,
};

const VIDEO_FIELDS: &str = "v.id,v.rev,v.path,v.title,v.ext,v.bytes,v.modified,v.added,v.duration,v.width,v.height,v.codec,v.audio,v.fps,v.created";
const IN_LIBRARY: &str = "(v.pinned=1 OR v.indexed=1)";
const VISIBLE: &str = "v.probe=1 AND (v.pinned=1 OR v.indexed=1)";

pub fn open(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(8))?;
    db.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-4096;",
    )?;
    Ok(db)
}

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(include_str!("schema.sql"))?;
    // Existing settings have already been migrated from their saved layout.
    let defaults =
        serde_json::json!({"interface_theme": crate::player::options::InterfaceTheme::default()});
    db.execute(
        "INSERT OR IGNORE INTO player_settings(id,options) VALUES(1,?)",
        [defaults.to_string()],
    )?;
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('videos') WHERE name='created')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        db.execute("ALTER TABLE videos ADD COLUMN created INTEGER", [])?;
    }
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('player_preferences') WHERE name='progress_bar')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        db.execute(
            "ALTER TABLE player_preferences ADD COLUMN progress_bar TEXT NOT NULL DEFAULT 'Follow layout'",
            [],
        )?;
    }
    db.execute_batch("CREATE INDEX IF NOT EXISTS videos_created ON videos(created DESC,id DESC) WHERE pinned=1 OR indexed=1;
        CREATE INDEX IF NOT EXISTS videos_modified ON videos(modified DESC,id DESC) WHERE pinned=1 OR indexed=1;")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        let tx = db.unchecked_transaction()?;
        // Older catalogs marked failed probes as complete and could accept a still image stream.
        tx.execute(
            "UPDATE videos SET probe=0,rev=rev+1,duration=NULL,width=NULL,height=NULL,codec=NULL,audio=NULL,fps=NULL
             WHERE probe=1 AND (codec IS NULL OR ext='mod' OR codec IN
             ('mjpeg','jpegls','jpeg2000','jpegxl','ljpeg','png','apng','bmp','gif','webp','tiff','targa',
              'av1','hevc','rawvideo','dpx','exr','pam','pbm','pgm','ppm','pcx','qoi','svg','xpm'))",
            [],
        )?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = db.unchecked_transaction()?;
        // Older releases serialized the 400 ms default alongside customized settings.
        tx.execute(
            "UPDATE player_settings SET options=json_set(options,'$.hide_delay_ms',?)
             WHERE json_extract(options,'$.hide_delay_ms')=400",
            [crate::player::options::DEFAULT_HIDE_DELAY_MS],
        )?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
    }
    Ok(())
}

pub fn sources(db: &Connection, table: &str) -> Result<Vec<Source>> {
    let sql = match table {
        "sources" => {
            "SELECT s.id,s.path,a.source IS NOT NULL FROM sources s LEFT JOIN automatic_drives a ON s.id=a.source ORDER BY s.id"
        }
        _ => "SELECT id,path,0 FROM exclusions ORDER BY id",
    };
    collect(db, sql, [], |row| {
        Ok(Source {
            id: row.get(0)?,
            path: from_key(row.get_ref(1)?.as_blob()?),
            automatic: row.get(2)?,
        })
    })
}

pub fn scan_sources(db: &Connection, recent_after: Option<i64>) -> Result<Vec<Source>> {
    collect(
        db,
        "SELECT s.id,s.path,a.source IS NOT NULL FROM sources s
         LEFT JOIN automatic_drives a ON s.id=a.source
         LEFT JOIN source_scan_history h ON s.id=h.source
         WHERE (a.source IS NULL OR (a.online=1 AND (SELECT scan_drives FROM library_preferences WHERE id=1)))
           AND (?1 IS NULL OR h.completed IS NULL OR h.completed<=?1 OR h.completed>?2)
         ORDER BY s.id",
        params![recent_after, crate::model::now()],
        |row| {
            Ok(Source {
                id: row.get(0)?,
                path: from_key(row.get_ref(1)?.as_blob()?),
                automatic: row.get(2)?,
            })
        },
    )
}

pub fn invalidate_scan_history(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM source_scan_history", [])?;
    Ok(())
}

pub fn auto_scan(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT scan_drives FROM library_preferences WHERE id=1",
        [],
        |row| row.get(0),
    )?)
}

pub fn set_auto_scan(db: &Connection, enable: bool) -> Result<()> {
    db.execute(
        "UPDATE library_preferences SET scan_drives=? WHERE id=1",
        [enable],
    )?;
    Ok(())
}

pub fn library_view(db: &Connection) -> Result<LibraryView> {
    Ok(
        db.query_row("SELECT list,size FROM library_view WHERE id=1", [], |row| {
            Ok(LibraryView {
                list: row.get(0)?,
                size: row.get::<_, i64>(1)?.clamp(0, 3) as usize,
            })
        })?,
    )
}

pub fn save_view(db: &Connection, view: LibraryView) -> Result<()> {
    db.execute(
        "UPDATE library_view SET list=?,size=? WHERE id=1",
        params![view.list, view.size as i64],
    )?;
    Ok(())
}

pub fn appearance(db: &Connection) -> Result<Appearance> {
    let theme: String = db.query_row("SELECT theme FROM appearance WHERE id=1", [], |row| {
        row.get(0)
    })?;
    Ok(Appearance::ALL
        .into_iter()
        .find(|choice| choice.label() == theme)
        .unwrap_or_default())
}

pub fn save_appearance(db: &Connection, appearance: Appearance) -> Result<()> {
    db.execute(
        "UPDATE appearance SET theme=? WHERE id=1",
        [appearance.label()],
    )?;
    Ok(())
}

pub fn control_bar_layout(db: &Connection) -> Result<ControlBarLayout> {
    let layout: String = db.query_row(
        "SELECT control_bar FROM player_preferences WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    Ok(ControlBarLayout::ALL
        .into_iter()
        .find(|choice| {
            choice.label() == layout
                || (*choice == ControlBarLayout::Classic && layout == "Classic")
        })
        .unwrap_or_default())
}

pub fn save_control_bar_layout(db: &Connection, layout: ControlBarLayout) -> Result<()> {
    db.execute(
        "UPDATE player_preferences SET control_bar=? WHERE id=1",
        [layout.label()],
    )?;
    Ok(())
}

pub fn progress_bar_position(db: &Connection) -> Result<ProgressBarPosition> {
    let position: String = db.query_row(
        "SELECT progress_bar FROM player_preferences WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    Ok(ProgressBarPosition::ALL
        .into_iter()
        .find(|choice| choice.label() == position)
        .unwrap_or_default())
}

pub fn save_progress_bar_position(db: &Connection, position: ProgressBarPosition) -> Result<()> {
    db.execute(
        "UPDATE player_preferences SET progress_bar=? WHERE id=1",
        [position.label()],
    )?;
    Ok(())
}

pub fn register_drives(db: &mut Connection, drives: &[PathBuf]) -> Result<()> {
    let tx = db.transaction()?;
    // Keep offline volumes and their catalog entries. The absence of a volume must
    // not be mistaken for an empty mount point on the underlying filesystem.
    let online = collect(
        &tx,
        "SELECT s.id,s.path FROM sources s JOIN automatic_drives a ON s.id=a.source WHERE a.online=1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, from_key(row.get_ref(1)?.as_blob()?))),
    )?;
    for (id, path) in online {
        if !drives.contains(&path) {
            tx.execute("UPDATE automatic_drives SET online=0 WHERE source=?", [id])?;
            tx.execute("DELETE FROM source_scan_history WHERE source=?", [id])?;
        }
    }
    for drive in drives {
        let key = path_key(drive);
        if let Some(id) = tx
            .query_row(
                "INSERT INTO sources(path) VALUES(?) ON CONFLICT DO NOTHING RETURNING id",
                [&key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            tx.execute(
                "INSERT INTO automatic_drives(source,online) VALUES(?,1)",
                [id],
            )?;
        } else {
            tx.execute("UPDATE automatic_drives SET online=1 WHERE online=0 AND source=(SELECT id FROM sources WHERE path=?)", [&key])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn set_source(db: &Connection, path: &Path, exclusion: bool) -> Result<()> {
    let sql = if exclusion {
        "INSERT OR IGNORE INTO exclusions(path) VALUES(?)"
    } else {
        "INSERT OR IGNORE INTO sources(path) VALUES(?)"
    };
    db.execute(sql, [path_key(path)])?;
    if !exclusion {
        db.execute(
            "DELETE FROM automatic_drives WHERE source=(SELECT id FROM sources WHERE path=?)",
            [path_key(path)],
        )?;
    }
    Ok(())
}

pub fn remove_source(db: &Connection, id: i64, exclusion: bool) -> Result<()> {
    let sql = if exclusion {
        "DELETE FROM exclusions WHERE id=?"
    } else {
        "DELETE FROM sources WHERE id=?"
    };
    db.execute(sql, [id])?;
    if !exclusion {
        prune_orphans(db)?;
    }
    Ok(())
}

pub fn watch_later(db: &Connection) -> Result<Vec<i64>> {
    collect(db, "SELECT video FROM watch_later", [], |row| row.get(0))
}

pub fn set_watch_later(db: &Connection, id: i64, saved: bool) -> Result<bool> {
    if saved {
        db.execute(
            &format!(
                "INSERT OR IGNORE INTO watch_later(video,saved_at)
                      SELECT v.id,?1 FROM videos v WHERE v.id=?2 AND {VISIBLE}"
            ),
            params![crate::model::now(), id],
        )?;
    } else {
        db.execute("DELETE FROM watch_later WHERE video=?", [id])?;
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM watch_later WHERE video=?)",
        [id],
        |row| row.get(0),
    )?)
}

pub fn collections(db: &Connection) -> Result<Vec<Collection>> {
    collect(
        db,
        "SELECT id,name FROM collections ORDER BY name COLLATE NOCASE",
        [],
        |row| {
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
}

fn new_collection(db: &Connection, name: &str) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        bail!("Use a collection name between 1 and 80 characters");
    }
    if db.execute("INSERT OR IGNORE INTO collections(name) VALUES(?)", [name])? == 0 {
        bail!("A collection with this name already exists");
    }
    Ok(db.last_insert_rowid())
}

pub fn add_to_collection(
    db: &mut Connection,
    target: CollectionTarget,
    videos: &[i64],
) -> Result<usize> {
    let tx = db.transaction()?;
    let id = match target {
        CollectionTarget::New(name) => new_collection(&tx, &name)?,
        CollectionTarget::Existing(id) => id,
    };
    let mut added = 0;
    {
        let mut insert = tx.prepare_cached(
            "INSERT OR IGNORE INTO collection_videos(collection,video)
             SELECT ?1,id FROM videos WHERE id=?2 AND probe=1 AND (pinned=1 OR indexed=1)",
        )?;
        for video in videos {
            added += insert.execute(params![id, video])?;
        }
    }
    tx.commit()?;
    Ok(added)
}

pub fn memberships(db: &Connection, id: i64) -> Result<Vec<i64>> {
    collect(
        db,
        "SELECT collection FROM collection_videos WHERE video=?",
        [id],
        |row| row.get(0),
    )
}

pub fn set_membership(db: &Connection, id: i64, collection: i64, included: bool) -> Result<()> {
    let sql = if included {
        "INSERT OR IGNORE INTO collection_videos(video,collection) VALUES(?,?)"
    } else {
        "DELETE FROM collection_videos WHERE video=? AND collection=?"
    };
    db.execute(sql, [id, collection])?;
    Ok(())
}

fn collect<T, P: rusqlite::Params>(
    db: &Connection,
    sql: &str,
    parameters: P,
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = db.prepare_cached(sql)?;
    Ok(stmt
        .query_map(parameters, map)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn video(row: &Row<'_>) -> rusqlite::Result<Video> {
    Ok(Video {
        id: row.get(0)?,
        rev: row.get(1)?,
        path: from_key(row.get_ref(2)?.as_blob()?),
        title: row.get(3)?,
        ext: row.get(4)?,
        stamp: Stamp {
            bytes: row.get(5)?,
            modified: row.get(6)?,
        },
        added: row.get(7)?,
        created: row.get(14)?,
        media: Media {
            duration: row.get(8)?,
            width: row.get(9)?,
            height: row.get(10)?,
            codec: row.get(11)?,
            audio: row.get(12)?,
            fps: row.get(13)?,
        },
    })
}

pub fn get(db: &Connection, id: i64) -> Result<Option<Video>> {
    Ok(db
        .query_row(
            &format!("SELECT {VIDEO_FIELDS} FROM videos v WHERE id=?"),
            [id],
            video,
        )
        .optional()?)
}

pub fn playback_history(db: &Connection) -> Result<Vec<Video>> {
    collect(
        db,
        &format!(
            "SELECT {VIDEO_FIELDS} FROM playback_history h JOIN videos v ON v.id=h.video WHERE {VISIBLE} ORDER BY h.opened DESC,h.video LIMIT 256"
        ),
        [],
        video,
    )
}

pub fn export_playlist(db: &Connection, filter: &Filter, destination: &Path) -> Result<()> {
    use anyhow::Context as _;
    use std::io::Write;
    let parent = destination
        .parent()
        .context("No playlist destination folder")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let (condition, args, order) = video_query(filter);
    let mut query = db.prepare(&format!(
        "SELECT v.path FROM videos v {condition} ORDER BY {order}"
    ))?;
    let mut rows = query.query(params_from_iter(args))?;
    writeln!(file, "#EXTM3U")?;
    while let Some(row) = rows.next()? {
        let path = from_key(row.get_ref(0)?.as_blob()?);
        let path = path.to_str().context("M3U8 requires a UTF-8 file path")?;
        anyhow::ensure!(
            !path.contains(['\r', '\n']),
            "A file name contains a line break that M3U8 cannot represent"
        );
        writeln!(file, "{path}")?;
    }
    file.persist(destination)?;
    Ok(())
}

pub fn get_visible(db: &Connection, id: i64) -> Result<Option<Video>> {
    Ok(db
        .prepare_cached(&format!(
            "SELECT {VIDEO_FIELDS} FROM videos v WHERE id=? AND {VISIBLE}"
        ))?
        .query_row([id], video)
        .optional()?)
}

pub fn playback_position(db: &Connection, (id, revision): (i64, i64)) -> Result<Option<f64>> {
    Ok(db
        .query_row(
            "SELECT p.position FROM playback_checkpoints p JOIN videos v ON v.id=p.video
         WHERE p.video=? AND p.revision=? AND v.rev=p.revision",
            params![id, revision],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn save_playback_position(
    db: &Connection,
    (id, revision): (i64, i64),
    position: Option<f64>,
) -> Result<()> {
    if let Some(position) = position.filter(|at| at.is_finite() && *at > 0.0) {
        db.execute(
            "INSERT INTO playback_checkpoints(video,revision,position)
             SELECT id,rev,?3 FROM videos WHERE id=?1 AND rev=?2
             ON CONFLICT(video) DO UPDATE SET revision=excluded.revision,position=excluded.position",
            params![id, revision, position],
        )?;
    } else {
        db.execute(
            "DELETE FROM playback_checkpoints WHERE video=? AND revision=?",
            params![id, revision],
        )?;
    }
    Ok(())
}

fn video_query(filter: &Filter) -> (String, Vec<Value>, &'static str) {
    let mut condition = format!("WHERE {VISIBLE}");
    let mut args = Vec::<Value>::new();
    if filter.watch_later {
        condition.push_str(" AND v.id IN (SELECT video FROM watch_later)");
    }
    if let Some(id) = filter.collection {
        condition.push_str(" AND v.id IN (SELECT video FROM collection_videos WHERE collection=?)");
        args.push(id.into());
    }
    if let Some(id) = filter.source {
        condition.push_str(" AND v.id IN (SELECT video FROM sightings WHERE source=?)");
        args.push(id.into());
    }
    if let Some(directory) = &filter.directory {
        let prefix = directory_prefix(directory);
        if let Some(upper) = prefix_end(&prefix) {
            condition.push_str(" AND v.path>=? AND v.path<?");
            args.push(prefix.into());
            args.push(upper.into());
        }
    }
    let mut indexed_terms = Vec::new();
    let search = filter.text.trim();
    #[cfg(windows)]
    let normalized = search.replace('/', std::path::MAIN_SEPARATOR_STR);
    #[cfg(windows)]
    let search = normalized.as_str();
    let absolute = Path::new(search).is_absolute();
    let terms: Vec<&str> = if absolute {
        vec![search]
    } else {
        search.split_whitespace().take(12).collect()
    };
    for term in terms {
        if let Some(term) = search_term(term) {
            indexed_terms.push(term);
        } else {
            condition.push_str(" AND (v.search LIKE ? ESCAPE '\\' OR v.codec LIKE ? ESCAPE '\\')");
            let term = contains(term);
            args.extend([term.clone().into(), term.into()]);
        }
    }
    for (column, value) in [
        ("filename", &filter.filename),
        ("folder", &filter.folder),
        ("codec", &filter.codec),
    ] {
        if !value.trim().is_empty() {
            condition.push_str(&format!(" AND v.{column} LIKE ? ESCAPE '\\'"));
            args.push(contains(value.trim()).into());
            indexed_terms.extend(search_term(value.trim()));
        }
    }
    if !indexed_terms.is_empty() {
        condition
            .push_str(" AND v.id IN (SELECT rowid FROM video_search WHERE video_search MATCH ?)");
        args.push(indexed_terms.join(" AND ").into());
    }
    if !filter.ext.trim().is_empty() {
        condition.push_str(" AND v.ext=?");
        args.push(
            filter
                .ext
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase()
                .into(),
        );
    }
    for (clause, value) in [
        ("duration>=?", filter.min_seconds),
        ("duration<?", filter.max_seconds),
        ("min(v.width,v.height)>=?", filter.min_height),
        ("min(v.width,v.height)<?", filter.max_height),
        ("added>=?", filter.added_since),
        ("v.bytes>=?", filter.min_bytes),
    ] {
        if let Some(value) = value {
            condition.push_str(" AND ");
            condition.push_str(clause);
            args.push(value.into());
        }
    }
    for ((column, nanos), date) in [("added", false), ("created", true), ("modified", true)]
        .into_iter()
        .zip(filter.dates)
    {
        let (from, until) = date.bounds(nanos);
        for (operator, bound) in [(">=", from), ("<", until)] {
            if let Some(bound) = bound {
                condition.push_str(&format!(" AND v.{column}{operator}?"));
                args.push(bound.into());
            }
        }
    }
    let order = match filter.sort {
        Sort::Recent if filter.watch_later => {
            "(SELECT saved_at FROM watch_later WHERE video=v.id) DESC,v.id DESC"
        }
        Sort::Recent => "v.added DESC,v.id DESC",
        Sort::AddedOldest => "v.added ASC,v.id ASC",
        Sort::CreatedNewest => "v.created DESC,v.id DESC",
        Sort::CreatedOldest => "v.created ASC NULLS LAST,v.id ASC",
        Sort::ModifiedNewest => "v.modified DESC,v.id DESC",
        Sort::ModifiedOldest => "v.modified ASC,v.id ASC",
        Sort::Title => "substr(v.filename,1,length(v.filename)-length(v.ext)-1) ASC,v.id ASC",
        Sort::Duration => "v.duration DESC,v.id DESC",
        Sort::Size => "v.bytes DESC,v.id DESC",
        Sort::Smallest => "v.bytes ASC,v.id ASC",
    };
    (condition, args, order)
}

pub fn range_ids(db: &Connection, filter: &Filter, first: i64, last: i64) -> Result<Vec<i64>> {
    let (condition, args, order) = video_query(filter);
    let endpoints = [first, last];
    let mut stmt = db.prepare_cached(&format!(
        "SELECT v.id FROM videos v {condition} ORDER BY {order}"
    ))?;
    let mut rows = stmt.query(params_from_iter(args))?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        if endpoints.contains(&id) {
            let complete = first == last || !found.is_empty();
            found.push(id);
            if complete {
                return Ok(found);
            }
        } else if !found.is_empty() {
            found.push(id);
        }
    }
    bail!("The selection changed. Select the range again.")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRegion {
    pub columns: usize,
    pub rows: std::ops::Range<usize>,
    pub cells: std::ops::Range<usize>,
}

impl CatalogRegion {
    pub fn contains(&self, index: usize) -> bool {
        self.rows.contains(&(index / self.columns)) && self.cells.contains(&(index % self.columns))
    }
}

pub fn region_ids(db: &Connection, filter: &Filter, region: CatalogRegion) -> Result<Vec<i64>> {
    let (condition, mut args, order) = video_query(filter);
    let first = region.rows.start * region.columns;
    let count = region.rows.len() * region.columns;
    args.extend([Value::from(count as i64), Value::from(first as i64)]);
    let mut stmt = db.prepare_cached(&format!(
        "SELECT v.id FROM videos v {condition} ORDER BY {order} LIMIT ? OFFSET ?"
    ))?;
    let ids = stmt.query_map(params_from_iter(args), |row| row.get(0))?;
    Ok(ids
        .enumerate()
        .filter(|(offset, _)| region.contains(first + offset))
        .map(|(_, id)| id)
        .collect::<rusqlite::Result<_>>()?)
}

pub fn ids_at_positions(
    db: &Connection,
    filter: &Filter,
    mut positions: Vec<usize>,
) -> Result<Vec<i64>> {
    positions.sort_unstable();
    positions.dedup();
    let (condition, mut args, order) = video_query(filter);
    let mut stmt = db.prepare_cached(&format!(
        "SELECT v.id FROM videos v {condition} ORDER BY {order} LIMIT ? OFFSET ?"
    ))?;
    let arg_count = args.len();
    let mut ids = Vec::with_capacity(positions.len());
    let mut start = 0;
    while start < positions.len() {
        let first = positions[start];
        let mut end = start + 1;
        while end < positions.len() && positions[end] - first == end - start {
            end += 1;
        }
        args.truncate(arg_count);
        args.extend([Value::from((end - start) as i64), Value::from(first as i64)]);
        let rows = stmt.query_map(params_from_iter(&args), |row| row.get(0))?;
        for id in rows {
            ids.push(id?);
        }
        start = end;
    }
    Ok(ids)
}

pub fn selection_files(db: &mut Connection, ids: Vec<i64>) -> Result<Vec<Video>> {
    let tx = db.transaction()?;
    let mut videos = Vec::with_capacity(ids.len());
    for id in ids {
        videos.push(get_visible(&tx, id)?.ok_or_else(|| {
            anyhow::anyhow!("A selected video left the library. Select the videos again.")
        })?);
    }
    tx.commit()?;
    videos.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(videos)
}

#[cfg(test)]
pub fn page(
    db: &mut Connection,
    filter: &Filter,
    index: usize,
    count: bool,
) -> Result<(Option<usize>, Vec<Video>)> {
    let page = search_page(db, filter, index, count, None)?;
    Ok((page.total, page.rows))
}

pub struct SearchPage {
    pub index: usize,
    pub total: Option<usize>,
    pub rows: Vec<Video>,
    pub anchor_position: Option<usize>,
}

pub fn search_page(
    db: &mut Connection,
    filter: &Filter,
    mut index: usize,
    count: bool,
    anchor: Option<i64>,
) -> Result<SearchPage> {
    let (condition, mut args, order) = video_query(filter);
    let transaction = db.transaction()?;
    let total = if count || anchor.is_some() {
        Some(
            transaction
                .prepare_cached(&format!("SELECT count(*) FROM videos v {condition}"))?
                .query_row(params_from_iter(&args), |row| row.get::<_, i64>(0))?
                as usize,
        )
    } else {
        None
    };
    let mut anchor_position = None;
    if let Some(anchor) = anchor {
        let mut stmt = transaction.prepare_cached(&format!(
            "SELECT v.id FROM videos v {condition} ORDER BY {order}"
        ))?;
        let ids = stmt.query_map(params_from_iter(&args), |row| row.get::<_, i64>(0))?;
        for (position, id) in ids.enumerate() {
            if id? == anchor {
                anchor_position = Some(position);
                break;
            }
        }
        index = anchor_position.map_or_else(
            || index.min(total.unwrap_or_default().saturating_sub(1) / PAGE),
            |position| position / PAGE,
        );
    }
    args.push((PAGE as i64).into());
    args.push((index.saturating_mul(PAGE).min(i64::MAX as usize) as i64).into());
    let rows = collect(
        &transaction,
        &format!(
            "SELECT {VIDEO_FIELDS} FROM videos v {condition} ORDER BY {order} LIMIT ? OFFSET ?"
        ),
        params_from_iter(args),
        video,
    )?;
    transaction.commit()?;
    Ok(SearchPage {
        index,
        total,
        rows,
        anchor_position,
    })
}

fn directory_prefix(directory: &Path) -> Vec<u8> {
    // Joining an empty component produces the native separator without
    // changing the original path bytes, including non-UTF-8 names.
    path_key(&directory.join(""))
}

fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    let last = prefix.iter().rposition(|&byte| byte != u8::MAX)?;
    let mut end = prefix[..=last].to_vec();
    end[last] += 1;
    Some(end)
}

fn contains(term: &str) -> String {
    format!(
        "%{}%",
        term.to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

fn search_term(term: &str) -> Option<String> {
    (term.chars().take(3).count() == 3)
        .then(|| format!("\"{}\"", term.to_lowercase().replace('"', "\"\"")))
}

pub fn known_file(db: &Connection, key: &[u8]) -> Result<Option<(i64, Stamp, Option<i64>)>> {
    Ok(db
        .prepare_cached("SELECT id,bytes,modified,created FROM videos WHERE path=?")?
        .query_row([key], |row| {
            Ok((
                row.get(0)?,
                Stamp {
                    bytes: row.get(1)?,
                    modified: row.get(2)?,
                },
                row.get(3)?,
            ))
        })
        .optional()?)
}

pub fn upsert(
    db: &Connection,
    path: &Path,
    ext: &str,
    stamp: Stamp,
    created: Option<i64>,
    added: i64,
) -> Result<(i64, bool)> {
    let key = path_key(path);
    if let Some((id, recorded, previous_creation)) = known_file(db, &key)? {
        let changed = recorded != stamp;
        if changed {
            db.execute(
                "UPDATE videos SET bytes=?,modified=?,created=?,rev=rev+1,probe=0,duration=NULL,width=NULL,height=NULL,codec=NULL,audio=NULL,fps=NULL WHERE id=?",
                params![stamp.bytes, stamp.modified, created, id],
            )?;
            db.execute("DELETE FROM thumbs WHERE video=?", [id])?;
        }
        if !changed && previous_creation != created {
            db.execute(
                "UPDATE videos SET created=? WHERE id=?",
                params![created, id],
            )?;
        }
        return Ok((id, changed || previous_creation != created));
    }
    let (title, filename, folder, search) = path_fields(path);
    let id = db.prepare_cached(
        "INSERT INTO videos(path,title,filename,folder,search,ext,bytes,modified,created,added) VALUES(?,?,?,?,?,?,?,?,?,?) RETURNING id",
    )?.query_row(
        params![key, title, filename, folder, search, ext, stamp.bytes, stamp.modified, created, added], |row| row.get(0),
    )?;
    Ok((id, true))
}

pub fn path_fields(path: &Path) -> (String, String, String, String) {
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let folder = path
        .parent()
        .map_or_else(String::new, |dir| dir.to_string_lossy().to_lowercase());
    let search = path.to_string_lossy().to_lowercase();
    (title, filename, folder, search)
}

pub fn sight(db: &Connection, source: i64, id: i64, generation: i64) -> Result<()> {
    db.prepare_cached("INSERT INTO sightings(source,video,generation) VALUES(?,?,?)         ON CONFLICT(source,video) DO UPDATE SET generation=excluded.generation")?
        .execute(params![source, id, generation])?;
    Ok(())
}

pub fn reconcile(
    db: &mut Connection,
    source: i64,
    generation: i64,
    protected: &[std::path::PathBuf],
) -> Result<()> {
    let tx = db.transaction()?;
    if protected.is_empty() {
        tx.execute(
            "DELETE FROM sightings WHERE source=? AND generation<>?",
            params![source, generation],
        )?;
    } else {
        let stale = collect(
            &tx,
            "SELECT v.id,v.path FROM sightings s JOIN videos v ON v.id=s.video WHERE s.source=? AND s.generation<>?",
            params![source, generation],
            |row| Ok((row.get::<_, i64>(0)?, from_key(row.get_ref(1)?.as_blob()?))),
        )?;
        let mut delete = tx.prepare_cached("DELETE FROM sightings WHERE source=? AND video=?")?;
        for (id, path) in stale {
            if !protected.iter().any(|dir| path.starts_with(dir)) {
                delete.execute(params![source, id])?;
            }
        }
    }
    prune_orphans(&tx)?;
    tx.execute(
        "INSERT INTO source_scan_history(source,completed) VALUES(?,?)
         ON CONFLICT(source) DO UPDATE SET completed=excluded.completed",
        params![source, crate::model::now()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn prune_orphans(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM videos AS v WHERE pinned=0 AND         indexed=0 AND         NOT EXISTS(SELECT 1 FROM collection_videos c WHERE c.video=v.id) AND NOT EXISTS(SELECT 1 FROM watch_later w WHERE w.video=v.id)", [])?;
    Ok(())
}

pub fn presence_batch(db: &Connection, after: i64) -> Result<Vec<Video>> {
    collect(
        db,
        &format!(
            "SELECT {VIDEO_FIELDS} FROM videos v WHERE v.id>? AND {VISIBLE} ORDER BY v.id LIMIT {PAGE}"
        ),
        [after],
        video,
    )
}

pub fn remove_missing(db: &rusqlite::Transaction<'_>, video: &Video) -> Result<bool> {
    let changed = db.execute(
        "UPDATE videos SET pinned=0 WHERE id=? AND rev=? AND path=? AND (pinned=1 OR indexed=1)",
        params![video.id, video.rev, path_key(&video.path)],
    )?;
    if changed != 0 {
        db.execute("DELETE FROM sightings WHERE video=?", [video.id])?;
        db.execute(
            "DELETE FROM videos WHERE id=? AND NOT EXISTS(SELECT 1 FROM collection_videos WHERE video=?)",
            [video.id, video.id],
        )?;
    }
    Ok(changed != 0)
}

pub fn pending(db: &Connection, after: i64) -> Result<Vec<Video>> {
    collect(
        db,
        &format!(
            "SELECT {VIDEO_FIELDS} FROM videos v WHERE v.probe=0 AND v.id>? AND {IN_LIBRARY} ORDER BY v.id LIMIT 20"
        ),
        [after],
        video,
    )
}

pub fn set_media(db: &Connection, video: &Video, media: &Media) -> Result<()> {
    db.prepare_cached("UPDATE videos SET probe=1,duration=?,width=?,height=?,codec=?,audio=?,fps=? WHERE id=? AND rev=? AND probe=0")?
        .execute(params![media.duration,media.width,media.height,media.codec,media.audio,media.fps,video.id,video.rev])?;
    Ok(())
}

pub fn reject_media(db: &Connection, video: &Video) -> Result<()> {
    db.prepare_cached("UPDATE videos SET probe=2 WHERE id=? AND rev=? AND probe=0")?
        .execute(params![video.id, video.rev])?;
    Ok(())
}

pub fn path_probe(db: &Connection, path: &Path) -> Result<Option<i64>> {
    Ok(db
        .prepare_cached("SELECT probe FROM videos WHERE path=?")?
        .query_row([path_key(path)], |row| row.get(0))
        .optional()?)
}

pub fn next_generation(db: &Connection) -> Result<i64> {
    Ok(db.query_row(
        "UPDATE state SET generation=generation+1 RETURNING generation",
        [],
        |row| row.get(0),
    )?)
}
