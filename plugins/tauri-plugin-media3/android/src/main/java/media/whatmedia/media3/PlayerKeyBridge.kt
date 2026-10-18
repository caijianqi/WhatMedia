package media.whatmedia.media3

import android.app.PictureInPictureParams
import android.os.Build
import android.util.Rational

/**
 * 遥控器按键的静态入口。
 *
 * **为什么是个 singleton**：`MainActivity` 在 `dispatchKeyEvent` 里要同步问一句
 * "这个键归不归播放器管"，而插件实例是 Tauri 在启动流程里创建的，
 * Activity 拿不到它的引用（也不该拿 —— 那是反过来的依赖方向）。
 * 所以这里做一个静态登记点：插件起来后把自己的处理器注册进来，
 * Activity 只管问，没人注册就返回 false 让系统自己处理。
 */
object PlayerKeyBridge {

    /** 按键 → 是否已被消费。 */
    fun interface Handler {
        /** 返回 true 表示消费掉，不再往系统传。 */
        fun onKey(key: Contract.RemoteKey): Boolean
    }

    fun interface PlayingQuery {
        fun isPlaying(): Boolean
    }

    fun interface PipListener {
        fun onPipChanged(active: Boolean)
    }

    @Volatile private var handler: Handler? = null
    @Volatile private var playingQuery: PlayingQuery? = null
    @Volatile private var pipListener: PipListener? = null

    fun register(h: Handler, q: PlayingQuery, p: PipListener) {
        handler = h
        playingQuery = q
        pipListener = p
    }

    fun unregister() {
        handler = null
        playingQuery = null
        pipListener = null
    }

    /** Activity 的唯一入口。返回 false 时 Activity 会交给 `super.dispatchKeyEvent`。 */
    fun dispatch(key: Contract.RemoteKey): Boolean = handler?.onKey(key) ?: false

    fun isPlaying(): Boolean = playingQuery?.isPlaying() ?: false

    fun onPipChanged(active: Boolean) {
        pipListener?.onPipChanged(active)
    }
}

/**
 * PiP 参数。
 *
 * `aspectRatio` 用 16:9 写死而不是取视频真实比例：
 * 系统会**拒绝**超出 [2.39, 0.42] 区间的比例，而片源里有 4:3 的老片、
 * 也有 2.35:1 的宽银幕。写死 16:9 最省事，视觉上小窗里也看不出差别。
 */
object PipParams {
    fun params(): PictureInPictureParams {
        val builder = PictureInPictureParams.Builder()
            .setAspectRatio(Rational(16, 9))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // 手势导航下进出 PiP 的过渡是否跟手。不给也能用，给了更顺。
            builder.setSeamlessResizeEnabled(true)
        }
        return builder.build()
    }
}
