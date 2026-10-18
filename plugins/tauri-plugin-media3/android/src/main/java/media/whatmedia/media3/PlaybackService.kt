package media.whatmedia.media3

import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService

/**
 * 后台播放服务。
 *
 * ## 为什么必须有它
 *
 * 没有 MediaSessionService 时，播放器挂在 Activity 上。用户按 Home 或锁屏，
 * Activity 进入后台 → 系统在几十秒内就会把播放停掉（前台服务限制）。
 * 表现是"锁屏后播十几秒就没声了"，而且日志里什么都没有。
 *
 * ## 为什么用 MediaSessionService 而不是自己写 Service + startForeground
 *
 * MediaSessionService 会自动处理通知栏、MediaSession 与前台服务三者的生命周期，
 * 自己写要处理 Android 12 的前台服务启动限制、Android 13 的通知权限、
 * Android 14 的 foregroundServiceType，任一处漏掉都是"在某些机型上后台播放失效"。
 *
 * ## 一个容易踩的点
 *
 * **MediaSession 必须在 Service 里创建，且必须在 `onCreate` 之后**。
 * 在 Activity 里创建再传给 Service 会导致 Controller 连不上，
 * 症状是通知栏出来了但按播放键没反应。
 */
internal class PlaybackService : MediaSessionService() {

    override fun onCreate() {
        super.onCreate()
        // 注意：这里**不**建 player。player 由 PlayerHolder 持有，
        // 通过 Media3Bridge 注册进来。Service 只负责"活着"。
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? =
        Media3Bridge.session()

    override fun onDestroy() {
        // 服务被系统回收时不要 release player —— 用户可能只是退到了后台，
        // 系统随时可能把服务杀掉再拉起，release 掉就再也回不来了。
        super.onDestroy()
    }

    override fun onTaskRemoved(rootIntent: android.content.Intent?) {
        super.onTaskRemoved(rootIntent)
        // 用户从最近任务里划掉应用：应当真正停止播放并结束服务。
        // 不处理的话会留下一个"看不见但还在响"的播放器。
        Media3Bridge.stopForTaskRemoval()
        stopSelf()
    }
}
