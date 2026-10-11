//! 扫描专用查询：增量指纹索引与失效标记。

use anyhow::Result;
use whatmedia_core::Id;
use std::collections::HashMap;

use crate::db::Db;

/// 增量扫描的决策依据：路径 → (文件 id, 上次的 (mtime,size) 指纹)。
#[derive(Debug, Clone)]
pub struct FileIndexEntry {
    pub id: Id,
    pub fingerprint: String,
}

/// 拉取某个媒体库下所有已入库文件的指纹。
///
/// 10 万文件的库一次拉全量大约几十 MB 内存，可接受；
/// 换来的是增量扫描不需要逐条查库（逐条查在 ARM 上会明显变慢）。
pub async fn file_index_for_library(
    db: &Db,
    library_id: Id,
) -> Result<HashMap<String, FileIndexEntry>> {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT mf.id, mf.path, mf.file_fingerprint
           FROM media_file mf
           JOIN media_version mv ON mv.id = mf.version_id
           JOIN media_item   mi ON mi.id = mv.item_id
          WHERE mi.library_id = ?",
    )
    .bind(library_id.to_string())
    .fetch_all(db.pool())
    .await?;

    let mut map = HashMap::with_capacity(rows.len());
    for (id, path, fingerprint) in rows {
        if let Some(id) = crate::rows::to_id(&id) {
            map.insert(path, FileIndexEntry { id, fingerprint });
        }
    }
    Ok(map)
}

/// 把已不在磁盘上的文件标记为不可用（**不删除**）。
///
/// 为什么不做物理删除：移动硬盘/网络存储临时掉线是很常见的，
/// 直接删会把用户的续播点、手动改过的元数据一起带走。
/// 真正清理留给 P9 的维护任务，且需用户确认。
pub async fn mark_files_missing(db: &Db, ids: &[Id]) -> Result<usize> {
    let mut n = 0;
    for id in ids {
        let r = sqlx::query("UPDATE media_file SET is_available = 0 WHERE id = ?")
            .bind(id.to_string())
            .execute(db.pool())
            .await?;
        n += r.rows_affected() as usize;
    }
    Ok(n)
}
