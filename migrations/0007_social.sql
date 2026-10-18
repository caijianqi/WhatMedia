-- 0007 收藏 / 通知 / 可写设置
--
-- 这三件事的共同点：它们都是"界面上画得出来、但服务端一个字段都没有"的功能。
-- 与其在前端放一排永远点不动的假按钮，不如把地基补上。
--
-- 三条设计决定：
--
-- 1. **收藏是 (user_id, item_id) 的联合主键，不带自增 id。**
--    收藏只有"有 / 没有"两种状态，给它一个 id 就是邀请别人写出
--    "同一条目被收藏两次"的 bug。取消收藏是 DELETE，重复收藏是 INSERT OR IGNORE。
-- 2. **通知在写入时扇出成每人一行，而不是存一条 user_id NULL 的广播。**
--    广播行的两难在于"已读"是**每人一份**的状态，却只有一行可写 ——
--    要么读的人把别人的也标成已读，要么再开一张关联表。
--    家庭场景用户数是几个量级，直接扇出最省心，且 DELETE 单条/清空都自然。
-- 3. **设置用 (scope, scope_id, key) 三元组，而不是两张表。**
--    server 级（管理员改、全局生效）与 user 级（各人自己的播放偏好）
--    结构完全一样，分成两张表只会让"读一个设置"要写两遍查询。
--
-- 另：`setting` 里只存**白名单内的键**（见 crates/config 的 `SETTING_KEYS`），
-- 不让 HTTP 层直接改任意配置 —— 否则一个 PATCH 就能把路径或端口写坏，
-- 而那时服务已经起不来、界面也已经打不开了。

-- ───────────────── 收藏 ─────────────────

CREATE TABLE IF NOT EXISTS favorite (
    user_id    TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    item_id    TEXT NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, item_id)
);
-- 反查"这个条目被谁收藏"：详情页要显示收藏数。
CREATE INDEX IF NOT EXISTS idx_favorite_item ON favorite(item_id);
-- "我的收藏"列表按时间倒序翻页。
CREATE INDEX IF NOT EXISTS idx_favorite_user_time ON favorite(user_id, created_at DESC);

-- ───────────────── 通知 ─────────────────

CREATE TABLE IF NOT EXISTS notification (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    -- scan_finished | scrape_finished | live_sync_failed | approval_pending | system
    kind       TEXT NOT NULL,
    -- info | warn | error。界面只用它决定颜色，不参与逻辑。
    level      TEXT NOT NULL DEFAULT 'info',
    title      TEXT NOT NULL,
    body       TEXT,
    -- 站内跳转目标，形如 `/item/<id>`。可空。
    link       TEXT,
    created_at TEXT NOT NULL,
    read_at    TEXT
);
-- 列表页：先按未读，再按时间倒序。
CREATE INDEX IF NOT EXISTS idx_notification_user ON notification(user_id, created_at DESC);
-- 顶栏小红点只查这一列，单独建索引避免全表扫。
CREATE INDEX IF NOT EXISTS idx_notification_unread ON notification(user_id, read_at);

-- ───────────────── 设置 ─────────────────

CREATE TABLE IF NOT EXISTS setting (
    scope      TEXT NOT NULL,   -- 'server' | 'user'
    scope_id   TEXT NOT NULL,   -- server 级固定写 'default'
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,   -- 一律存字符串，由白名单里的解析器转类型
    updated_at TEXT NOT NULL,
    updated_by TEXT,            -- 谁改的（server 级才有意义）
    PRIMARY KEY (scope, scope_id, key)
);
