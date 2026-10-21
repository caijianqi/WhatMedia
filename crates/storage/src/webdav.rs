//! WebDAV 存储实现。
//!
//! 支持通过标准 WebDAV 协议接入：
//! 1. AList、CloudDrive、rclone 等聚合网盘中转服务（挂载百度网盘、夸克网盘、迅雷网盘、移动云盘等）。
//! 2. 各种自建 NAS (Synology, QNAP, TrueNAS) 与公共 WebDAV 服务。
//! 3. 原生支持 HTTP Range 局部切片读取与 302 直链播放。

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RANGE};
use reqwest::{Client, Method, StatusCode};
use std::time::Duration;
use tracing::warn;
use whatmedia_core::{ByteRange, Result, StorageEntry, StorageProvider, WhatMediaError};

/// WebDAV 认证方式。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WebDavAuth {
    Anonymous,
    Basic { username: String, password: String },
    Bearer { token: String },
}

/// WebDAV 配置。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebDavConfig {
    pub id: String,
    /// WebDAV 服务器根端点 (例如 "http://127.0.0.1:5244/dav" 或 "https://pan.example.com/dav")
    pub endpoint: String,
    pub auth: WebDavAuth,
    /// 媒体库相对该 WebDAV 服务的起始根路径 (例如 "/" 或 "/夸克网盘/电影")
    pub root_path: String,
    /// 是否允许输出直链进行 302 重定向播放
    pub direct_stream: bool,
}

#[derive(Debug, Clone)]
pub struct WebDavStorage {
    cfg: WebDavConfig,
    client: Client,
    endpoint: String,
}

impl WebDavStorage {
    pub fn new(cfg: WebDavConfig) -> Result<Self> {
        let endpoint = cfg.endpoint.trim_end_matches('/').to_string();
        if endpoint.is_empty() || (!endpoint.starts_with("http://") && !endpoint.starts_with("https://")) {
            return Err(WhatMediaError::Storage {
                provider: cfg.id.clone(),
                message: format!("WebDAV 端点地址无效：{}", cfg.endpoint),
            });
        }

        let mut headers = HeaderMap::new();
        match &cfg.auth {
            WebDavAuth::Basic { username, password } => {
                use base64::Engine;
                let creds = format!("{}:{}", username, password);
                let encoded = base64::engine::general_purpose::STANDARD.encode(creds.as_bytes());
                if let Ok(val) = HeaderValue::from_str(&format!("Basic {}", encoded)) {
                    headers.insert(AUTHORIZATION, val);
                }
            }
            WebDavAuth::Bearer { token } => {
                if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", token)) {
                    headers.insert(AUTHORIZATION, val);
                }
            }
            WebDavAuth::Anonymous => {}
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| WhatMediaError::Storage {
                provider: cfg.id.clone(),
                message: format!("构建 WebDAV HTTP 客户端失败: {e}"),
            })?;

        Ok(Self {
            cfg,
            client,
            endpoint,
        })
    }

    /// 把相对根路径拼接成完整 URL。
    pub fn build_url(&self, path: &str) -> String {
        let clean_path = path.trim_matches('/');
        let base_root = self.cfg.root_path.trim_matches('/');
        if base_root.is_empty() {
            if clean_path.is_empty() {
                format!("{}/", self.endpoint)
            } else {
                format!("{}/{}", self.endpoint, url_encode_path(clean_path))
            }
        } else if clean_path.is_empty() {
            format!("{}/{}", self.endpoint, url_encode_path(base_root))
        } else {
            format!("{}/{}/{}", self.endpoint, url_encode_path(base_root), url_encode_path(clean_path))
        }
    }
}

/// 对 URL 路径中的每一个段进行编码，保留 '/' 分隔符。
fn url_encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            // 对每个路径段进行 URI 编码，避免中文或空格乱码
            let mut encoded = String::with_capacity(segment.len() * 2);
            for b in segment.bytes() {
                match b {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        encoded.push(b as char);
                    }
                    _ => {
                        encoded.push_str(&format!("%{:02X}", b));
                    }
                }
            }
            encoded
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 解码 URL 编码的字符串（如 %20 -> 空格）。
fn url_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(hex_str) = std::str::from_utf8(&[h1, h2]) {
                    if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
                        bytes.push(byte);
                        continue;
                    }
                }
            }
            bytes.push(b'%');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

/// 解析 WebDAV PROPFIND 返回的 XML multistatus。
pub fn parse_propfind_xml(xml: &str, base_url_prefix: &str) -> Vec<StorageEntry> {
    let doc = match roxmltree::Document::parse(xml) {
        Ok(d) => d,
        Err(e) => {
            warn!("解析 WebDAV PROPFIND XML 失败: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();

    for node in doc.descendants().filter(|n| n.has_tag_name("response")) {
        let href = match node.descendants().find(|n| n.has_tag_name("href")).and_then(|n| n.text()) {
            Some(h) => h,
            None => continue,
        };

        let decoded_href = url_decode(href);
        let trimmed_href = decoded_href.trim_end_matches('/');

        // 提取名称与路径
        let (name, is_dir) = {
            let resourcetype = node.descendants().find(|n| n.has_tag_name("resourcetype"));
            let is_col = resourcetype.map(|r| r.descendants().any(|n| n.has_tag_name("collection"))).unwrap_or(false)
                || href.ends_with('/');

            let displayname = node.descendants().find(|n| n.has_tag_name("displayname")).and_then(|n| n.text());
            let n = match displayname {
                Some(dn) if !dn.trim().is_empty() => dn.to_string(),
                _ => trimmed_href.split('/').last().unwrap_or("").to_string(),
            };
            (n, is_col)
        };

        // 文件大小
        let size = node
            .descendants()
            .find(|n| n.has_tag_name("getcontentlength"))
            .and_then(|n| n.text())
            .and_then(|s| s.trim().parse::<i64>().ok());

        // 修改时间 (getlastmodified: RFC 2822 / 1123, e.g. "Sun, 06 Nov 1994 08:49:37 GMT")
        let modified_ns = node
            .descendants()
            .find(|n| n.has_tag_name("getlastmodified"))
            .and_then(|n| n.text())
            .and_then(|s| {
                chrono::DateTime::parse_from_rfc2822(s.trim())
                    .or_else(|_| chrono::DateTime::parse_from_rfc3339(s.trim()))
                    .ok()
                    .and_then(|dt| dt.timestamp_nanos_opt())
            });

        let norm_base = base_url_prefix.trim_end_matches('/');
        // 相对路径
        let path = if trimmed_href.starts_with(norm_base) {
            trimmed_href[norm_base.len()..].trim_start_matches('/').to_string()
        } else {
            trimmed_href.to_string()
        };

        if !name.is_empty() {
            entries.push(StorageEntry {
                path,
                name,
                is_dir,
                size,
                modified_ns,
            });
        }
    }

    entries
}

#[async_trait]
impl StorageProvider for WebDavStorage {
    fn id(&self) -> &str {
        &self.cfg.id
    }

    fn is_remote(&self) -> bool {
        true
    }

    async fn get_direct_url(&self, path: &str) -> Result<Option<String>> {
        if self.cfg.direct_stream {
            Ok(Some(self.build_url(path)))
        } else {
            Ok(None)
        }
    }

    async fn list(&self, dir: &str) -> Result<Vec<StorageEntry>> {
        let url = self.build_url(dir);
        let req = self
            .client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Depth", "1")
            .header(CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(
                r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getlastmodified/>
  </D:prop>
</D:propfind>"#,
            );

        let res = req.send().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("WebDAV PROPFIND 请求失败 {url}: {e}"),
        })?;

        if !res.status().is_success() && res.status() != StatusCode::MULTI_STATUS {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV 返回错误状态码 {} for {url}", res.status()),
            });
        }

        let body = res.text().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("读取 WebDAV 响应失败: {e}"),
        })?;

        // 过滤掉自身目录
        let mut entries = parse_propfind_xml(&body, &url);
        let current_target = dir.trim_matches('/');
        entries.retain(|e| {
            let p = e.path.trim_matches('/');
            p != current_target && !p.is_empty()
        });

        Ok(entries)
    }

    async fn stat(&self, path: &str) -> Result<StorageEntry> {
        let url = self.build_url(path);
        let req = self
            .client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Depth", "0")
            .header(CONTENT_TYPE, "application/xml; charset=utf-8");

        let res = req.send().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("WebDAV stat 失败 {url}: {e}"),
        })?;

        if res.status() == StatusCode::NOT_FOUND {
            return Err(WhatMediaError::NotFound {
                entity: "webdav_file",
                id: path.into(),
            });
        }

        if !res.status().is_success() && res.status() != StatusCode::MULTI_STATUS {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV stat 状态码异常 {}: {url}", res.status()),
            });
        }

        let body = res.text().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("读取 WebDAV 响应体失败: {e}"),
        })?;

        let entries = parse_propfind_xml(&body, &url);
        entries.into_iter().next().ok_or_else(|| WhatMediaError::NotFound {
            entity: "webdav_file",
            id: path.into(),
        })
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let url = self.build_url(path);
        let res = self
            .client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Depth", "0")
            .send()
            .await;

        match res {
            Ok(r) => Ok(r.status().is_success() || r.status() == StatusCode::MULTI_STATUS),
            Err(_) => Ok(false),
        }
    }

    async fn range_read(&self, path: &str, range: ByteRange) -> Result<Vec<u8>> {
        let url = self.build_url(path);
        let range_header = match range.end {
            Some(end) => format!("bytes={}-{}", range.start, end),
            None => format!("bytes={}-", range.start),
        };

        let res = self
            .client
            .get(&url)
            .header(RANGE, range_header)
            .send()
            .await
            .map_err(|e| WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV Range 读取失败 {url}: {e}"),
            })?;

        if !res.status().is_success() && res.status() != StatusCode::PARTIAL_CONTENT {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV Range 读取返回状态码 {}: {url}", res.status()),
            });
        }

        let bytes = res.bytes().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("接收 WebDAV 数据流失败: {e}"),
        })?;

        Ok(bytes.to_vec())
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        let url = self.build_url(path);
        let res = self.client.get(&url).send().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("WebDAV GET 失败 {url}: {e}"),
        })?;

        if !res.status().is_success() {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV GET 返回错误 {}: {url}", res.status()),
            });
        }

        let bytes = res.bytes().await.map_err(|e| WhatMediaError::Storage {
            provider: self.id().into(),
            message: format!("接收 WebDAV 字节失败: {e}"),
        })?;

        Ok(bytes.to_vec())
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let url = self.build_url(path);
        let res = self
            .client
            .put(&url)
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV PUT 写入失败 {url}: {e}"),
            })?;

        if !res.status().is_success() && res.status() != StatusCode::CREATED && res.status() != StatusCode::NO_CONTENT {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV PUT 返回错误 {}: {url}", res.status()),
            });
        }

        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let from_url = self.build_url(from);
        let to_url = self.build_url(to);
        let res = self
            .client
            .request(Method::from_bytes(b"MOVE").unwrap(), &from_url)
            .header("Destination", &to_url)
            .header("Overwrite", "T")
            .send()
            .await
            .map_err(|e| WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV MOVE 失败 {from_url} -> {to_url}: {e}"),
            })?;

        if !res.status().is_success() {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV MOVE 返回错误 {}: {from_url}", res.status()),
            });
        }

        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let url = self.build_url(path);
        let res = self
            .client
            .delete(&url)
            .send()
            .await
            .map_err(|e| WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV DELETE 失败 {url}: {e}"),
            })?;

        if !res.status().is_success() && res.status() != StatusCode::NO_CONTENT {
            return Err(WhatMediaError::Storage {
                provider: self.id().into(),
                message: format!("WebDAV DELETE 返回错误 {}: {url}", res.status()),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode_and_decode() {
        let raw = "夸克网盘/4K 电影/奥本海默 Oppenheimer (2023).mkv";
        let encoded = url_encode_path(raw);
        assert!(encoded.contains("%20"));
        assert!(!encoded.contains(' '));
        let decoded = url_decode(&encoded);
        assert_eq!(decoded, raw);
    }

    #[test]
    fn test_parse_webdav_propfind_multistatus() {
        let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/%E7%94%B5%E5%BD%B1/</D:href>
    <D:propstat>
      <D:prop>
        <D:displayname>电影</D:displayname>
        <D:resourcetype><D:collection/></D:resourcetype>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/%E7%94%B5%E5%BD%B1/%E6%B5%81%E6%B5%AA%E5%9C%B0%E7%90%832.mp4</D:href>
    <D:propstat>
      <D:prop>
        <D:displayname>流浪地球2.mp4</D:displayname>
        <D:resourcetype/>
        <D:getcontentlength>15482390140</D:getcontentlength>
        <D:getlastmodified>Sun, 06 Nov 2024 08:49:37 GMT</D:getlastmodified>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let entries = parse_propfind_xml(xml, "/dav/电影");
        assert_eq!(entries.len(), 2);
        let file = &entries[1];
        assert_eq!(file.name, "流浪地球2.mp4");
        assert!(!file.is_dir);
        assert_eq!(file.size, Some(15482390140));
    }
}
