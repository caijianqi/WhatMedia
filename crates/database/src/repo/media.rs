//! 媒体资产读写：item / version / file / stream。

use anyhow::Result;
use serde::Serialize;
use sqlx::QueryBuilder;
use whatmedia_core::{Id, MediaFile, MediaItem, MediaKind, MediaStream, MediaVersion, Page};

use crate::db::Db;
use crate::repo::{page_of, ItemQuery, ItemSummary};
use crate::rows::{
    codec_name, container_name, from_hdr, from_kind, from_stream_type, to_id, FileRow, ItemRow,
    StreamRow, VersionRow,
};

// ─────────────────────────────────────────────────────────────
// 写入
// ─────────────────────────────────────────────────────────────

pub async fn upsert_item(db: &Db, item: &MediaItem) -> Result<()> {
    sqlx::query(
        "INSERT INTO media_item (
            id, library_id, kind, title, original_title, sort_title, year, overview,
            runtime_ms, rating, locked_fields, provider, provider_id, added_at, match_title
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET
            -- match_title 永远跟随文件名，不受用户改名影响
            match_title = excluded.match_title,
            title = excluded.title,
            original_title = excluded.original_title,
            sort_title = excluded.sort_title,
            year = excluded.year,
            overview = excluded.overview,
            runtime_ms = excluded.runtime_ms,
            rating = excluded.rating,
            locked_fields = excluded.locked_fields,
            provider = excluded.provider,
            provider_id = excluded.provider_id",
    )
    .bind(item.id.to_string())
    .bind(item.library_id.to_string())
    .bind(from_kind(item.kind))
    .bind(&item.title)
    .bind(&item.original_title)
    .bind(&item.sort_title)
    .bind(item.year)
    .bind(&item.overview)
    .bind(item.runtime_ms)
    .bind(item.rating)
    .bind(serde_json::to_string(&item.locked_fields)?)
    .bind(&item.provider)
    .bind(&item.provider_id)
    .bind(item.added_at.to_rfc3339())
    .bind(&item.title)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn upsert_version(db: &Db, v: &MediaVersion) -> Result<()> {
    sqlx::query(
        "INSERT INTO media_version (id, item_id, name, source, resolution, video_codec, hdr_type, is_default)
         VALUES (?,?,?,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            source = excluded.source,
            resolution = excluded.resolution,
            video_codec = excluded.video_codec,
            hdr_type = excluded.hdr_type,
            is_default = excluded.is_default",
    )
    .bind(v.id.to_string())
    .bind(v.item_id.to_string())
    .bind(&v.name)
    .bind(&v.source)
    .bind(&v.resolution)
    .bind(v.video_codec.as_ref().map(codec_name))
    .bind(from_hdr(v.hdr_type))
    .bind(v.is_default as i32)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 按 (存储, 路径) 幂等写入 —— 重复扫描同一个文件不会产生第二条记录。
pub async fn upsert_file(db: &Db, f: &MediaFile) -> Result<Id> {
    let existing = find_file_by_path(db, &f.storage_provider, &f.path).await?;

    match existing {
        Some(row_id) => {
            sqlx::query(
                "UPDATE media_file SET version_id = ?, container = ?, size_bytes = ?,
                        duration_ms = ?, bitrate = ?, file_fingerprint = ?,
                        probe_json = ?, is_available = 1
                 WHERE id = ?",
            )
            .bind(f.version_id.to_string())
            .bind(f.container.as_ref().map(container_name))
            .bind(f.size_bytes)
            .bind(f.duration_ms)
            .bind(f.bitrate)
            .bind(&f.file_fingerprint)
            .bind(&f.probe_json)
            .bind(row_id.to_string())
            .execute(db.pool())
            .await?;
            Ok(row_id)
        }
        None => {
            sqlx::query(
                "INSERT INTO media_file (
                    id, version_id, storage_provider, path, container, size_bytes,
                    duration_ms, bitrate, file_fingerprint, probe_json, is_available
                 ) VALUES (?,?,?,?,?,?,?,?,?,?,1)",
            )
            .bind(f.id.to_string())
            .bind(f.version_id.to_string())
            .bind(&f.storage_provider)
            .bind(&f.path)
            .bind(f.container.as_ref().map(container_name))
            .bind(f.size_bytes)
            .bind(f.duration_ms)
            .bind(f.bitrate)
            .bind(&f.file_fingerprint)
            .bind(&f.probe_json)
            .execute(db.pool())
            .await?;
            Ok(f.id)
        }
    }
}

pub async fn find_file_by_path(db: &Db, storage_provider: &str, path: &str) -> Result<Option<Id>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM media_file WHERE storage_provider = ? AND path = ? LIMIT 1")
            .bind(storage_provider)
            .bind(path)
            .fetch_optional(db.pool())
            .await?;
    Ok(row.and_then(|(id,)| to_id(&id)))
}

/// 整轨替换。文件变了流就会变，做增量 diff 不划算。
pub async fn replace_streams(db: &Db, file_id: Id, streams: &[MediaStream]) -> Result<()> {
    sqlx::query("DELETE FROM media_stream WHERE file_id = ?")
        .bind(file_id.to_string())
        .execute(db.pool())
        .await?;

    for s in streams {
        sqlx::query(
            "INSERT INTO media_stream (
                id, file_id, stream_type, idx, codec, language, title,
                is_default, is_forced, width, height, bit_depth, hdr_type, channels, bitrate
             ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(s.id.to_string())
        .bind(file_id.to_string())
        .bind(from_stream_type(s.stream_type))
        .bind(s.index)
        .bind(&s.codec)
        .bind(&s.language)
        .bind(&s.title)
        .bind(s.is_default as i32)
        .bind(s.is_forced as i32)
        .bind(s.width)
        .bind(s.height)
        .bind(s.bit_depth)
        .bind(from_hdr(s.hdr_type))
        .bind(s.channels)
        .bind(s.bitrate)
        .execute(db.pool())
        .await?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────
// 读取
// ─────────────────────────────────────────────────────────────

pub async fn count_items(db: &Db, q: &ItemQuery) -> Result<i64> {
    let mut b = QueryBuilder::<sqlx::Sqlite>::new("SELECT COUNT(*) FROM media_item WHERE 1=1");
    apply_filters(&mut b, q);
    let total: (i64,) = b.build_query_as().fetch_one(db.pool()).await?;
    Ok(total.0)
}

/// 每个类型的条目数（首页"分类 tab"用它决定显示哪几个 tab）。
///
/// **只返回数量大于 0 的类型**。硬编码一份"电影 / 剧集 / 动画 / 纪录片"的
/// tab 列表，在只有电影的库里会留下三个点进去永远是空的 tab ——
/// 那种空 tab 比没有 tab 更糟，它让人以为自己的库坏了。
///
/// 顺序按数量降序：最常用的类型在最前面。
pub async fn kind_counts(db: &Db) -> Result<Vec<KindCount>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT kind, COUNT(*) AS n FROM media_item
          GROUP BY kind HAVING n > 0
          ORDER BY n DESC, kind",
    )
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(kind, n)| KindCount {
            // `to_kind` 不认识的串会落回 Mixed。这里不因此丢行：
            // 库里真有 `mixed` 时它也是一个可点的分类，只是标签不好看而已。
            kind: crate::rows::to_kind(&kind),
            count: n,
        })
        .collect())
}

/// 一个类型有多少条目。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct KindCount {
    pub kind: MediaKind,
    pub count: i64,
}

pub async fn list_items(db: &Db, q: &ItemQuery) -> Result<Page<ItemSummary>> {
    let total = count_items(db, q).await?;
    let (_, size) = q.clamp();

    let mut b = QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM media_item WHERE 1=1");
    apply_filters(&mut b, q);
    b.push(" ORDER BY ");
    b.push(q.sort.sql());
    b.push(" ");
    b.push(q.order.sql());
    b.push(" LIMIT ");
    b.push_bind(size as i64);
    b.push(" OFFSET ");
    b.push_bind(q.offset());

    let rows = b.build_query_as::<ItemRow>().fetch_all(db.pool()).await?;
    let data = rows
        .into_iter()
        .filter_map(|r| r.into_domain())
        .map(|i| ItemSummary::from_row(&i))
        .collect();

    Ok(page_of(data, total, q))
}

fn apply_filters(b: &mut QueryBuilder<sqlx::Sqlite>, q: &ItemQuery) {
    if let Some(id) = q.library_id {
        b.push(" AND library_id = ").push_bind(id.to_string());
    }
    if let Some(kind) = q.kind {
        b.push(" AND kind = ").push_bind(from_kind(kind));
    }
    if let Some(year) = q.year {
        b.push(" AND year = ").push_bind(year);
    }
}

/// 详情：item + 所有版本 + 每个版本的文件 + 每个文件的流。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ItemDetail {
    #[serde(flatten)]
    pub item: ItemSummary,
    pub overview: Option<String>,
    /// 类型（动作 / 科幻 …）。**刮削之前是空数组**，不是 null ——
    /// 前端拿到 null 还得自己判空，而"没有类型"和"没刮削过"对 UI 是同一件事。
    pub genres: Vec<String>,
    pub versions: Vec<VersionWithFiles>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VersionWithFiles {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub name: String,
    pub source: Option<String>,
    pub resolution: Option<String>,
    pub hdr_type: String,
    pub is_default: bool,
    pub files: Vec<FileWithStreams>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FileWithStreams {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub path: String,
    pub storage_provider: String,
    pub size_bytes: i64,
    pub duration_ms: Option<i64>,
    pub is_available: bool,
    pub streams: Vec<MediaStream>,
}

pub async fn get_item(db: &Db, id: Id) -> Result<Option<ItemDetail>> {
    let row = sqlx::query_as::<_, ItemRow>("SELECT * FROM media_item WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    let item = match row.and_then(|r| r.into_domain()) {
        Some(i) => i,
        None => return Ok(None),
    };

    let version_rows = sqlx::query_as::<_, VersionRow>(
        "SELECT * FROM media_version WHERE item_id = ? ORDER BY is_default DESC, name",
    )
    .bind(id.to_string())
    .fetch_all(db.pool())
    .await?;

    let mut versions = Vec::new();
    for vr in version_rows {
        let version = match vr.clone().into_domain() {
            Some(v) => v,
            None => continue,
        };

        let file_rows = sqlx::query_as::<_, FileRow>(
            "SELECT * FROM media_file WHERE version_id = ? ORDER BY path",
        )
        .bind(version.id.to_string())
        .fetch_all(db.pool())
        .await?;

        let mut files = Vec::new();
        for fr in file_rows {
            let file = match fr.clone().into_domain() {
                Some(f) => f,
                None => continue,
            };
            let stream_rows = sqlx::query_as::<_, StreamRow>(
                "SELECT * FROM media_stream WHERE file_id = ? ORDER BY idx",
            )
            .bind(file.id.to_string())
            .fetch_all(db.pool())
            .await?;
            files.push(FileWithStreams {
                id: file.id,
                path: file.path,
                storage_provider: file.storage_provider,
                size_bytes: file.size_bytes,
                duration_ms: file.duration_ms,
                is_available: file.is_available,
                streams: stream_rows
                    .into_iter()
                    .filter_map(|s| s.into_domain())
                    .collect(),
            });
        }

        versions.push(VersionWithFiles {
            id: version.id,
            name: version.name,
            source: version.source,
            resolution: version.resolution,
            hdr_type: serde_json::to_value(version.hdr_type)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "sdr".into()),
            is_default: version.is_default,
            files,
        });
    }

    // 类型在另一张关联表上，单独取。刮削没跑过时 `genres_of` 返回空数组。
    let genres = crate::repo::metadata::genres_of(db, id)
        .await
        .unwrap_or_default();

    Ok(Some(ItemDetail {
        item: ItemSummary::from_row(&item),
        overview: item.overview,
        genres,
        versions,
    }))
}
