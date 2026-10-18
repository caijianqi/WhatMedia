-- 0006 鉴权与多用户
--
-- 0001 里 user / device 两表建好了骨架，注释写的是"P9 启用"。现在提前启用，
-- 原因是它是**唯一一个不能后补的东西**：默认开放的服务一旦被扫到，
-- 整个媒体库（含文件路径）就是裸奔的。鉴权不是功能，是地基。
--
-- 三条设计决定：
--
-- 1. **口令不落明文，只落 argon2id 摘要。** 建 `session` 表而不是依赖 JWT，
--    因为 JWTS 无法吊销 —— 用户"退出所有设备"这个动作必须能立刻生效。
-- 2. **儿童账户的审批是一件具体的物（`approval_request`），不是一个布尔位。**
--    它需要记录"谁申请的、谁批准的、什么时候、批的哪台设备"，
--    否则事后无法解释"孩子为什么能在这台电视上看 R 级片"。
-- 3. **role 收敛成 `admin | user | child` 三个值。** 0001 只有 `user`，
--    而侮辱之处在于：任何一个 admin 都必须是**显式赋予**的，
--    不能因为"刚好是第一个创建的用户"就隐式成为管理员 —— 那条规则一旦写歪，
--    后注册的永远是普通用户，先注册的永远是管理员，无法转让。

-- ───────────────── 用户补齐 ─────────────────

ALTER TABLE user ADD COLUMN pin_hash TEXT;         -- 家长 PIN（审批用），argon2id，独立于登录口令
ALTER TABLE user ADD COLUMN parent_id TEXT REFERENCES user(id) ON DELETE SET NULL;
ALTER TABLE user ADD COLUMN max_rating TEXT;       -- 内容分级上限：NULL=不限；G|PG|PG-13|R
ALTER TABLE user ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE user ADD COLUMN last_login_at TEXT;

-- 登录爆破保护：连续失败累计，`locked_until` 到期自动解锁。
-- 不做 IP 维度 —— 家庭内网全是同一个出口 IP，按 IP 限等于把全家一起锁死。
ALTER TABLE user ADD COLUMN failed_logins INTEGER NOT NULL DEFAULT 0;
ALTER TABLE user ADD COLUMN locked_until TEXT;

-- role 收敛：0001 之前只可能产生 'user'。此处**刻意不自动提权**，
-- 管理员由 `crates/auth::bootstrap` 在首次启动时显式指定。
UPDATE user SET role = 'user' WHERE role IS NULL OR role NOT IN ('admin', 'user', 'child');

-- 儿童标记与 role 同步：is_child 是旧字段，保留但不再作为判断依据，
-- 判断一律走 role = 'child'。
UPDATE user SET is_child = 1 WHERE role = 'child';
UPDATE user SET is_child = 0 WHERE role <> 'child';

-- ───────────────── 会话 ─────────────────
--
-- 一条 row 就是"这台设备的这个登录态"。token 明文只在**登录响应里出现一次**，
-- 之后服务端只认 `token_hash`（SHA-256），库被读走也换不来回话权。
--
-- kind 的差别：
--   web    —— 浏览器，走 Cookie，有效期短（默认 12 小时）
--   device —— Tauri / 移动端 / 电视，走 Bearer，长期（默认 90 天）
--   stream —— 只为播放单个 resource 签发的短命票据（默认 4 小时）

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
CREATE INDEX IF NOT EXISTS idx_session_user ON session(user_id, revoked_at);
CREATE INDEX IF NOT EXISTS idx_session_expiry ON session(expires_at);

-- ───────────────── 设备补齐 ─────────────────

ALTER TABLE device ADD COLUMN kind TEXT;          -- web | desktop | mobile | tv
ALTER TABLE device ADD COLUMN platform TEXT;      -- windows | macos | linux | android | ios | web
ALTER TABLE device ADD COLUMN app_version TEXT;
ALTER TABLE device ADD COLUMN last_ip TEXT;
ALTER TABLE device ADD COLUMN created_at TEXT;

-- `approved = 0` 的设备不能登录，直到家长在 `approval_request` 里放行。
-- 默认 1 是刻意的：已经存在（或由家长自己登记）的设备不该被 retroactively 关在门外。
ALTER TABLE device ADD COLUMN approved INTEGER NOT NULL DEFAULT 1;
ALTER TABLE device ADD COLUMN approval_id TEXT;
CREATE INDEX IF NOT EXISTS idx_device_user ON device(user_id);

-- ───────────────── 审批请求 ─────────────────
--
-- 儿童账户在新设备上登录 → 建一条 pending → 服务端回 403 APPROVAL_REQUIRED，
-- 前端把 request id 显示成"已请求，请家长批准" → 家长设备在
-- `/api/v1/auth/approvals` 看到并放行 → 设备 approved=1，儿童重新登录成功。
--
-- kind 预留 playback（超分级请求 PLAYLIST）与 purchase（未来），一期只实现 device。

CREATE TABLE IF NOT EXISTS approval_request (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    device_id    TEXT,
    kind         TEXT NOT NULL DEFAULT 'device',  -- device | rating
    title        TEXT NOT NULL,
    detail       TEXT,
    status       TEXT NOT NULL DEFAULT 'pending', -- pending | approved | denied | expired
    requested_at TEXT NOT NULL,
    decided_at   TEXT,
    decided_by   TEXT REFERENCES user(id) ON DELETE SET NULL,
    note         TEXT
);
CREATE INDEX IF NOT EXISTS idx_approval_user ON approval_request(user_id, status);
CREATE INDEX IF NOT EXISTS idx_approval_pending ON approval_request(status, requested_at DESC);
