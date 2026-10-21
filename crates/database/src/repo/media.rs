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

type ItemSummaryRow = (
    String,
    String,
    String,
    Option<String>,
    Option<i32>,
    Option<f64>,
    Option<i64>,
    String,
    Option<i64>,
);

pub async fn count_items(db: &Db, q: &ItemQuery) -> Result<i64> {
    let is_aggregate = (q.aggregate_series == Some(true) || q.kind == Some(MediaKind::Series))
        && q.kind != Some(MediaKind::Episode);
    if is_aggregate {
        if let Some(kind) = q.kind {
            let kind_str = from_kind(kind);

            // 1. 散片中符合该 kind 的数量（排除属于任何剧集的分集）
            let mut b1 = QueryBuilder::<sqlx::Sqlite>::new(
                "SELECT COUNT(*) FROM media_item m
                 WHERE m.kind = ",
            );
            b1.push_bind(kind_str);
            b1.push(" AND NOT EXISTS (SELECT 1 FROM episode e WHERE e.item_id = m.id)");
            if let Some(id) = q.library_id {
                b1.push(" AND m.library_id = ").push_bind(id.to_string());
            }
            if let Some(year) = q.year {
                b1.push(" AND m.year = ").push_bind(year);
            }
            let standalone_count: (i64,) = b1.build_query_as().fetch_one(db.pool()).await?;

            // 2. 剧集中推导类型符合该 kind 的数量
            let mut b2 = QueryBuilder::<sqlx::Sqlite>::new(
                "SELECT COUNT(*) FROM series s
                 WHERE COALESCE(
                     (SELECT m.kind FROM episode e 
                      JOIN season se ON se.id = e.season_id 
                      JOIN media_item m ON m.id = e.item_id 
                      WHERE se.series_id = s.id AND m.kind IN ('anime', 'documentary', 'variety') 
                      LIMIT 1),
                     'series'
                 ) = ",
            );
            b2.push_bind(kind_str);
            b2.push(
                " AND EXISTS (
                     SELECT 1 FROM season se
                     JOIN episode e ON e.season_id = se.id
                     JOIN media_item m ON m.id = e.item_id
                     WHERE se.series_id = s.id",
            );
            if let Some(lib_id) = q.library_id {
                b2.push(" AND m.library_id = ").push_bind(lib_id.to_string());
            }
            b2.push(")");
            if let Some(year) = q.year {
                b2.push(" AND s.year = ").push_bind(year);
            }
            let series_count: (i64,) = b2.build_query_as().fetch_one(db.pool()).await?;

            return Ok(standalone_count.0 + series_count.0);
        }

        // 全部（All）：未被剧集关联的所有散片 + 所有剧集
        let mut b1 = QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT COUNT(*) FROM media_item m
             WHERE NOT EXISTS (SELECT 1 FROM episode e WHERE e.item_id = m.id)",
        );
        if let Some(id) = q.library_id {
            b1.push(" AND m.library_id = ").push_bind(id.to_string());
        }
        if let Some(year) = q.year {
            b1.push(" AND m.year = ").push_bind(year);
        }
        let standalone_count: (i64,) = b1.build_query_as().fetch_one(db.pool()).await?;

        let mut b2 = QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT COUNT(*) FROM series s WHERE EXISTS (
                SELECT 1 FROM season se
                JOIN episode e ON e.season_id = se.id
                JOIN media_item m ON m.id = e.item_id
                WHERE se.series_id = s.id",
        );
        if let Some(lib_id) = q.library_id {
            b2.push(" AND m.library_id = ").push_bind(lib_id.to_string());
        }
        b2.push(")");
        if let Some(year) = q.year {
            b2.push(" AND s.year = ").push_bind(year);
        }
        let series_total: (i64,) = b2.build_query_as().fetch_one(db.pool()).await?;

        return Ok(standalone_count.0 + series_total.0);
    }

    // 默认底层查询：直接统计 media_item
    let mut b = QueryBuilder::<sqlx::Sqlite>::new("SELECT COUNT(*) FROM media_item WHERE 1=1");
    apply_filters(&mut b, q);
    let total: (i64,) = b.build_query_as().fetch_one(db.pool()).await?;
    Ok(total.0)
}

/// 每个类型的条目数（首页"分类 tab"用它决定显示哪几个 tab）。
///
/// **只返回数量大于 0 的类型**。剧集按聚合的 Series 数量统计，避免单集分散平铺。
pub async fn kind_counts(db: &Db) -> Result<Vec<KindCount>> {
    // 1. 统计未被挂载到剧集的独立条目（散片）
    let standalone_rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT kind, COUNT(*) AS n FROM media_item m
          WHERE NOT EXISTS (SELECT 1 FROM episode e WHERE e.item_id = m.id)
          GROUP BY kind HAVING n > 0",
    )
    .fetch_all(db.pool())
    .await?;

    // 2. 统计剧集（按 Series 实体聚合，有单集的才算），并根据分集推导 series 的真实 kind
    let series_rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT 
            COALESCE(
                (SELECT m.kind FROM episode e 
                 JOIN season se ON se.id = e.season_id 
                 JOIN media_item m ON m.id = e.item_id 
                 WHERE se.series_id = s.id AND m.kind IN ('anime', 'documentary', 'variety') 
                 LIMIT 1),
                'series'
            ) AS inferred_kind,
            COUNT(*) AS n
         FROM series s
         WHERE EXISTS (
             SELECT 1 FROM season se
             JOIN episode e ON e.season_id = se.id
             WHERE se.series_id = s.id
         )
         GROUP BY inferred_kind HAVING n > 0",
    )
    .fetch_all(db.pool())
    .await?;

    let mut map: std::collections::HashMap<MediaKind, i64> = std::collections::HashMap::new();
    for (k, n) in standalone_rows {
        *map.entry(crate::rows::to_kind(&k)).or_insert(0) += n;
    }
    for (k, n) in series_rows {
        *map.entry(crate::rows::to_kind(&k)).or_insert(0) += n;
    }

    let mut counts: Vec<KindCount> = map
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(kind, count)| KindCount { kind, count })
        .collect();

    counts.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| format!("{:?}", a.kind).cmp(&format!("{:?}", b.kind)))
    });
    Ok(counts)
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

    let is_aggregate = (q.aggregate_series == Some(true) || q.kind == Some(MediaKind::Series))
        && q.kind != Some(MediaKind::Episode);
    if is_aggregate {
        let mut b = QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT id, kind, title, original_title, year, rating, runtime_ms, added_at, episode_count FROM (
                SELECT 
                    m.id AS id,
                    m.kind AS kind,
                    m.title AS title,
                    m.original_title AS original_title,
                    m.year AS year,
                    m.rating AS rating,
                    m.runtime_ms AS runtime_ms,
                    m.added_at AS added_at,
                    NULL AS episode_count
                FROM media_item m
                WHERE NOT EXISTS (SELECT 1 FROM episode e WHERE e.item_id = m.id)",
        );
        if let Some(kind) = q.kind {
            b.push(" AND m.kind = ").push_bind(from_kind(kind));
        }
        if let Some(id) = q.library_id {
            b.push(" AND m.library_id = ").push_bind(id.to_string());
        }
        if let Some(year) = q.year {
            b.push(" AND m.year = ").push_bind(year);
        }

        b.push(
            " UNION ALL
            SELECT 
                (SELECT e.item_id 
                 FROM episode e 
                 JOIN season se ON se.id = e.season_id 
                 JOIN media_item m ON m.id = e.item_id
                 WHERE se.series_id = s.id 
                 ORDER BY se.season_number ASC, e.episode_number ASC 
                 LIMIT 1) AS id,
                COALESCE(
                    (SELECT m.kind FROM episode e 
                     JOIN season se ON se.id = e.season_id 
                     JOIN media_item m ON m.id = e.item_id 
                     WHERE se.series_id = s.id AND m.kind IN ('anime', 'documentary', 'variety') 
                     LIMIT 1),
                    'series'
                ) AS kind,
                s.title AS title,
                s.original_title AS original_title,
                s.year AS year,
                NULL AS rating,
                NULL AS runtime_ms,
                COALESCE(
                    (SELECT m.added_at 
                     FROM episode e 
                     JOIN season se ON se.id = e.season_id 
                     JOIN media_item m ON m.id = e.item_id
                     WHERE se.series_id = s.id 
                     ORDER BY m.added_at DESC 
                     LIMIT 1),
                    '1970-01-01T00:00:00Z'
                ) AS added_at,
                (SELECT COUNT(*) 
                 FROM episode e 
                 JOIN season se ON se.id = e.season_id 
                 WHERE se.series_id = s.id) AS episode_count
            FROM series s
            WHERE EXISTS (
                SELECT 1 FROM season se
                JOIN episode e ON e.season_id = se.id
                JOIN media_item m ON m.id = e.item_id
                WHERE se.series_id = s.id",
        );
        if let Some(lib_id) = q.library_id {
            b.push(" AND m.library_id = ").push_bind(lib_id.to_string());
        }
        b.push(")");
        if let Some(kind) = q.kind {
            b.push(
                " AND COALESCE(
                    (SELECT m.kind FROM episode e 
                     JOIN season se ON se.id = e.season_id 
                     JOIN media_item m ON m.id = e.item_id 
                     WHERE se.series_id = s.id AND m.kind IN ('anime', 'documentary', 'variety') 
                     LIMIT 1),
                    'series'
                ) = ",
            );
            b.push_bind(from_kind(kind));
        }
        if let Some(year) = q.year {
            b.push(" AND s.year = ").push_bind(year);
        }

        b.push(") ORDER BY ");
        b.push(q.sort.sql());
        b.push(" ");
        b.push(q.order.sql());
        b.push(" LIMIT ");
        b.push_bind(size as i64);
        b.push(" OFFSET ");
        b.push_bind(q.offset());

        let rows = b.build_query_as::<ItemSummaryRow>().fetch_all(db.pool()).await?;
        let data = rows
            .into_iter()
            .filter_map(|r| {
                Some(ItemSummary {
                    id: to_id(&r.0)?,
                    kind: crate::rows::to_kind(&r.1),
                    title: r.2,
                    original_title: r.3,
                    year: r.4,
                    rating: r.5,
                    runtime_ms: r.6,
                    added_at: r.7,
                    episode_count: r.8,
                })
            })
            .collect();

        return Ok(page_of(data, total, q));
    }

    // 默认底层查询：直接查询 media_item
    let mut b = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT id, kind, title, original_title, year, rating, runtime_ms, added_at, NULL AS episode_count
         FROM media_item WHERE 1=1",
    );
    apply_filters(&mut b, q);
    b.push(" ORDER BY ");
    b.push(q.sort.sql());
    b.push(" ");
    b.push(q.order.sql());
    b.push(" LIMIT ");
    b.push_bind(size as i64);
    b.push(" OFFSET ");
    b.push_bind(q.offset());

    let rows = b.build_query_as::<ItemSummaryRow>().fetch_all(db.pool()).await?;
    let data = rows
        .into_iter()
        .filter_map(|r| {
            Some(ItemSummary {
                id: to_id(&r.0)?,
                kind: crate::rows::to_kind(&r.1),
                title: r.2,
                original_title: r.3,
                year: r.4,
                rating: r.5,
                runtime_ms: r.6,
                added_at: r.7,
                episode_count: r.8,
            })
        })
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
