package media.whatmedia.media3

import android.content.Context
import android.graphics.Color
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.TrackSelectionParameters
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import androidx.media3.ui.PlayerView

/**
 * ExoPlayer 的持有者：唯一跟 Media3 说话的地方。
 *
 * ## 图层关系（S1 待真机确认）
 *
 * 播放器画在 WebView **下面**：把 `PlayerView` 插到 `android.R.id.content` 的第 0 位，
 * 再把 WebView 背景置透明，于是 Vue 渲染的控制条/字幕菜单浮在画面之上。
 *
 * 这样排的理由：UI 复杂（画质菜单、字幕列表、手势层）用 Vue 写成本很低，
 * 而用原生 View 重写一遍代价极高。反过来（原生播放器在上）就只能原生写 UI。
 *
 * **风险**：SurfaceView 在 WebView 之下时，部分机型上会出现
 * "画面黑但声音正常"或"控制条半透明区域透出奇怪颜色"。
 * 这正是 S1 Spike 要验的第一件事。降级方案见 `docs/adr/0005`：
 * 改成 `TextureView`（`surface_type = texture_view`），代价是功耗略高。
 */
@UnstableApi
internal class PlayerHolder(
    private val context: Context,
    private val emitter: EventEmitter,
) {

    /** 事件出口。由插件实现，转发给 Rust → TS。 */
    fun interface EventEmitter {
        fun emit(json: String)
    }

    val player: ExoPlayer = ExoPlayer.Builder(context)
        .setMediaSourceFactory(
            // HLS 走 Media3 自带的 HlsMediaSource，progressive（mp4/mkv）
            // 走默认的 ProgressiveMediaSource —— 一个工厂两种都能认，
            // 不需要按 Source 类型自己挑。
            //
            // 不要在这里 setDataSourceFactory(OkHttp)：Tauri 的 WebView 已经有一套
            // 网络栈，再引一套会让证书与 cookie 行为跟页面里的请求不一致，
            // 排查起来非常痛苦。
            DefaultMediaSourceFactory(context)
        )
        .setSeekBackIncrementMs(SEEK_JUMP_MS)
        .setSeekForwardIncrementMs(SEEK_JUMP_MS)
        .build()
        .apply {
            // 蓝牙断开时自动暂停。不设的话耳机关了声音还在外放。
            setHandleAudioBecomingNoisy(true)
        }

    var mediaSession: MediaSession? = null
        private set

    private var view: PlayerView? = null
    private var container: FrameLayout? = null
    /** 上一次上报的状态。`state-change` 事件要带 from，必须自己记。 */
    private var lastState: Contract.PlayerState = Contract.PlayerState.IDLE

    init {
        player.addListener(PlayerListener())
    }

    // ── 图层 ────────────────────────────────────────────────

    /**
     * 把 PlayerView 插到 WebView 之下，并把 WebView 设为透明。
     *
     * **幂等**：重复调用只更新布局，不重建 —— 旋转屏幕时 Activity 不重建
     * （见 manifest 的 configChanges），但容器尺寸会变，所以这里只做重新测量。
     */
    fun attach(activity: android.app.Activity) {
        val decor = activity.findViewById<FrameLayout>(android.R.id.content) ?: return
        if (view == null) {
            val v = PlayerView(context).apply {
                // 控制条我们自己做（Vue），原生的一律关掉。
                useController = false
                setBackgroundColor(Color.BLACK)
                // S1 待定：若真机上出现黑屏/闪屏，改成 texture_view。
                // PlayerView 的 XML 属性 surface_type 在代码里对应这里。
                layoutParams = FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                )
            }
            v.player = player
            view = v
            decor.addView(v, 0)
        }
        container = decor
        makeWebViewTransparent(decor)
    }

    /**
     * 让 WebView 透明，否则画面会被 WebView 的不透明背景挡住。
     *
     * 为什么遍历找而不是按 id 取：Tauri 没给 WebView 设 id，
     * 而按类型找在本应用里不会有第二个 WebView（我们没引广告/地图 SDK）。
     */
    private fun makeWebViewTransparent(root: View) {
        val stack = ArrayList<View>()
        stack.add(root)
        while (stack.isNotEmpty()) {
            val v = stack.removeAt(stack.size - 1)
            if (v is WebView) {
                v.setBackgroundColor(Color.TRANSPARENT)
                // 硬件层是必需的：软件层下透明 WebView 会整块变白。
                v.setLayerType(View.LAYER_TYPE_HARDWARE, null)
                continue
            }
            if (v is ViewGroup) {
                for (i in 0 until v.childCount) stack.add(v.getChildAt(i))
            }
        }
    }

    /** 画面可见性。TV 上弹层时不需要隐藏，手机上进 PiP 要隐藏。 */
    fun setSurfaceVisible(visible: Boolean) {
        view?.visibility = if (visible) View.VISIBLE else View.GONE
    }

    /**
     * 设置通知栏 / 锁屏 / 蓝牙耳机上显示的元数据。
     *
     * **必须显式调用**：ExoPlayer 不会从媒体里读标题，
     * 不设的话通知栏显示的是"未知"，用户根本不知道在播什么。
     *
     * 封面 URL 的 http(s) 校验由 Rust 侧 `session::sanitize` 做过；
     * 这里只负责把合法的值交给 MediaSession。
     */
    fun setMetadata(m: Contract.MetadataArgs) {
        val session = mediaSession ?: MediaSession.Builder(context, player)
            .setId(SESSION_ID)
            .build()
            .also { mediaSession = it }

        session.player = player
        // 只在拿到新的 artwork 时才让 MediaSession 重新加载图片，
        // 否则每次 timeupdate 都会触发一次网络请求。
        val art = m.artworkUrl?.let { runCatching { android.net.Uri.parse(it) }.getOrNull() }
        session.setMediaMetadata(
            androidx.media3.common.MediaMetadata.Builder()
                .setTitle(m.title)
                .setSubtitle(m.subtitle)
                .setArtworkUri(art)
                .setDurationMs(m.durationMs.takeIf { it > 0 })
                .build()
        )
    }

    // ── 载入 ────────────────────────────────────────────────

    fun load(args: Contract.LoadArgs) {
        player.stop()
        player.clearMediaItems()

        val item = when (val src = args.source) {
            is Contract.Source.Hls -> MediaItem.fromUri(src.url)
            is Contract.Source.Progressive -> MediaItem.fromUri(src.url)
        }

        // 外挂字幕。Media3 靠 `MediaItem.SubtitleConfiguration` 挂上去，
        // **必须在 setMediaItem 之前配好**，set 完再 sub-add 是 mpv 的写法，
        // Media3 上不生效（也不报错，就是不显示 —— 这点很容易踩）。
        val withSubs = if (args.externalSubtitles.isEmpty()) {
            item
        } else {
            item.buildUpon()
                .setSubtitleConfigurations(
                    args.externalSubtitles.map { sub ->
                        MediaItem.SubtitleConfiguration.Builder(sub.url.toUriOrEmpty())
                            .setMimeType(sub.mime ?: guessSubtitleMime(sub.url))
                            .setLanguage(sub.language)
                            .setLabel(sub.label)
                            .setSelectionFlags(
                                if (sub.isDefault) C.SELECTION_FLAG_DEFAULT else 0
                            )
                            .build()
                    }
                )
                .build()
        }

        player.setMediaItem(withSubs)
        // 起始位置交给 seek（clamp 由 Rust 侧做过），这里只决定要不要立刻播。
        player.seekTo((args.startAt * 1000).toLong().coerceAtLeast(0))
        player.playWhenReady = args.autoplay
        player.prepare()
    }

    private fun String.toUriOrEmpty(): android.net.Uri =
        android.net.Uri.parse(this)

    /**
     * 猜字幕 MIME。猜不出来返回 SRT —— 宁可猜错也不要留空：
     * 留空会让 Media3 抛 `IllegalArgumentException`（mime 为 null），
     * 猜错最坏是字幕不显示。
     */
    private fun guessSubtitleMime(url: String): String {
        val lower = url.substringBefore('?').lowercase()
        return when {
            lower.endsWith(".vtt") -> MimeTypes.TEXT_VTT
            lower.endsWith(".ass") || lower.endsWith(".ssa") -> MimeTypes.TEXT_SSA
            lower.endsWith(".srt") -> MimeTypes.APPLICATION_SUBRIP
            else -> MimeTypes.APPLICATION_SUBRIP
        }
    }

    // ── 轨道切换 ────────────────────────────────────────────

    /**
     * 选中一条轨道。id 形如 `g{group}:t{index}`。
     *
     * 关字幕（id == null）用的是 `setTrackTypeDisabled`，不是"取消 override" ——
     * 取消 override 会让 ExoPlayer 回到默认选择策略，于是字幕又冒出来。
     */
    fun selectTrack(type: Int, id: String?) {
        val tracks = player.currentTracks
        val params = player.trackSelectionParameters.buildUpon()

        if (id == null) {
            params.setTrackTypeDisabled(type, true)
            player.trackSelectionParameters = params.build()
            return
        }

        val (g, t) = id.splitTrackId() ?: return
        val group = tracks.groups.getOrNull(g) ?: return
        if (group.type != type) return
        params.setTrackTypeDisabled(type, false)
        params.setOverrideForType(
            TrackSelectionOverride(group.mediaTrackGroup, t)
        )
        player.trackSelectionParameters = params.build()
    }

    /** `g2:t5` → (2, 5)。解析不了返回 null。规则由 Rust 侧定义。 */
    private fun String.splitTrackId(): Pair<Int, Int>? {
        val parts = split(':')
        if (parts.size != 2) return null
        val g = parts[0].removePrefix("g").toIntOrNull() ?: return null
        val t = parts[1].removePrefix("t").toIntOrNull() ?: return null
        return g to t
    }

    /**
     * 画质。档位 id 是 `g*:t*`（视频轨），`auto` 表示交还给自适应。
     */
    fun selectQuality(id: String) {
        if (id == AUTO_QUALITY_ID) {
            // 交还给自适应。注意是"清掉 override"，
            // 而不是 override 到某一条 —— 后者等于锁死一个档位。
            player.trackSelectionParameters = player.trackSelectionParameters
                .buildUpon()
                .clearOverridesOfType(C.TRACK_TYPE_VIDEO)
                .build()
            return
        }
        selectTrack(C.TRACK_TYPE_VIDEO, id)
    }

    // ── 状态 ────────────────────────────────────────────────

    fun snapshot(state: Contract.PlayerState, tracks: TracksBundle?): Contract.Snapshot {
        return Contract.Snapshot(
            state = state,
            position = positionSec(),
            duration = durationSec(),
            volume = player.volume.toDouble(),
            muted = player.volume == 0f,
            rate = player.playbackParameters.speed.toDouble(),
            buffered = player.bufferedPosition / 1000.0,
            audioTracks = tracks?.audio ?: emptyList(),
            subtitleTracks = tracks?.text ?: emptyList(),
            qualities = tracks?.qualities ?: emptyList(),
            currentAudioTrackId = tracks?.audioSelectedId,
            currentSubtitleTrackId = tracks?.textSelectedId,
            currentQualityId = tracks?.currentQualityId,
        )
    }

    /** 当前轨道信息。由 `onTracksChanged` 触发时刷新并缓存，避免每次快照都重算。 */
    data class TracksBundle(
        val audio: List<Contract.Track>,
        val text: List<Contract.Track>,
        val qualities: List<Contract.Track>,
        val audioSelectedId: String?,
        val textSelectedId: String?,
        val currentQualityId: String?,
    )

    fun release() {
        mediaSession?.release()
        mediaSession = null
        view?.player = null
        container?.removeView(view)
        view = null
        player.release()
    }

    companion object {
        /** 快进/快退步长。10 秒是流媒体的通行值。 */
        const val SEEK_JUMP_MS = 10_000L

        /**
         * 直播目标延迟。我们服务端只出 VOD 的 HLS，但保底设一个，
         * 免得将来接直播时默认 0 导致频繁卡顿。
         */
        const val LIVE_TARGET_OFFSET_MS = 3_000L

        /** 与 Rust 侧 `protocol::AUTO_QUALITY_ID` 一致。 */
        const val AUTO_QUALITY_ID = "auto"

        /**
         * MediaSession 的 id。
         *
         * **必须稳定**：蓝牙耳机与 Android Auto 靠它认session。
         * 用随机会导致每次播放都新建一个 session，耳机上的曲目信息不更新。
         */
        const val SESSION_ID = "whatmedia.playback"
    }

    // ── ExoPlayer 事件 → 我们的事件 ──────────────────────────

    inner class PlayerListener : Player.Listener {

        override fun onPlaybackStateChanged(playbackState: Int) {
            val state = when (playbackState) {
                Player.STATE_IDLE -> Contract.PlayerState.IDLE
                Player.STATE_BUFFERING -> Contract.PlayerState.BUFFERING
                Player.STATE_READY -> if (player.playWhenReady) {
                    Contract.PlayerState.PLAYING
                } else {
                    Contract.PlayerState.READY
                }
                Player.STATE_ENDED -> Contract.PlayerState.ENDED
                else -> Contract.PlayerState.IDLE
            }
            emitter.emit(Json.stateChange(lastState, state))
            lastState = state
            // 卡顿：STATE_BUFFERING 且不是起播阶段。
            if (playbackState == Player.STATE_BUFFERING && player.playWhenReady) {
                emitter.emit(Json.waiting)
            }
            // 起播与卡顿恢复要分开提示，见前端 types.ts 的说明。
            if (playbackState == Player.STATE_READY && player.playWhenReady) {
                emitter.emit(Json.playing)
            }
        }

        override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
            if (player.playbackState != Player.STATE_READY) return
            if (playWhenReady) return
            // reason == USER_REQUEST 才是"用户按了暂停"；
            // 被 AudioFocus 抢走（蓝牙来电）不算，那种情况不该弹"已暂停"。
            val byUser = reason == Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST
            emitter.emit(Json.paused(byUser))
        }

        override fun onPlayerError(error: PlaybackException) {
            // **只报原始 errorCode，分类交给 Rust**。
            // 在这里写 `if (errorCode == 1008) NETWORK` 就等于把测试覆盖不到的逻辑
            // 放进测试覆盖不到的语言里 —— Rust 侧 `classify_error` 有 8 条测试守着。
            emitter.emit(Json.error(error.errorCode, error.message ?: ""))
        }

        override fun onPositionDiscontinuity(
            oldPosition: Player.PositionInfo,
            newPosition: Player.PositionInfo,
            reason: Int,
        ) {
            // 只在 seek 完成时上报（DISCONTINUITY_REASON_SEEK）。
            // 自动切换自适应档位也会触发 discontinuity，那种不该算"跳转"。
            if (reason == Player.DISCONTINUITY_REASON_SEEK) {
                emitter.emit(Json.timeUpdate(positionSec(), durationSec()))
            }
        }

        override fun onTracksChanged(tracks: androidx.media3.common.Tracks) {
            // 轨道列表变了就通知。Rust 侧会重新生成 label 与 id。
            emitter.emit(Json.tracksListed)
        }
    }

    private fun positionSec(): Double = player.currentPosition / 1000.0

    private fun durationSec(): Double =
        player.duration.let { if (it == C.TIME_UNSET) 0.0 else it / 1000.0 }
}

/**
 * 事件 JSON 的构造。
 *
 * 字段名与 Rust `PlayerEvent` 的 serde 输出一致。
 * 手写 JSON 而不是用 kotlinx.serialization 编码 sealed class，
 * 是因为这里的字段集很小且稳定，序列化器生成 + ProGuard 规则的
 * 维护成本高于直接写字符串。
 */
internal object Json {
    fun stateChange(from: Contract.PlayerState, to: Contract.PlayerState): String =
        """{"type":"state-change","from":"${from.wire}","to":"${to.wire}"}"""

    val playing: String = """{"type":"playing"}"""
    fun paused(byUser: Boolean): String = """{"type":"paused","byUser":$byUser}"""
    val waiting: String = """{"type":"waiting"}"""
    val ended: String = """{"type":"ended"}"""
    val tracksListed: String = """{"type":"tracks-listed"}"""

    fun timeUpdate(position: Double, duration: Double): String =
        """{"type":"time-update","position":$position,"duration":$duration}"""

    fun error(code: Int, message: String): String =
        """{"type":"error","error":{"errorCode":$code,"message":${message.quote()}}}"""

    fun pipChanged(active: Boolean): String =
        """{"type":"pip-changed","active":$active}"""

    fun remoteKey(key: Contract.RemoteKey): String =
        """{"type":"remote-key","key":"${key.wire}"}"""

    fun networkChanged(online: Boolean, kind: Contract.NetworkKind): String =
        """{"type":"network-changed","online":$online,"kind":"${kind.wire}"}"""

    private fun String.quote(): String =
        "\"" + replace("\\", "\\\\").replace("\"", "\\\"") + "\""
}

private val Contract.PlayerState.wire: String
    get() = name.lowercase()

private val Contract.RemoteKey.wire: String
    get() = name.lowercase().replace('_', '-')

private val Contract.NetworkKind.wire: String
    get() = name.lowercase()
