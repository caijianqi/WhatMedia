-- 0008_storage_accounts.sql
-- 存储源管理：支持 本地目录 / WebDAV / 统一网盘服务 (AList / CD2)

CREATE TABLE IF NOT EXISTS storage_account (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    driver         TEXT NOT NULL,                -- 'local' | 'webdav' | 'alist'
    endpoint       TEXT NOT NULL,                -- 远程端点 URL (如 http://127.0.0.1:5244/dav) 或本地根
    auth_type      TEXT NOT NULL DEFAULT 'none', -- 'none' | 'basic' | 'bearer'
    username       TEXT,
    password       TEXT,
    root_path      TEXT NOT NULL DEFAULT '/',
    direct_stream  INTEGER NOT NULL DEFAULT 1,    -- 1: 支持 302 直链秒开, 0: 仅走服务端代理
    extra_headers  TEXT NOT NULL DEFAULT '{}',    -- JSON 自定义头
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_storage_account_driver ON storage_account(driver);
