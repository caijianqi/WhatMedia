package media.whatmedia.media3

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Build
import app.tauri.App
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import androidx.media3.common.C
import androidx.media3.session.MediaSession
import kotlinx.serialization.json.Json as KJson

/**
 * Media3 播放器插件（Tauri 2）。
 *
 * ## 命令命名
 *
 * 一律 `media3_*`，与 Rust 侧 `plugins/tauri-plugin-media3/src/lib.rs` 注册的
 * `#[tauri::command]` 名一一对应。Rust 侧只是转发，所以**真正实现在这里** ——
 * 改名必须两边同时改，漏了的表现是前端 `invoke` 报 "command not found"。
 *
 * ## ⚠️ Tauri API 面需要装好 SDK 后校对
 *
 * 本文件的 `app.tauri.*` 调用是按 Tauri 2 的插件约定写的，
 * 但开发环境没有 Android SDK，**一行都没编译过**。
 * 装好 SDK 后请优先校对这些点：
 *   1. `@TauriPlugin` 需要 KSP 插件，KSP 版本要与 Kotlin 版本匹配；
 *   2. `invoke.parseArgs<T>()` 的泛型签名（有的版本是 `parseArgs(Class<T>)`）；
 *   3. `Channel` 的获取方式（有的版本从 `invoke.message` 里取）；
 *   4. `Plugin.app` 的类型是 `App`，`app.activity` 在某些版本上叫 `activity`。
 * 校对清单同步记录在 `docs/adr/0005`。
 */
@TauriPlugin
internal class Media3Plugin(private val app: App) : Plugin(app) {

    private val json = KJson { ignoreUnknownKeys = true }

    override fun load(webview: android.webkit.WebView) {
        super.load(webview)
        Media3Bridge.attach(app.activity, this)
        Media3Bridge.startNetwork(app.activity.applicationContext) { online, kind ->
            emit(Json.networkChanged(online, kind))
        }
        // 遥控器按键：注册进静态登记点，MainActivity 会来问。
        PlayerKeyBridge.register(
            handler = { key -> Media3Bridge.onRemoteKey(key) },
            playingQuery = { Media3Bridge.isPlaying() },
            pipListener = { active -> emit(Json.pipChanged(active)) },
        )
    }

    // ── 命令 ────────────────────────────────────────────────

    @Command
    fun media3_event_channel(invoke: Invoke) {
        // 前端订阅事件用的通道。Rust 侧用 `Channel` 接收后转 emit 到 TS。
        // 参数名 `onEvent` 与前端 `media3.ts` 里的调用一致。
        val channel: Channel? = runCatching { invoke.getChannel("onEvent") }.getOrNull()
        Media3Bridge.setChannel(channel)
        invoke.resolve(JSObject().apply { put("ok", true) })
    }

    @Command
    fun media3_load(invoke: Invoke) {
        val args = invoke.parseArgsOrNull<Contract.LoadArgs>() ?: return invoke.rejectBadArgs()
        Media3Bridge.load(args)
        invoke.resolve()
    }

    @Command
    fun media3_play(invoke: Invoke) = Media3Bridge.play().let { invoke.resolve() }

    @Command
    fun media3_pause(invoke: Invoke) = Media3Bridge.pause().let { invoke.resolve() }

    @Command
    fun media3_stop(invoke: Invoke) = Media3Bridge.stop().let { invoke.resolve() }

    @Command
    fun media3_destroy(invoke: Invoke) = Media3Bridge.destroy().let { invoke.resolve() }

    @Command
    fun media3_seek(invoke: Invoke) {
        val seconds = invoke.getDouble("position") ?: return invoke.rejectBadArgs()
        Media3Bridge.seek(seconds)
        invoke.resolve()
    }

    @Command
    fun media3_set_volume(invoke: Invoke) {
        val v = invoke.getDouble("volume") ?: return invoke.rejectBadArgs()
        Media3Bridge.setVolume(v.toFloat().coerceIn(0f, 1f))
        invoke.resolve()
    }

    @Command
    fun media3_set_muted(invoke: Invoke) {
        Media3Bridge.setVolume(if (invoke.getBoolean("muted") == true) 0f else 1f)
        invoke.resolve()
    }

    @Command
    fun media3_set_rate(invoke: Invoke) {
        val r = invoke.getDouble("rate") ?: return invoke.rejectBadArgs()
        Media3Bridge.setRate(r.toFloat().coerceIn(MIN_RATE, MAX_RATE))
        invoke.resolve()
    }

    @Command
    fun media3_select_audio(invoke: Invoke) {
        Media3Bridge.selectTrack(C.TRACK_TYPE_AUDIO, invoke.getString("id"))
        invoke.resolve()
    }

    @Command
    fun media3_select_subtitle(invoke: Invoke) {
        // id 为 null 表示关闭字幕。JS 传 null 时 getString 返回 null，正合用。
        Media3Bridge.selectTrack(C.TRACK_TYPE_TEXT, invoke.getString("id"))
        invoke.resolve()
    }

    @Command
    fun media3_select_quality(invoke: Invoke) {
        Media3Bridge.selectQuality(invoke.getString("id") ?: PlayerHolder.AUTO_QUALITY_ID)
        invoke.resolve()
    }

    @Command
    fun media3_set_metadata(invoke: Invoke) {
        val m = invoke.parseArgsOrNull<Contract.MetadataArgs>() ?: return invoke.rejectBadArgs()
        Media3Bridge.setMetadata(m)
        invoke.resolve()
    }

    @Command
    fun media3_set_pip(invoke: Invoke) {
        if (invoke.getBoolean("enabled") == true) Media3Bridge.enterPip(app.activity)
        invoke.resolve()
    }

    /**
     * 设备形态。前端拿到它决定用哪套 UI。
     *
     * **不能让前端自己猜**：UA / 屏幕尺寸都区分不了 TV 与手机
     * （TV 也是"小屏 + 无鼠标"）。猜错的后果是 TV 用户拿到触摸 UI，
     * 而遥控器不产生触摸事件 → 完全无法操作，且没有任何报错。
     */
    @Command
    fun media3_device_form(invoke: Invoke) {
        val fm = app.activity?.packageManager
        val isTv = fm?.hasSystemFeature(android.content.pm.PackageManager.FEATURE_LEANBACK) == true ||
            fm?.hasSystemFeature("android.hardware.type.television") == true
        val isTouch = fm?.hasSystemFeature(android.content.pm.PackageManager.FEATURE_TOUCHSCREEN) == true
        val form = when {
            isTv -> "tv"
            isTouch -> "handheld"
            else -> "unknown"
        }
        invoke.resolve(JSObject().apply { put("form", form) })
    }

    /**
     * 取播放器的原始状态。
     *
     * **只给原值，不给成品快照**。轨道的 label 与 id 由 Rust 侧生成
     * （`crates/media3` 的 `to_tracks` / `to_qualities`），
     * 所以前端拿到这里的结果后还要再过一道 `plugin:media3|media3_snapshot`。
     *
     * 看着绕了一趟，但把"标签怎么取"留在一个能被测试覆盖的地方，
     * 比在 Kotlin 里顺手拼一个字符串划算得多。
     */
    @Command
    fun media3_status(invoke: Invoke) {
        val ret = JSObject()
        ret.put("state", Media3Bridge.stateName())
        ret.put("tracks", Media3Bridge.rawTracksJson())
        invoke.resolve(ret)
    }

    /** 事件出口：通道存在就发，不存在就丢弃（前端还没订阅）。 */
    private fun emit(json: String) {
        Media3Bridge.emit(json)
    }

    private fun Invoke.rejectBadArgs() {
        reject("media3: 参数缺失或类型不对")
    }

    private inline fun <reified T> Invoke.parseArgsOrNull(): T? =
        runCatching {
            val raw = this.message.getString("") ?: return null
            json.decodeFromString<T>(raw)
        }.getOrNull()

    companion object {
        // 速率上下限。ExoPlayer 对超出范围的速率会抛异常，
        // 而前端拖动条很容易给出 0 或 4。
        const val MIN_RATE = 0.25f
        const val MAX_RATE = 4.0f
    }
}

/**
 * 插件与 Service / Activity 之间的共享状态。
 *
 * 存在的理由和 `PlayerKeyBridge` 一样：`PlaybackService` 由系统创建，
 * 拿不到插件实例；`MainActivity` 也不该持有插件实例。
 * 所以真正的播放器实例放在这里，由插件初始化、被 Service 查询。
 */
internal object Media3Bridge {

    private var holder: PlayerHolder? = null
    private var channel: Channel? = null
    private var monitor: NetworkMonitor? = null
    private var activity: Activity? = null
    private var plugin: Plugin? = null

    fun attach(act: Activity?, owner: Plugin) {
        if (act == null) return
        activity = act
        plugin = owner
        if (holder == null) {
            holder = PlayerHolder(act.applicationContext) { emit(it) }
        }
        holder?.attach(act)
    }

    fun startNetwork(context: Context, onChange: (Boolean, Contract.NetworkKind) -> Unit) {
        monitor?.stop()
        monitor = NetworkMonitor(context, onChange).also { it.start() }
    }

    fun setChannel(ch: Channel?) {
        channel = ch
    }

    fun emit(json: String) {
        // Tauri 的 Channel 只吃 JSObject，所以把整条 JSON 当一个字符串字段塞进去，
        // Rust 侧再解出来。看着绕，但避免了在 Kotlin 侧手写一整套事件 data class
        // （那套代码没有测试覆盖，不值得）。
        val obj = JSObject()
        obj.put("event", json)
        runCatching { channel?.send(obj) }
    }

    fun session(): MediaSession? = holder?.mediaSession

    fun isPlaying(): Boolean = holder?.player?.isPlaying == true

    fun load(args: Contract.LoadArgs) {
        holder?.load(args)
    }

    fun play() {
        holder?.player?.play()
    }

    fun pause() {
        holder?.player?.pause()
    }

    fun stop() {
        holder?.player?.stop()
    }

    fun destroy() {
        holder?.release()
        holder = null
        monitor?.stop()
        monitor = null
    }

    fun stopForTaskRemoval() {
        // 从最近任务划掉：必须真正停。见 PlaybackService.onTaskRemoved。
        holder?.player?.stop()
        holder?.player?.clearMediaItems()
    }

    fun seek(seconds: Double) {
        // clamp 由 Rust 侧 clamp_seek 做过；这里只防负数与非法值。
        val ms = (seconds * 1000).toLong().coerceAtLeast(0)
        holder?.player?.seekTo(ms)
    }

    fun setVolume(v: Float) {
        holder?.player?.volume = v
    }

    fun setRate(r: Float) {
        holder?.player?.setPlaybackSpeed(r)
    }

    fun selectTrack(type: Int, id: String?) {
        holder?.selectTrack(type, id)
    }

    fun selectQuality(id: String) {
        holder?.selectQuality(id)
    }

    fun setMetadata(m: Contract.MetadataArgs) {
        holder?.setMetadata(m)
    }

    fun enterPip(act: Activity?) {
        if (act == null) return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        runCatching { act.enterPictureInPictureMode(PipParams.params()) }
    }

    fun onRemoteKey(key: Contract.RemoteKey): Boolean {
        val h = holder ?: return false
        when (key) {
            Contract.RemoteKey.PLAY -> h.player.play()
            Contract.RemoteKey.PAUSE -> h.player.pause()
            Contract.RemoteKey.TOGGLE -> if (h.player.isPlaying) h.player.pause() else h.player.play()
            Contract.RemoteKey.SEEK_FORWARD -> h.player.seekForward()
            Contract.RemoteKey.SEEK_BACKWARD -> h.player.seekBack()
            Contract.RemoteKey.STOP -> stop()
            else -> return false // 方向键 / 返回 / 菜单交给前端的焦点系统
        }
        return true
    }

    /** 原始轨道。label 由 Rust 侧生成，这里只给原值。 */
    fun rawTracksJson(): String {
        val tracks = holder?.player?.currentTracks
        val raw = tracks?.let {
            RawTracksPayload(
                audio = it.rawAudioTracks(),
                text = it.rawTextTracks(),
                video = it.rawVideoTracks(),
            )
        }
        return KJson.encodeToString(raw ?: RawTracksPayload())
    }

    /** 当前播放器状态的名字。线格式与 Rust `PlayerState` 一致（小写）。 */
    fun stateName(): String {
        val p = holder?.player ?: return "idle"
        return when (p.playbackState) {
            androidx.media3.common.Player.STATE_IDLE -> "idle"
            androidx.media3.common.Player.STATE_BUFFERING -> "buffering"
            androidx.media3.common.Player.STATE_ENDED -> "ended"
            androidx.media3.common.Player.STATE_READY ->
                if (p.playWhenReady) "playing" else "ready"
            else -> "idle"
        }
    }

    /** 启动后台服务。第一次播放时调用。 */
    fun ensureService(context: Context) {
        val intent = Intent(context, PlaybackService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(intent)
        } else {
            context.startService(intent)
        }
    }
}
