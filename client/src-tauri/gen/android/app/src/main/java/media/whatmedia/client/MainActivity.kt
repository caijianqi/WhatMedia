package media.whatmedia.client

import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.KeyEvent
import androidx.activity.ComponentActivity
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/**
 * 入口 Activity。
 *
 * Tauri 2 要求是 `AppCompatActivity` 的子类（模板默认继承它）。
 * 这里继承 `ComponentActivity` 而不是 AppCompatActivity —— 我们不用 ActionBar，
 * 少一层依赖就少一个与 Material 主题打架的地方。
 *
 * 三件事在这里做，其余全在插件里：
 *   1. 边到边（edge-to-edge）+ 安全区 → 交给前端的 `useSafeArea`
 *   2. 设备形态（手机 / TV）→ 启动时一次性推给前端，前端据此选 UI 形态
 *   3. 遥控器按键 → 转成事件转发给前端（TV 的 D-Pad 在 WebView 里有聚焦怪癖，
 *      完全交给 DOM focus 会丢键，所以关键几个键在这里拦下）
 */
class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        // 旋转时不重建 Activity。配合 manifest 里的 configChanges，
        // 这是"横屏继续播"的另一半 —— 少了它前面那段配置等于没写。
        super.onCreate(null)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        enableImmersiveWhenPlaying()
        Log.i(TAG, "device form=${deviceForm()}")
    }

    /**
     * 设备形态。一次判定，终身不变（除非外接显示器），
     * 所以不必监听 —— 但 TV 与手机的 **UI 形态完全不同**，
     * 前端必须一开始就知道，不能等拿到第一屏数据才切，那会闪一下桌面版布局。
     */
    private fun deviceForm(): String {
        val pm = packageManager
        val isTv = pm.hasSystemFeature(PackageManager.FEATURE_LEANBACK) ||
            pm.hasSystemFeature("android.hardware.type.television")
        val isTouch = pm.hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN)
        return when {
            isTv -> "tv"
            isTouch -> "handheld"
            else -> "unknown"
        }
    }

    /** 沉浸模式：播放时隐藏系统栏。非播放态不调，否则用户没法退出应用。 */
    private fun enableImmersiveWhenPlaying() {
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    /**
     * 遥控器按键。
     *
     * **为什么不让 WebView 自己处理**：Android WebView 对 D-Pad 的处理是
     * "移动 DOM 焦点"，但 Tauri 的页面里大量元素是 div（不可聚焦），
     * 结果就是按方向键没反应。所以这里把四个方向键 + 确认 + 返回 +
     * 播放暂停 + 快进快退 全部拦下，转成事件推给前端的 TV 焦点管理器。
     *
     * 返回键的处理有讲究：`KEYCODE_BACK` 在 TV 上是"退出上一层"，
     * 直接交给系统会把整个 Activity finish 掉 —— 用户只是想关掉一个弹窗。
     * 所以交给前端决定：前端在弹层里就消费掉，否则才让系统处理。
     */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_DOWN) {
            val mapped = mapKey(event.keyCode) ?: return super.dispatchKeyEvent(event)
            // 交给插件转发（插件持有 WebView 的事件通道）。
            if (PlayerKeyBridge.dispatch(mapped)) return true
        }
        return super.dispatchKeyEvent(event)
    }

    private fun mapKey(code: Int): String? = when (code) {
        KeyEvent.KEYCODE_DPAD_UP -> "up"
        KeyEvent.KEYCODE_DPAD_DOWN -> "down"
        KeyEvent.KEYCODE_DPAD_LEFT -> "left"
        KeyEvent.KEYCODE_DPAD_RIGHT -> "right"
        KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER -> "select"
        KeyEvent.KEYCODE_BACK -> "back"
        KeyEvent.KEYCODE_MENU -> "menu"
        KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE -> "toggle"
        KeyEvent.KEYCODE_MEDIA_PLAY -> "play"
        KeyEvent.KEYCODE_MEDIA_PAUSE -> "pause"
        KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> "seek-forward"
        KeyEvent.KEYCODE_MEDIA_REWIND -> "seek-backward"
        KeyEvent.KEYCODE_MEDIA_NEXT -> "next"
        KeyEvent.KEYCODE_MEDIA_PREVIOUS -> "previous"
        KeyEvent.KEYCODE_MEDIA_STOP -> "stop"
        else -> null
    }

    /**
     * 画中画。
     *
     * 只有 Android 8+ 支持，且**必须在用户按 Home 时（onUserLeaveHint）进入**，
     * 不能在播放开始就进 —— 那会让整个应用缩成小窗。
     */
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && PlayerKeyBridge.isPlaying()) {
            enterPictureInPictureMode(PictureInPictureParamsHolder.params())
        }
    }

    override fun onPictureInPictureModeChanged(
        isInPictureInPictureMode: Boolean,
        newConfig: Configuration,
    ) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        // PiP 里必须隐藏控制条 UI，否则小窗里全是控件。
        PlayerKeyBridge.onPipChanged(isInPictureInPictureMode)
    }

    private companion object {
        const val TAG = "WhatMedia"
    }
}
