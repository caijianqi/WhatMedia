-- 0001_init.sql —— 全表骨架
--
-- 设计原则（见 DEVELOPMENT-PLAN.md §六）：
--   P0 就把所有表建好，包括 P9 才会用到的多用户 / 插件 / 审计表。
--   理由是避免后期改表引发迁移地狱 —— 空表不花钱，改表很花钱。
--
-- 兼容性：SQLite 与 PostgreSQL 共用一份 SQL，因此
--   · 主键统一用 TEXT（存 UUID 字符串），不用 UUID 类型
--   · 时间统一用 TEXT（ISO-8601 UTC）
--   · 布尔统一用 INTEGER (0/1)

PRAGMA foreign_keys = ON;

-- ───────────────── 媒体库 ─────────────────

CREATE TABLE IF NOT EXISTS library (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    kind              TEXT NOT NULL,
    storage_provider  TEXT NOT NULL,
    root_path         TEXT NOT NULL,
    scan_policy       TEXT NOT NULL DEFAULT 'incremental',
    metadata_providers TEXT NOT NULL DEFAULT '[]',
    created_at        TEXT NOT NULL
);

-- ───────────────── 媒体资产（四层模型） ─────────────────

CREATE TABLE IF NOT EXISTS media_item (
    id              TEXT PRIMARY KEY,
    library_id      TEXT NOT NULL REFERENCES library(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    title           TEXT NOT NULL,
    original_title  TEXT,
    sort_title      TEXT,
    year            INTEGER,
    overview        TEXT,
    runtime_ms      INTEGER,
    rating          REAL,
    -- 字段级锁定（§32）：记录用户手动改过的字段，刮削不得覆盖
    locked_fields   TEXT NOT NULL DEFAULT '[]',
    provider        TEXT,
    provider_id     TEXT,
    added_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_media_item_library_kind ON media_item(library_id, kind);
CREATE INDEX IF NOT EXISTS idx_media_item_provider ON media_item(provider, provider_id);
CREATE INDEX IF NOT EXISTS idx_media_item_added ON media_item(added_at DESC);

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
    -- 增量扫描指纹 = (mtime_ns, size)，两者缺一不可
    file_fingerprint TEXT NOT NULL,
    -- ffprobe 原始输出，避免重复探测（ARM 上 ffprobe 很贵）
    probe_json       TEXT,
    is_available     INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_media_file_path ON media_file(storage_provider, path);
CREATE INDEX IF NOT EXISTS idx_media_file_fingerprint ON media_file(file_fingerprint);

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
    year           INTEGER,
    overview       TEXT
);

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

-- ───────────────── 图片 / 人物 ─────────────────

CREATE TABLE IF NOT EXISTS image (
    id          TEXT PRIMARY KEY,
    item_id     TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    url         TEXT,
    width       INTEGER,
    height      INTEGER,
    cached_path TEXT
);
CREATE INDEX IF NOT EXISTS idx_image_item ON image(item_id, kind);

CREATE TABLE IF NOT EXISTS person (
    id    TEXT PRIMARY KEY,
    name  TEXT NOT NULL,
    image TEXT
);

CREATE TABLE IF NOT EXISTS credit (
    person_id   TEXT NOT NULL REFERENCES person(id) ON DELETE CASCADE,
    item_id     TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    character   TEXT,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (person_id, item_id, role)
);
CREATE INDEX IF NOT EXISTS idx_credit_item ON credit(item_id);

-- ───────────────── 播放 / 转码 ─────────────────

CREATE TABLE IF NOT EXISTS playback_session (
    id          TEXT PRIMARY KEY,
    user_id     TEXT,
    item_id     TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    file_id     TEXT NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    device_id   TEXT,
    strategy    TEXT NOT NULL,
    position_ms INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_user ON playback_session(user_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS transcode_job (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    args       TEXT NOT NULL DEFAULT '[]',
    hw_accel   TEXT,
    state      TEXT NOT NULL,
    progress   REAL NOT NULL DEFAULT 0,
    output_dir TEXT,
    pid        INTEGER
);
CREATE INDEX IF NOT EXISTS idx_transcode_state ON transcode_job(state);

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

-- ───────────────── 多用户（P9 启用，P0 建表） ─────────────────

CREATE TABLE IF NOT EXISTS user (
    id           TEXT PRIMARY KEY,
    username     TEXT NOT NULL UNIQUE,
    display_name TEXT,
    password_hash TEXT,
    role         TEXT NOT NULL DEFAULT 'user',
    is_child     INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS device (
    id          TEXT PRIMARY KEY,
    user_id     TEXT REFERENCES user(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    token_hash  TEXT NOT NULL,
    last_seen_at TEXT
);

-- ───────────────── 插件（P9 启用） ─────────────────

CREATE TABLE IF NOT EXISTS plugin (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL UNIQUE,
    version   TEXT NOT NULL,
    enabled   INTEGER NOT NULL DEFAULT 0,
    kind      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_permission (
    plugin_id   TEXT NOT NULL REFERENCES plugin(id) ON DELETE CASCADE,
    permission  TEXT NOT NULL,
    granted     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (plugin_id, permission)
);

-- ───────────────── 审计（P9 启用） ─────────────────

CREATE TABLE IF NOT EXISTS audit_log (
    id         TEXT PRIMARY KEY,
    actor      TEXT,
    action     TEXT NOT NULL,
    target     TEXT,
    trace_id   TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_log(created_at DESC);

-- ───────────────── 全文检索 ─────────────────
-- FTS5 trigram tokenizer：SQLite 无内置中文分词器，trigram 让中文子串可检索。
-- 数据量过 5 万后评估切 Tantivy（见 Spike S4）。

CREATE VIRTUAL TABLE IF NOT EXISTS media_item_fts USING fts5(
    title,
    original_title,
    content='media_item',
    content_rowid='rowid',
    tokenize='trigram'
);
