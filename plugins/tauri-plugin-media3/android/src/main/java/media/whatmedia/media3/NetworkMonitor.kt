package media.whatmedia.media3

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build

/**
 * 网络状态监听。P6 DoD 最后一条："网络切换（WiFi ↔ 4G）不崩溃，可恢复播放"。
 *
 * ## 为什么必须自己监听
 *
 * ExoPlayer 断网时只会抛一个 `ERROR_CODE_IO_NETWORK_CONNECTION_FAILED` 然后进入 STATE_IDLE。
 * 它**不会**自己重连 —— 网络恢复后播放器还是死的，用户得退出去重新点一遍。
 * 所以必须有人在网络回来时说一句"可以重连了"。
 *
 * ## 用 `registerDefaultNetworkCallback` 而不是广播
 *
 * `CONNECTIVITY_ACTION` 广播在 Android 7+ 上对静态注册无效、
 * 在 Android 10+ 上还带节流。用 ConnectivityManager 的回调是唯一可靠的方式。
 *
 * ## 只报"能上网的网络"
 *
 * `NetworkCapabilities.NET_CAPABILITY_INTERNET` 不等于"能上网" ——
 * 连上一个需要登录的 WiFi 也有这个能力，但实际请求全失败。
 * 所以还要 `NET_CAPABILITY_VALIDATED`（系统确认过真的能通）。
 * 少了这个判断，结果是"刚连上 WiFi 就重连，然后又失败，反复重试"。
 */
internal class NetworkMonitor(
    context: Context,
    private val onChange: (online: Boolean, kind: Contract.NetworkKind) -> Unit,
) {

    private val manager =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            report(network)
        }

        override fun onLost(network: Network) {
            // 丢了一个网络不代表断网（可能还有另一个活着），
            // 所以重新查一次当前默认网络，而不是直接报 offline。
            report(null)
        }

        override fun onCapabilitiesChanged(
            network: Network,
            capabilities: NetworkCapabilities,
        ) {
            // 从"未验证"变成"已验证"也走这里 —— 这正是"刚连上 WiFi"的那一刻。
            report(network)
        }
    }

    fun start() {
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        manager.registerDefaultNetworkCallback(callback)
        // 立刻报一次当前状态：应用可能在已经联网/已经断网的情况下启动，
        // 等下一次回调可能要很久。
        report(null)
    }

    fun stop() {
        runCatching { manager.unregisterNetworkCallback(callback) }
    }

    private fun report(network: Network?) {
        val caps = if (network != null) {
            manager.getNetworkCapabilities(network)
        } else {
            manager.getNetworkCapabilities(manager.activeNetwork ?: return reportDown())
        }

        if (caps == null) return reportDown()

        val validated = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
        } else {
            true
        }
        if (!validated) {
            // 连上了但还没通过验证：不报 online，也不报 offline ——
            // 报 offline 会让 UI 弹出"网络已断开"，几秒后又变回来，闪一下很难看。
            return
        }

        val kind = when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> Contract.NetworkKind.WIFI
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> Contract.NetworkKind.CELLULAR
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> Contract.NetworkKind.ETHERNET
            else -> Contract.NetworkKind.UNKNOWN
        }
        onChange(true, kind)
    }

    private fun reportDown() {
        onChange(false, Contract.NetworkKind.NONE)
    }
}
