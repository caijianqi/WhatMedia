-- 0003 元数据与图片
--
-- 0001 已经把骨架建好了，这一步补齐 P2 需要的部分：
--   · genre / item_genre      —— 类型与多对多关系
--   · image_cache             —— 图片缓存的 LRU 元数据（正文在磁盘上）
--   · metadata_job            —— 刮削任务队列，支持断点续跑
--   · 若干 ALTER              —— 字段级锁定、provider 溯源、tagline
--
-- 设计要点：**刮削必须是幂等且可续跑的**。
-- 1,000 部电影刮一次要几十分钟，中途崩了从头再来是不可接受的。

-- ───────────────── 类型 ─────────────────

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

-- ───────────────── 图片缓存 ─────────────────
--
-- 正文放磁盘（cache/images/<kind>/<hash>.<ext>），这里只放 LRU 需要的元数据。
-- `key` = sha256(url + 尺寸变体)，所以同一张海报的不同尺寸是不同条目。

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

-- ───────────────── 刮削任务队列 ─────────────────
--
-- 一条 item 一行。`status` 是断点续跑的依据：重启后只重跑
-- `pending` 和 `failed`（且 attempts 未超上限）的，不动 `done`。

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

-- ───────────────── 字段补充 ─────────────────
-- SQLite 的 ALTER TABLE 只支持 ADD COLUMN，且必须带默认值或可空。

ALTER TABLE media_item ADD COLUMN tagline TEXT;

-- 匹配用标题 = **文件名解析出来的原始标题**。
--
-- 没有它就会出现这个死结：用户把标题改成"黑客帝国"并锁定，
-- 之后重扫时拿改名后的标题去 TMDB 搜，当然搜不到，于是再也匹配不上。
-- 这个字段只由扫描器写，用户改名和刮削都不碰它。
ALTER TABLE media_item ADD COLUMN match_title TEXT;
ALTER TABLE series ADD COLUMN match_title TEXT;

-- 剧集同样需要字段级锁定和 provider 溯源：
-- 用户改过剧集名之后，重扫剧集目录不能把它改回去。
ALTER TABLE series ADD COLUMN locked_fields TEXT NOT NULL DEFAULT '[]';
ALTER TABLE series ADD COLUMN provider TEXT;
ALTER TABLE series ADD COLUMN provider_id TEXT;

ALTER TABLE person ADD COLUMN provider TEXT;
ALTER TABLE person ADD COLUMN provider_id TEXT;

-- 图片溯源 + 缓存键。重刮时要能定位到已缓存的那一条，而不是再下一遍。
ALTER TABLE image ADD COLUMN provider TEXT;
ALTER TABLE image ADD COLUMN cache_key TEXT;

-- 同一 item 的同一种图只保留一张，重刮是 UPDATE 不是 INSERT 堆积。
CREATE UNIQUE INDEX IF NOT EXISTS uq_image_item_kind ON image(item_id, kind);
CREATE UNIQUE INDEX IF NOT EXISTS uq_person_provider ON person(provider, provider_id);
