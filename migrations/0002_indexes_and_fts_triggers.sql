-- 0002 索引与 FTS 触发器
--
-- 0001 建表时故意不建这些，因为它们是为 P1 的查询模式服务的：
--   · media_file(version_id) —— 增量扫描要按库反查全部文件指纹
--   · FTS 触发器             —— 检索依赖 media_item 与 media_item_fts 同步
-- 分开放的好处是：P1 的查询模式一旦变了，改索引不会牵动建表迁移。

-- 增量扫描：library_id → media_item → media_version → media_file 的反向连接
CREATE INDEX IF NOT EXISTS idx_media_file_version ON media_file(version_id);

-- 海报墙按标题排序是最高频操作
CREATE INDEX IF NOT EXISTS idx_media_item_title ON media_item(title);
CREATE INDEX IF NOT EXISTS idx_media_item_year ON media_item(year);

-- ───────────────── FTS 同步触发器 ─────────────────
-- 用 external content 表（content='media_item'），索引里不重复存正文，
-- 代价是每次写 media_item 都要同步维护。

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

-- ───────────────── 幂等 upsert 所需的唯一约束 ─────────────────
-- 没有它们，"两集并发扫描"会创建两条同名 series —— 这是测出来的，不是猜的。
-- 有了唯一索引，并发插入可以用 INSERT ... ON CONFLICT DO NOTHING 收敛到同一行。

CREATE UNIQUE INDEX IF NOT EXISTS uq_series_title_year
    ON series(lower(title), ifnull(year, -1));
