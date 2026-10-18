-- 0005 电视直播（Live TV / IPTV）
--
-- 这一组表是**刻意与 media_item 并列而非挂在它下面**的。
-- 理由见 `docs/proposal/live-tv.md` 原则 1，一句话版：
-- media_item 的语义是"媒体资产"（有版本、文件、时长、刮削、续播、看完），
-- 直播一条都没有。硬塞进去会让扫描、刮削、续播、8 个 AI 工具、FTS 索引
-- 全部为"无文件无时长的实体"加分支，每条分支都是一个可能漏掉的 if。
--
-- 独立建模后 media_item 一个字节都不用改。

-- ───────────────── 直播源 ─────────────────
--
-- 一个 M3U 订阅 / 一台 tuner / 一份纯 EPG 订阅，各是一行。
-- kind 是闭合集合，由 crates/live 的 LiveSourceKind 解析，不在这儿做 CHECK
-- （SQLite 加 CHECK 后要改就得重建表，闭合性交给 Rust 侧保证）。

CREATE TABLE IF NOT EXISTS live_source (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  kind         TEXT NOT NULL,          -- 'm3u' | 'xmltv' | 'hdhomerun' | 'dvb'
  url          TEXT,                   -- kind=m3u 的订阅地址；tuner 可为 NULL
  epg_url      TEXT,                   -- XMLTV 地址（EPG 可与频道源分开）
  enabled      INTEGER NOT NULL DEFAULT 1,
  -- 频道号从这个数开始往下发，避免多个源都从 1 开始撞号
  number_offset INTEGER NOT NULL DEFAULT 0,
  last_sync_at TEXT,
  sync_error   TEXT,
  created_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_live_source_enabled ON live_source(enabled);

-- ───────────────── 频道 ─────────────────
--
-- 刻意不 FK 到 media_item。stream_url 是真正的流地址，可能是
-- http(s) / rtmp / udp 多播，本地文件对它不成立。

CREATE TABLE IF NOT EXISTS live_channel (
  id           TEXT PRIMARY KEY,
  source_id    TEXT NOT NULL REFERENCES live_source(id) ON DELETE CASCADE,
  number       INTEGER,                -- 频道号，排序用
  name         TEXT NOT NULL,
  logo_url     TEXT,
  group_title  TEXT,                   -- M3U 的 group-title：央视 / 卫视 / 体育…
  stream_url   TEXT NOT NULL,
  -- 直播没有 probe 结果，但允许手工标注，或首次拉流后回填
  video_codec  TEXT,
  audio_codec  TEXT,
  -- 时移窗口（L1）。0 = 不支持时移，只给实时边缘。
  dvr_window_secs INTEGER NOT NULL DEFAULT 0,
  enabled      INTEGER NOT NULL DEFAULT 1,
  -- M3U 里的 tvg-id，用于和 XMLTV 的 channel 对应（L2）
  epg_channel_id TEXT,
  sort_order   INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_live_channel_source ON live_channel(source_id, sort_order);
CREATE INDEX IF NOT EXISTS idx_live_channel_number ON live_channel(number);
CREATE INDEX IF NOT EXISTS idx_live_channel_group ON live_channel(group_title);
CREATE INDEX IF NOT EXISTS idx_live_channel_epg ON live_channel(epg_channel_id);

-- ───────────────── 节目单（L2） ─────────────────
--
-- 时间是 ISO8601 字符串，与全库一致。节目单允许重叠与缺失，
-- 所以没有唯一索引 —— XMLTV 本身就不保证干净。

CREATE TABLE IF NOT EXISTS live_program (
  id           TEXT PRIMARY KEY,
  channel_id   TEXT NOT NULL REFERENCES live_channel(id) ON DELETE CASCADE,
  start_at     TEXT NOT NULL,
  end_at       TEXT NOT NULL,
  title        TEXT NOT NULL,
  subtitle     TEXT,
  description  TEXT
);

CREATE INDEX IF NOT EXISTS idx_program_channel_time ON live_program(channel_id, start_at);
CREATE INDEX IF NOT EXISTS idx_program_end ON live_program(channel_id, end_at);

-- ───────────────── 录制（L1） ─────────────────
--
-- 录制是直播里唯一会变成"资产"的东西：录完就是一部有文件、有时长、
-- 可以刮削、可以续播的片子。所以它**是**唯一一个回头指向 media_item 的表 —
-- 方向是对的：录制产物是资产，直播流不是。
-- media_item_id 在录制落盘、入库后才回填；NULL 表示还没转完。

CREATE TABLE IF NOT EXISTS live_recording (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL REFERENCES live_channel(id) ON DELETE CASCADE,
  -- 计划录制：用户点"录"时的意图；实际录制：ffmpeg 已在跑
  status        TEXT NOT NULL,         -- 'scheduled' | 'recording' | 'finished' | 'failed'
  start_at      TEXT NOT NULL,         -- 计划开始
  end_at        TEXT NOT NULL,         -- 计划结束
  -- 实际落盘文件。录制完成后由 library 侧入库，再回填 media_item_id。
  file_path     TEXT,
  media_item_id TEXT,
  error         TEXT,
  created_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_recording_channel ON live_recording(channel_id, start_at);
CREATE INDEX IF NOT EXISTS idx_recording_status ON live_recording(status);

-- ───────────────── 直播会话（在途登记） ─────────────────
--
-- 与 transcode_session 同构：登记"哪个频道现在由哪个进程在拉"。
-- 为什么必须落库而不是只放内存 HashMap：进程崩了、容器重启了，
-- 没人认领的 ffmpeg 会一直挂着拉流，占带宽也占 CPU。
-- ref_count 是"几个客户端在看同一个频道"，归零才停进程。

CREATE TABLE IF NOT EXISTS live_session (
  channel_id   TEXT PRIMARY KEY REFERENCES live_channel(id) ON DELETE CASCADE,
  job_id       TEXT NOT NULL,
  dir          TEXT NOT NULL,
  ref_count    INTEGER NOT NULL DEFAULT 0,
  started_at   TEXT NOT NULL,
  last_access_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_live_session_access ON live_session(last_access_at);
