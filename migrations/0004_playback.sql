-- 0004 播放与转码
--
-- 0001 已经建了 playback_session / transcode_job 的骨架，这一步补齐 P3 真正需要的部分：
--   · playback_session  —— 客户端标识、起止时间、续播点索引
--   · transcode_job     —— 归属文件、输出形态、时间戳、错误信息
--   · transcode_session —— 流式输出（remux / HLS）的在途登记
--
-- 第三条是关键。**流式输出目录必须落库**：
-- 进程崩了、容器重启了、用户关掉播放器了，都得有据可查地把临时分片删掉。
-- 只靠"内存里的 HashMap"会导致 /cache 里堆积一堆没人认领的 .ts。

-- ───────────────── 播放会话 ─────────────────

ALTER TABLE playback_session ADD COLUMN client_id TEXT;
ALTER TABLE playback_session ADD COLUMN created_at TEXT;
ALTER TABLE playback_session ADD COLUMN stopped_at TEXT;
ALTER TABLE playback_session ADD COLUMN strategy_reason TEXT;

CREATE INDEX IF NOT EXISTS idx_session_item ON playback_session(item_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_client ON playback_session(client_id, updated_at DESC);

-- 续播点：同一 (user, item) 只保留最新的那条，靠 updated_at 倒序取第一条即可。
-- 不建唯一索引是因为"同一部片在两个设备上各看一段"是合理的，
-- 强行唯一会把另一台的进度覆盖掉。

-- ───────────────── 转码任务 ─────────────────

ALTER TABLE transcode_job ADD COLUMN file_id TEXT;
ALTER TABLE transcode_job ADD COLUMN output_kind TEXT;
ALTER TABLE transcode_job ADD COLUMN created_at TEXT;
ALTER TABLE transcode_job ADD COLUMN started_at TEXT;
ALTER TABLE transcode_job ADD COLUMN finished_at TEXT;
ALTER TABLE transcode_job ADD COLUMN error TEXT;

CREATE INDEX IF NOT EXISTS idx_transcode_file ON transcode_job(file_id);

-- ───────────────── 流式输出登记 ─────────────────
--
-- remux 直出与 HLS 都会往磁盘写临时数据。这里登记"谁在往哪写"，
-- 既有 `last_access_at` 供回收器判断僵尸，也能在重启后清理残留目录。

CREATE TABLE IF NOT EXISTS transcode_session (
    job_id         TEXT PRIMARY KEY,
    file_id        TEXT NOT NULL,
    session_id     TEXT,
    kind           TEXT NOT NULL,          -- remux_ts | remux_fmp4 | hls
    strategy       TEXT NOT NULL,          -- remux | transcode
    output_dir     TEXT NOT NULL,
    args           TEXT NOT NULL DEFAULT '[]',
    state          TEXT NOT NULL,          -- running | finished | failed | cancelled
    created_at     TEXT NOT NULL,
    last_access_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tsession_state ON transcode_session(state, last_access_at);
CREATE INDEX IF NOT EXISTS idx_tsession_file ON transcode_session(file_id);
