-- 0001_init.sql —— WhatMedia 全量数据库初始化结构
--
-- 兼容性：
--   · 主键统一用 TEXT（存 UUID 字符串）
--   · 时间统一用 TEXT（ISO-8601 UTC）
--   · 布尔统一用 INTEGER (0/1)

PRAGMA foreign_keys = ON;

-- ───────────────── 媒体库 ─────────────────

CREATE TABLE IF NOT EXISTS library (
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL,
    kind               TEXT NOT NULL,
    storage_provider   TEXT NOT NULL,
    root_path          TEXT NOT NULL,
    scan_policy        TEXT NOT NULL DEFAULT 'incremental',
    metadata_providers TEXT NOT NULL DEFAULT '[]',
    created_at         TEXT NOT NULL
);

-- ───────────────── 媒体资产（四层模型） ─────────────────

CREATE TABLE IF NOT EXISTS media_item (
    id              TEXT PRIMARY KEY,
    library_id      TEXT NOT NULL REFERENCES library(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    title           TEXT NOT NULL,
    original_title  TEXT,
    sort_title      TEXT,
    match_title     TEXT,
    tagline         TEXT,
    year            INTEGER,
    overview        TEXT,
    runtime_ms      INTEGER,
    rating          REAL,
    locked_fields   TEXT NOT NULL DEFAULT '[]',
    provider        TEXT,
    provider_id     TEXT,
    added_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_media_item_library_kind ON media_item(library_id, kind);
CREATE INDEX IF NOT EXISTS idx_media_item_provider ON media_item(provider, provider_id);
CREATE INDEX IF NOT EXISTS idx_media_item_added ON media_item(added_at DESC);
CREATE INDEX IF NOT EXISTS idx_media_item_title ON media_item(title);
CREATE INDEX IF NOT EXISTS idx_media_item_year ON media_item(year);

CREATE TABLE IF NOT EXISTS media_version (
    id           TEXT PRIMARY KEY,
    item_id      TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    source       TEXT,
    resolution   TEXT,
    video_codec  TEXT,
    hdr_type     TEXT NOT NULL DEFAULT 'sdr',
    is_default   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_media_version_item ON media_version(item_id);

CREATE TABLE IF NOT EXISTS media_file (
    id               TEXT PRIMARY KEY,
    version_id       TEXT NOT NULL REFERENCES media_version(id) ON DELETE CASCADE,
    storage_provider TEXT NOT NULL,
    path             TEXT NOT NULL,
    container        TEXT,
    size_bytes       INTEGER NOT NULL DEFAULT 0,
    duration_ms      INTEGER,
    bitrate          INTEGER,
    file_fingerprint TEXT NOT NULL,
    probe_json       TEXT,
    is_available     INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_media_file_path ON media_file(storage_provider, path);
CREATE INDEX IF NOT EXISTS idx_media_file_fingerprint ON media_file(file_fingerprint);
CREATE INDEX IF NOT EXISTS idx_media_file_version ON media_file(version_id);

CREATE TABLE IF NOT EXISTS media_stream (
    id          TEXT PRIMARY KEY,
    file_id     TEXT NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    stream_type TEXT NOT NULL,
    idx         INTEGER NOT NULL,
    codec       TEXT,
    language    TEXT,
    title       TEXT,
    is_default  INTEGER NOT NULL DEFAULT 0,
    is_forced   INTEGER NOT NULL DEFAULT 0,
    width       INTEGER,
    height      INTEGER,
    bit_depth   INTEGER,
    hdr_type    TEXT NOT NULL DEFAULT 'sdr',
    channels    INTEGER,
    bitrate     INTEGER
);
CREATE INDEX IF NOT EXISTS idx_media_stream_file ON media_stream(file_id, stream_type);

-- ───────────────── 电视剧结构 ─────────────────

CREATE TABLE IF NOT EXISTS series (
    id             TEXT PRIMARY KEY,
    title          TEXT NOT NULL,
    original_title TEXT,
    match_title    TEXT,
    year           INTEGER,
    overview       TEXT,
    locked_fields  TEXT NOT NULL DEFAULT '[]',
    provider       TEXT,
    provider_id    TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_series_title_year
    ON series(lower(title), ifnull(year, -1));

CREATE TABLE IF NOT EXISTS season (
    id            TEXT PRIMARY KEY,
    series_id     TEXT NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    title         TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_season ON season(series_id, season_number);

CREATE TABLE IF NOT EXISTS episode (
    id             TEXT PRIMARY KEY,
    season_id      TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    episode_number INTEGER NOT NULL,
    item_id        TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    title          TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_episode ON episode(season_id, episode_number);

-- ───────────────── 类型 / 图片 / 人物 / 刮削 ─────────────────

CREATE TABLE IF NOT EXISTS genre (
    id    TEXT PRIMARY KEY,
    name  TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_genre_name ON genre(lower(name));

CREATE TABLE IF NOT EXISTS item_genre (
    item_id  TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    genre_id TEXT NOT NULL REFERENCES genre(id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, genre_id)
);
CREATE INDEX IF NOT EXISTS idx_item_genre_genre ON item_genre(genre_id);

CREATE TABLE IF NOT EXISTS image (
    id          TEXT PRIMARY KEY,
    item_id     TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    url         TEXT,
    width       INTEGER,
    height      INTEGER,
    cached_path TEXT,
    provider    TEXT,
    cache_key   TEXT
);
CREATE INDEX IF NOT EXISTS idx_image_item ON image(item_id, kind);
CREATE UNIQUE INDEX IF NOT EXISTS uq_image_item_kind ON image(item_id, kind);

CREATE TABLE IF NOT EXISTS image_cache (
    key          TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    url          TEXT NOT NULL,
    width        INTEGER,
    height       INTEGER,
    size_bytes   INTEGER NOT NULL DEFAULT 0,
    path         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    accessed_at  TEXT NOT NULL,
    hits         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_image_cache_kind ON image_cache(kind, accessed_at);
CREATE INDEX IF NOT EXISTS idx_image_cache_accessed ON image_cache(accessed_at);

CREATE TABLE IF NOT EXISTS person (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    image       TEXT,
    provider    TEXT,
    provider_id TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_person_provider ON person(provider, provider_id);

CREATE TABLE IF NOT EXISTS credit (
    person_id   TEXT NOT NULL REFERENCES person(id) ON DELETE CASCADE,
    item_id     TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    character   TEXT,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (person_id, item_id, role)
);
CREATE INDEX IF NOT EXISTS idx_credit_item ON credit(item_id);

CREATE TABLE IF NOT EXISTS metadata_job (
    item_id      TEXT PRIMARY KEY REFERENCES media_item(id) ON DELETE CASCADE,
    library_id   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending',
    attempts     INTEGER NOT NULL DEFAULT 0,
    provider     TEXT,
    provider_id  TEXT,
    last_error   TEXT,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_metadata_job_status ON metadata_job(status, updated_at);
CREATE INDEX IF NOT EXISTS idx_metadata_job_library ON metadata_job(library_id, status);

-- ───────────────── 播放 / 转码 ─────────────────

CREATE TABLE IF NOT EXISTS playback_session (
    id              TEXT PRIMARY KEY,
    user_id         TEXT,
    item_id         TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    file_id         TEXT NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    device_id       TEXT,
    client_id       TEXT,
    strategy        TEXT NOT NULL,
    strategy_reason TEXT,
    position_ms     INTEGER NOT NULL DEFAULT 0,
    duration_ms     INTEGER,
    created_at      TEXT,
    stopped_at      TEXT,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_user ON playback_session(user_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_item ON playback_session(item_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_client ON playback_session(client_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS transcode_job (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    file_id     TEXT,
    output_kind TEXT,
    args        TEXT NOT NULL DEFAULT '[]',
    hw_accel    TEXT,
    state       TEXT NOT NULL,
    progress    REAL NOT NULL DEFAULT 0,
    output_dir  TEXT,
    pid         INTEGER,
    created_at  TEXT,
    started_at  TEXT,
    finished_at TEXT,
    error       TEXT
);
CREATE INDEX IF NOT EXISTS idx_transcode_state ON transcode_job(state);
CREATE INDEX IF NOT EXISTS idx_transcode_file ON transcode_job(file_id);

CREATE TABLE IF NOT EXISTS transcode_session (
    job_id         TEXT PRIMARY KEY,
    file_id        TEXT NOT NULL,
    session_id     TEXT,
    kind           TEXT NOT NULL,
    strategy       TEXT NOT NULL,
    output_dir     TEXT NOT NULL,
    args           TEXT NOT NULL DEFAULT '[]',
    state          TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    last_access_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tsession_state ON transcode_session(state, last_access_at);
CREATE INDEX IF NOT EXISTS idx_tsession_file ON transcode_session(file_id);

-- ───────────────── 电视直播（Live TV / IPTV） ─────────────────

CREATE TABLE IF NOT EXISTS live_source (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    url           TEXT,
    epg_url       TEXT,
    enabled       INTEGER NOT NULL DEFAULT 1,
    number_offset INTEGER NOT NULL DEFAULT 0,
    last_sync_at  TEXT,
    sync_error    TEXT,
    created_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_live_source_enabled ON live_source(enabled);

CREATE TABLE IF NOT EXISTS live_channel (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL REFERENCES live_source(id) ON DELETE CASCADE,
    number          INTEGER,
    name            TEXT NOT NULL,
    logo_url        TEXT,
    group_title     TEXT,
    stream_url      TEXT NOT NULL,
    video_codec     TEXT,
    audio_codec     TEXT,
    dvr_window_secs INTEGER NOT NULL DEFAULT 0,
    enabled         INTEGER NOT NULL DEFAULT 1,
    epg_channel_id  TEXT,
    sort_order      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_live_channel_source ON live_channel(source_id, sort_order);
CREATE INDEX IF NOT EXISTS idx_live_channel_number ON live_channel(number);
CREATE INDEX IF NOT EXISTS idx_live_channel_group ON live_channel(group_title);
CREATE INDEX IF NOT EXISTS idx_live_channel_epg ON live_channel(epg_channel_id);

CREATE TABLE IF NOT EXISTS live_program (
    id          TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL REFERENCES live_channel(id) ON DELETE CASCADE,
    start_at    TEXT NOT NULL,
    end_at      TEXT NOT NULL,
    title       TEXT NOT NULL,
    subtitle    TEXT,
    description TEXT
);
CREATE INDEX IF NOT EXISTS idx_program_channel_time ON live_program(channel_id, start_at);
CREATE INDEX IF NOT EXISTS idx_program_end ON live_program(channel_id, end_at);

CREATE TABLE IF NOT EXISTS live_recording (
    id            TEXT PRIMARY KEY,
    channel_id    TEXT NOT NULL REFERENCES live_channel(id) ON DELETE CASCADE,
    status        TEXT NOT NULL,
    start_at      TEXT NOT NULL,
    end_at        TEXT NOT NULL,
    file_path     TEXT,
    media_item_id TEXT,
    error         TEXT,
    created_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_recording_channel ON live_recording(channel_id, start_at);
CREATE INDEX IF NOT EXISTS idx_recording_status ON live_recording(status);

CREATE TABLE IF NOT EXISTS live_session (
    channel_id     TEXT PRIMARY KEY REFERENCES live_channel(id) ON DELETE CASCADE,
    job_id         TEXT NOT NULL,
    dir            TEXT NOT NULL,
    ref_count      INTEGER NOT NULL DEFAULT 0,
    started_at     TEXT NOT NULL,
    last_access_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_live_session_access ON live_session(last_access_at);

-- ───────────────── 任务 ─────────────────

CREATE TABLE IF NOT EXISTS task (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    priority   TEXT NOT NULL,
    state      TEXT NOT NULL,
    payload    TEXT,
    progress   REAL NOT NULL DEFAULT 0,
    error      TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_state_priority ON task(state, priority);

-- ───────────────── 多用户与鉴权 ─────────────────

CREATE TABLE IF NOT EXISTS user (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    display_name  TEXT,
    password_hash TEXT,
    pin_hash      TEXT,
    role          TEXT NOT NULL DEFAULT 'user',
    is_child      INTEGER NOT NULL DEFAULT 0,
    parent_id     TEXT REFERENCES user(id) ON DELETE SET NULL,
    max_rating    TEXT,
    disabled      INTEGER NOT NULL DEFAULT 0,
    failed_logins INTEGER NOT NULL DEFAULT 0,
    locked_until  TEXT,
    last_login_at TEXT,
    created_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    device_id    TEXT,
    token_hash   TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL DEFAULT 'web',
    ip           TEXT,
    user_agent   TEXT,
    created_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    revoked_at   TEXT
);
CREATE INDEX IF NOT EXISTS idx_session_token ON session(token_hash);
CREATE INDEX IF NOT EXISTS idx_auth_session_user ON session(user_id, revoked_at);
CREATE INDEX IF NOT EXISTS idx_session_expiry ON session(expires_at);

CREATE TABLE IF NOT EXISTS device (
    id           TEXT PRIMARY KEY,
    user_id      TEXT REFERENCES user(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL,
    kind         TEXT,
    platform     TEXT,
    app_version  TEXT,
    last_ip      TEXT,
    approved     INTEGER NOT NULL DEFAULT 1,
    approval_id  TEXT,
    created_at   TEXT,
    last_seen_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_device_user ON device(user_id);

CREATE TABLE IF NOT EXISTS approval_request (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    device_id    TEXT,
    kind         TEXT NOT NULL DEFAULT 'device',
    title        TEXT NOT NULL,
    detail       TEXT,
    status       TEXT NOT NULL DEFAULT 'pending',
    requested_at TEXT NOT NULL,
    decided_at   TEXT,
    decided_by   TEXT REFERENCES user(id) ON DELETE SET NULL,
    note         TEXT
);
CREATE INDEX IF NOT EXISTS idx_approval_user ON approval_request(user_id, status);
CREATE INDEX IF NOT EXISTS idx_approval_pending ON approval_request(status, requested_at DESC);

-- ───────────────── 收藏 / 通知 / 设置 ─────────────────

CREATE TABLE IF NOT EXISTS favorite (
    user_id    TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    item_id    TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, item_id)
);
CREATE INDEX IF NOT EXISTS idx_favorite_item ON favorite(item_id);
CREATE INDEX IF NOT EXISTS idx_favorite_user_time ON favorite(user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS notification (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    level      TEXT NOT NULL DEFAULT 'info',
    title      TEXT NOT NULL,
    body       TEXT,
    link       TEXT,
    created_at TEXT NOT NULL,
    read_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_notification_user ON notification(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_notification_unread ON notification(user_id, read_at);

CREATE TABLE IF NOT EXISTS setting (
    scope      TEXT NOT NULL,
    scope_id   TEXT NOT NULL,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    updated_by TEXT,
    PRIMARY KEY (scope, scope_id, key)
);

-- ───────────────── 存储源与多源聚合 ─────────────────

CREATE TABLE IF NOT EXISTS storage_account (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    driver        TEXT NOT NULL,
    endpoint      TEXT NOT NULL,
    auth_type     TEXT NOT NULL DEFAULT 'none',
    username      TEXT,
    password      TEXT,
    root_path     TEXT NOT NULL DEFAULT '/',
    direct_stream INTEGER NOT NULL DEFAULT 1,
    extra_headers TEXT NOT NULL DEFAULT '{}',
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_storage_account_driver ON storage_account(driver);

CREATE TABLE IF NOT EXISTS vod_remote_source (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    api_url    TEXT NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 1,
    weight     INTEGER NOT NULL DEFAULT 100,
    timeout_ms INTEGER NOT NULL DEFAULT 2500,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_vod_remote_source_weight ON vod_remote_source(weight);

CREATE TABLE IF NOT EXISTS vod_search_cache (
    query_hash    TEXT PRIMARY KEY,
    response_json TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    expires_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_vod_search_cache_expires ON vod_search_cache(expires_at);

-- ───────────────── 插件与审计 ─────────────────

CREATE TABLE IF NOT EXISTS plugin (
    id      TEXT PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    kind    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_permission (
    plugin_id  TEXT NOT NULL REFERENCES plugin(id) ON DELETE CASCADE,
    permission TEXT NOT NULL,
    granted    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (plugin_id, permission)
);

CREATE TABLE IF NOT EXISTS audit_log (
    id         TEXT PRIMARY KEY,
    actor      TEXT,
    action     TEXT NOT NULL,
    target     TEXT,
    trace_id   TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_log(created_at DESC);

-- ───────────────── 全文检索与同步触发器 ─────────────────

CREATE VIRTUAL TABLE IF NOT EXISTS media_item_fts USING fts5(
    title,
    original_title,
    content='media_item',
    content_rowid='rowid',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS media_item_fts_ai AFTER INSERT ON media_item BEGIN
    INSERT INTO media_item_fts(rowid, title, original_title)
    VALUES (new.rowid, new.title, new.original_title);
END;

CREATE TRIGGER IF NOT EXISTS media_item_fts_ad AFTER DELETE ON media_item BEGIN
    INSERT INTO media_item_fts(media_item_fts, rowid, title, original_title)
    VALUES ('delete', old.rowid, old.title, old.original_title);
END;

CREATE TRIGGER IF NOT EXISTS media_item_fts_au AFTER UPDATE ON media_item BEGIN
    INSERT INTO media_item_fts(media_item_fts, rowid, title, original_title)
    VALUES ('delete', old.rowid, old.title, old.original_title);
    INSERT INTO media_item_fts(rowid, title, original_title)
    VALUES (new.rowid, new.title, new.original_title);
END;
