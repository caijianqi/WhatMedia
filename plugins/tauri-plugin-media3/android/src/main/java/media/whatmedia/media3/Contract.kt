package media.whatmedia.media3

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * 与 Rust `crates/media3/src/model.rs` 共享的数据契约。
 *
 * **改字段名的正确顺序**：先改 Rust（有测试）→ 再改本文件 → 最后改前端 TS。
 * 反着来的话 Rust 那条 `wire_format_matches_the_frontend_contract` 测试会红，
 * 但那只是"及时发现"，不会替你把 Kotlin 改对。
 *
 * 每个 `@SerialName` 都显式写出，即使和 Kotlin 属性名一样。
 * 理由：kotlinx.serialization 默认用属性名，一旦有人把属性改成 camelCase 之外
 * 的写法，线格式就悄悄变了 —— 而这里变了的后果是**运行时静默丢字段**，
 * 编译期完全无感。显式写出来至少让 diff 里能看见。
 */
object Contract {

    @Serializable
    enum class PlayerState {
        @SerialName("idle") IDLE,
        @SerialName("loading") LOADING,
        @SerialName("ready") READY,
        @SerialName("playing") PLAYING,
        @SerialName("paused") PAUSED,
        @SerialName("buffering") BUFFERING,
        @SerialName("ended") ENDED,
        @SerialName("error") ERROR,
    }

    @Serializable
    enum class ErrorCode {
        @SerialName("network") NETWORK,
        @SerialName("decode") DECODE,
        @SerialName("not-found") NOT_FOUND,
        @SerialName("autoplay-blocked") AUTOPLAY_BLOCKED,
        @SerialName("unknown") UNKNOWN,
        ;

        companion object {
            /** 认不出来的码归 UNKNOWN，绝不猜成 NETWORK（会触发无意义重连）。 */
            fun fromWire(value: String?): ErrorCode =
                entries.firstOrNull { it.wire == value } ?: UNKNOWN

            private val ErrorCode.wire: String
                get() = when (this) {
                    NETWORK -> "network"
                    DECODE -> "decode"
                    NOT_FOUND -> "not-found"
                    AUTOPLAY_BLOCKED -> "autoplay-blocked"
                    UNKNOWN -> "unknown"
                }
        }
    }

    @Serializable
    enum class RemoteKey {
        @SerialName("up") UP,
        @SerialName("down") DOWN,
        @SerialName("left") LEFT,
        @SerialName("right") RIGHT,
        @SerialName("select") SELECT,
        @SerialName("back") BACK,
        @SerialName("menu") MENU,
        @SerialName("toggle") TOGGLE,
        @SerialName("play") PLAY,
        @SerialName("pause") PAUSE,
        @SerialName("seek-forward") SEEK_FORWARD,
        @SerialName("seek-backward") SEEK_BACKWARD,
        @SerialName("next") NEXT,
        @SerialName("previous") PREVIOUS,
        @SerialName("stop") STOP,
    }

    @Serializable
    enum class NetworkKind {
        @SerialName("wifi") WIFI,
        @SerialName("cellular") CELLULAR,
        @SerialName("ethernet") ETHERNET,
        @SerialName("none") NONE,
        @SerialName("unknown") UNKNOWN,
    }

    /** id 形如 `g{group}:t{index}`。解析规则由 Rust 侧 `parse_track_id` 定义。 */
    @Serializable
    data class Track(
        @SerialName("id") val id: String,
        @SerialName("label") val label: String,
        @SerialName("language") val language: String? = null,
        @SerialName("default") val isDefault: Boolean = false,
        @SerialName("forced") val forced: Boolean = false,
        @SerialName("height") val height: Int? = null,
        @SerialName("bitrateKbps") val bitrateKbps: Int? = null,
    )

    @Serializable
    data class ErrorPayload(
        @SerialName("code") val code: ErrorCode,
        @SerialName("message") val message: String,
    )

    @Serializable
    data class Snapshot(
        @SerialName("state") val state: PlayerState,
        @SerialName("position") val position: Double,
        @SerialName("duration") val duration: Double,
        @SerialName("volume") val volume: Double,
        @SerialName("muted") val muted: Boolean,
        @SerialName("rate") val rate: Double,
        @SerialName("buffered") val buffered: Double,
        @SerialName("audioTracks") val audioTracks: List<Track> = emptyList(),
        @SerialName("subtitleTracks") val subtitleTracks: List<Track> = emptyList(),
        @SerialName("qualities") val qualities: List<Track> = emptyList(),
        @SerialName("currentAudioTrackId") val currentAudioTrackId: String? = null,
        @SerialName("currentSubtitleTrackId") val currentSubtitleTrackId: String? = null,
        @SerialName("currentQualityId") val currentQualityId: String? = null,
        @SerialName("error") val error: ErrorPayload? = null,
    )

    @Serializable
    sealed interface Source {
        @Serializable
        @SerialName("progressive")
        data class Progressive(@SerialName("url") val url: String) : Source

        @Serializable
        @SerialName("hls")
        data class Hls(@SerialName("url") val url: String) : Source
    }

    @Serializable
    data class ExternalSubtitle(
        @SerialName("label") val label: String,
        @SerialName("url") val url: String,
        @SerialName("language") val language: String? = null,
        @SerialName("default") val isDefault: Boolean = false,
        @SerialName("mime") val mime: String? = null,
    )

    /** 载入请求的载荷。前端 `Media3LoadArgs` 的手写对应物。 */
    @Serializable
    data class LoadArgs(
        @SerialName("source") val source: Source,
        @SerialName("startAt") val startAt: Double = 0.0,
        @SerialName("autoplay") val autoplay: Boolean = false,
        @SerialName("externalSubtitles") val externalSubtitles: List<ExternalSubtitle> = emptyList(),
    )

    @Serializable
    data class MetadataArgs(
        @SerialName("title") val title: String,
        @SerialName("subtitle") val subtitle: String = "",
        @SerialName("artworkUrl") val artworkUrl: String? = null,
        @SerialName("durationMs") val durationMs: Long = 0,
    )
}
