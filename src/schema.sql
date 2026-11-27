PRAGMA journal_mode=WAL;
PRAGMA auto_vacuum=INCREMENTAL;

CREATE TABLE IF NOT EXISTS state(generation INTEGER NOT NULL);
INSERT INTO state(generation) SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM state);

CREATE TABLE IF NOT EXISTS sources(id INTEGER PRIMARY KEY AUTOINCREMENT,path BLOB NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS exclusions(id INTEGER PRIMARY KEY AUTOINCREMENT,path BLOB NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS library_preferences(id INTEGER PRIMARY KEY CHECK(id=1),scan_drives INTEGER NOT NULL);
INSERT OR IGNORE INTO library_preferences(id,scan_drives) VALUES(1,1);
CREATE TABLE IF NOT EXISTS automatic_drives(
    source INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    online INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS source_scan_history(
    source INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    completed INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS library_view(
    id INTEGER PRIMARY KEY CHECK(id=1),
    list INTEGER NOT NULL,
    size INTEGER NOT NULL
);
INSERT OR IGNORE INTO library_view(id,list,size) VALUES(1,0,1);
CREATE TABLE IF NOT EXISTS appearance(id INTEGER PRIMARY KEY CHECK(id=1),theme TEXT NOT NULL);
INSERT OR IGNORE INTO appearance(id,theme) VALUES(1,'Auto');
CREATE TABLE IF NOT EXISTS player_settings(id INTEGER PRIMARY KEY CHECK(id=1),options TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS player_preferences(
    id INTEGER PRIMARY KEY CHECK(id=1),
    control_bar TEXT NOT NULL,
    progress_bar TEXT NOT NULL DEFAULT 'Follow layout'
);
-- Older catalogs can have a saved layout without a player_settings row.
INSERT OR IGNORE INTO player_settings(id,options) SELECT id,'{}' FROM player_preferences;
INSERT OR IGNORE INTO player_preferences(id,control_bar) VALUES(1,'Floating');
UPDATE player_settings SET options=json_insert(options,'$.interface_theme',
    CASE COALESCE(json_extract(options,'$.liquid_glass'),
                  (SELECT control_bar!='Classic' FROM player_preferences WHERE id=1))
        WHEN 0 THEN 'Classic' ELSE 'LiquidGlass' END)
    WHERE json_type(options,'$.interface_theme') IS NULL;
UPDATE player_settings SET options=json_remove(json_set(options, '$.glass_strength',
    100 - min(100, max(0, CAST(json_extract(options, '$.glass_opacity') AS INTEGER)))), '$.glass_opacity')
    WHERE json_type(options,'$.glass_strength') IS NULL AND json_type(options,'$.glass_opacity')='integer';
CREATE TABLE IF NOT EXISTS videos(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path BLOB NOT NULL UNIQUE,
    title TEXT NOT NULL,
    filename TEXT NOT NULL,
    folder TEXT NOT NULL,
    search TEXT NOT NULL,
    ext TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    added INTEGER NOT NULL,
    created INTEGER,
    rev INTEGER NOT NULL DEFAULT 1,
    pinned INTEGER NOT NULL DEFAULT 0,
    indexed INTEGER NOT NULL DEFAULT 0,
    probe INTEGER NOT NULL DEFAULT 0, -- 0 pending, 1 verified video, 2 unsupported
    duration REAL,width INTEGER,height INTEGER,codec TEXT,audio TEXT,fps REAL
);
CREATE TABLE IF NOT EXISTS watch_later(
    video INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
    saved_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sightings(
    source INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    video INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL,
    PRIMARY KEY(source,video)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sightings_video ON sightings(video);
CREATE TRIGGER IF NOT EXISTS video_first_seen AFTER INSERT ON sightings
BEGIN
    UPDATE videos SET indexed=1 WHERE id=new.video AND indexed=0;
END;
CREATE TRIGGER IF NOT EXISTS video_no_longer_seen AFTER DELETE ON sightings
BEGIN
    UPDATE videos SET indexed=0 WHERE id=old.video AND indexed=1
        AND NOT EXISTS(SELECT 1 FROM sightings WHERE video=old.video);
END;
CREATE INDEX IF NOT EXISTS videos_recent ON videos(added DESC,id DESC) WHERE pinned=1 OR indexed=1;
CREATE INDEX IF NOT EXISTS videos_title_stem ON videos(substr(filename,1,length(filename)-length(ext)-1),id)
    WHERE pinned=1 OR indexed=1;
CREATE INDEX IF NOT EXISTS videos_duration ON videos(duration DESC,id DESC) WHERE pinned=1 OR indexed=1;
CREATE INDEX IF NOT EXISTS videos_size ON videos(bytes DESC,id DESC) WHERE pinned=1 OR indexed=1;
CREATE INDEX IF NOT EXISTS videos_ext ON videos(ext) WHERE pinned=1 OR indexed=1;
CREATE INDEX IF NOT EXISTS videos_pending ON videos(id) WHERE probe=0;

CREATE VIRTUAL TABLE IF NOT EXISTS video_search USING fts5(search,codec,tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS video_search_insert AFTER INSERT ON videos
BEGIN
    INSERT INTO video_search(rowid,search,codec) VALUES(new.id,new.search,new.codec);
END;
CREATE TRIGGER IF NOT EXISTS video_search_update AFTER UPDATE OF search,codec ON videos
WHEN old.search IS NOT new.search OR old.codec IS NOT new.codec
BEGIN
    UPDATE video_search SET search=new.search,codec=new.codec WHERE rowid=new.id;
END;
CREATE TRIGGER IF NOT EXISTS video_search_delete AFTER DELETE ON videos
BEGIN
    DELETE FROM video_search WHERE rowid=old.id;
END;

CREATE TABLE IF NOT EXISTS collections(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL COLLATE NOCASE UNIQUE);
CREATE TABLE IF NOT EXISTS collection_videos(
    collection INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    video INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
    PRIMARY KEY(collection,video)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS collections_video ON collection_videos(video);

CREATE TABLE IF NOT EXISTS playback_history(
    video INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
    opened INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS history_recency ON playback_history(opened DESC,video);

CREATE TABLE IF NOT EXISTS playback_checkpoints(
    video INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    position REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS thumbs(
    video INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    cached_at INTEGER NOT NULL,
    jpeg BLOB
);
CREATE INDEX IF NOT EXISTS thumbs_recency ON thumbs(cached_at DESC,video DESC);
CREATE TABLE IF NOT EXISTS preview_segments(
    video INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    segment INTEGER NOT NULL,
    cached_at INTEGER NOT NULL,
    jpeg BLOB NOT NULL,
    PRIMARY KEY(video,segment)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS preview_recency ON preview_segments(cached_at DESC,video,segment);
CREATE TABLE IF NOT EXISTS timeline_frames(
    video INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    position_ms INTEGER NOT NULL,
    cached_at INTEGER NOT NULL,
    jpeg BLOB NOT NULL,
    PRIMARY KEY(video,revision,position_ms)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS timeline_recency ON timeline_frames(cached_at DESC,video,revision,position_ms);

CREATE TABLE IF NOT EXISTS timeline_budget(
    id INTEGER PRIMARY KEY CHECK(id=1), bytes_used INTEGER NOT NULL, bytes_allowed INTEGER NOT NULL
);
INSERT OR IGNORE INTO timeline_budget(id,bytes_used,bytes_allowed)
SELECT 1,COALESCE(SUM(length(jpeg)),0),16777216 FROM timeline_frames
WHERE NOT EXISTS(SELECT 1 FROM timeline_budget);
CREATE TRIGGER IF NOT EXISTS timeline_budget_insert AFTER INSERT ON timeline_frames BEGIN
    UPDATE timeline_budget SET bytes_used=bytes_used+length(new.jpeg) WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS timeline_budget_update AFTER UPDATE OF jpeg ON timeline_frames BEGIN
    UPDATE timeline_budget SET bytes_used=bytes_used+length(new.jpeg)-length(old.jpeg) WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS timeline_budget_delete AFTER DELETE ON timeline_frames BEGIN
    UPDATE timeline_budget SET bytes_used=bytes_used-length(old.jpeg) WHERE id=1;
END;
