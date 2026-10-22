-- 0009_vod_sources.sql
-- 远程多源视频采集配置与搜索缓存

CREATE TABLE IF NOT EXISTS vod_remote_source (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    api_url        TEXT NOT NULL,
    is_active      INTEGER NOT NULL DEFAULT 1,
    weight         INTEGER NOT NULL DEFAULT 100,
    timeout_ms     INTEGER NOT NULL DEFAULT 2500,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_vod_remote_source_weight ON vod_remote_source(weight);

CREATE TABLE IF NOT EXISTS vod_search_cache (
    query_hash     TEXT PRIMARY KEY,
    response_json  TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    expires_at     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_vod_search_cache_expires ON vod_search_cache(expires_at);
