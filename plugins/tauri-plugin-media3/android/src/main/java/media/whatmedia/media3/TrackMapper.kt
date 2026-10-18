package media.whatmedia.media3

import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.Tracks
import kotlinx.serialization.Serializable

/**
 * 一次性把三套轨道原值送回 Rust。
 *
 * **注意字段名必须与 Rust `RawTracksPayload` 一致**，且 Rust 侧用 `Option` 收
 * —— Kotlin 序列化时缺字段不会报错，Rust 侧会静默拿到空列表，
 * 表现为"菜单里一条轨道都没有"，而且日志里没有任何线索。
 */
@Serializable
data class RawTracksPayload(
    val audio: List<RawTrack> = emptyList(),
    val text: List<RawTrack> = emptyList(),
    val video: List<RawTrack> = emptyList(),
)

/**
 * 原始轨道描述。**故意不带 label** —— 标签与 id 的编法在 Rust 侧
 * （`crates/media3/src/protocol.rs` 的 `to_tracks`），那边有测试。
 *
 * 这里只做"把 ExoPlayer 的原值抠出来"，一行语义判断都不做。
 * 一旦有人在这里加 `if (language == "chi") "中文"`，Rust 那 36 条测试就白写了。
 */
@Serializable
data class RawTrack(
    val group: Int,
    val index: Int,
    val title: String? = null,
    val language: String? = null,
    val codecs: String? = null,
    val selected: Boolean = false,
    val default: Boolean = false,
    val forced: Boolean = false,
    val height: Int? = null,
    val bitrateKbps: Int? = null,
)

/**
 * ExoPlayer 的 `Tracks` → 原始轨道列表。
 *
 * 两个坑：
 *
 * 1. **必须跳过 `isSupported == false` 的 group**。ExoPlayer 会把"设备放不了"
 *    的轨道照样列出来（比如没有 DTS 解码器的机器上的 DTS 音轨）。
 *    列进菜单就是给用户一个点了必失败的按钮。
 *
 * 2. **必须跳过 `C.TRACK_TYPE_METADATA` 等辅助类型**，尤其是
 *    HLS 里的 ID3 元数据轨 —— 它会被当成一条字幕轨出现在菜单里，
 *    显示成一个奇怪的 "轨道 N"。
 */
internal fun Tracks.rawTracksOfType(vararg types: Int): List<RawTrack> {
    val out = ArrayList<RawTrack>()
    for (group in this.groups) {
        if (!group.isSupported) continue
        if (group.type !in types) continue
        val trackGroup = group.mediaTrackGroup
        for (i in 0 until trackGroup.length) {
            val format = trackGroup.getFormat(i)
            out.add(
                RawTrack(
                    group = groups.indexOf(group),
                    index = i,
                    title = format.label,
                    language = format.language,
                    codecs = format.codecs,
                    selected = group.isTrackSelected(i),
                    default = format.selectionFlags and C.SELECTION_FLAG_DEFAULT != 0,
                    forced = format.selectionFlags and C.SELECTION_FLAG_FORCED != 0,
                    height = format.height.takeIf { it != Format.NO_VALUE },
                    bitrateKbps = format.bitrate.takeIf { it != Format.NO_VALUE }
                        ?.let { it / 1000 },
                )
            )
        }
    }
    return out
}

/** 音轨。 */
internal fun Tracks.rawAudioTracks(): List<RawTrack> =
    rawTracksOfType(C.TRACK_TYPE_AUDIO)

/** 字幕轨。含 CEA-608/708（内嵌字幕）—— 它们也是 TEXT 类型。 */
internal fun Tracks.rawTextTracks(): List<RawTrack> =
    rawTracksOfType(C.TRACK_TYPE_TEXT)

/** 视频轨。画质档从它推导。 */
internal fun Tracks.rawVideoTracks(): List<RawTrack> =
    rawTracksOfType(C.TRACK_TYPE_VIDEO)
