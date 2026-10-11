<script setup lang="ts">
/**
 * 设置页。
 *
 * 这里展示的是**服务端自检的结果**，不是配置项编辑器。
 * 把硬件探测结果暴露给用户是有意的：当"为什么这个片要转码"发生时，
 * 用户能自己看到"服务端只检测到软件编码"，比去翻日志强得多。
 */
import { onMounted, ref } from 'vue'
import * as api from '../api/client'
import { probeMpv, tauriAvailable, type MpvStatus } from '../player'

const health = ref<api.HealthReport | null>(null)
const hardware = ref<api.HardwareProfileReport | null>(null)
const error = ref<string | null>(null)

/** 桌面端才有的东西：mpv 后端在不在，决定服务端敢不敢给 MKV 直出。 */
const inTauri = tauriAvailable()
const mpv = ref<MpvStatus | null>(null)

onMounted(async () => {
  try {
    const [h, hw] = await Promise.all([api.health(), api.hardwareProfile()])
    health.value = h
    hardware.value = hw
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
  if (inTauri) mpv.value = await probeMpv()
})

function fmtUptime(s: number): string {
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分`
}
</script>

<template>
  <div class="max-w-3xl space-y-6">
    <h1 class="text-2xl font-semibold text-white">设置</h1>

    <p v-if="error" class="rounded bg-red-950/60 px-3 py-2 text-sm text-red-300">
      连不上服务端：{{ error }}
    </p>

    <section v-if="health" class="card p-4">
      <h2 class="mb-3 text-sm font-medium text-slate-300">服务端</h2>
      <dl class="grid grid-cols-2 gap-y-2 text-sm">
        <dt class="text-slate-500">状态</dt>
        <dd class="text-slate-200">{{ health.status }}</dd>
        <dt class="text-slate-500">版本</dt>
        <dd class="font-mono text-slate-200">{{ health.version }}</dd>
        <dt class="text-slate-500">架构</dt>
        <dd class="font-mono text-slate-200">{{ health.arch }} / {{ health.os }}</dd>
        <dt class="text-slate-500">运行时长</dt>
        <dd class="text-slate-200">{{ fmtUptime(health.uptime_secs) }}</dd>
      </dl>
    </section>

    <section v-if="hardware" class="card p-4">
      <h2 class="mb-3 text-sm font-medium text-slate-300">转码能力</h2>

      <p v-if="!hardware.ffmpeg" class="mb-3 rounded bg-amber-950/60 px-3 py-2 text-sm text-amber-300">
        服务端没有检测到 ffmpeg —— 所有需要重新封装或转码的片源都放不了，
        只有浏览器能直解的 MP4/WebM + H264/VP9 + AAC/Opus 可以播。
      </p>

      <dl class="grid grid-cols-2 gap-y-2 text-sm">
        <dt class="text-slate-500">ffmpeg</dt>
        <dd class="break-all font-mono text-xs text-slate-200">
          {{ hardware.ffmpeg ?? '未检测到' }}
        </dd>
        <dt class="text-slate-500">可用后端</dt>
        <dd class="text-slate-200">{{ hardware.profiles.length }}</dd>
        <dt class="text-slate-500">并发上限</dt>
        <dd class="text-slate-200">{{ hardware.capacity }}</dd>
        <dt class="text-slate-500">进行中的转码</dt>
        <dd class="text-slate-200">{{ hardware.active_transcodes }}</dd>
        <dt class="text-slate-500">活跃流</dt>
        <dd class="text-slate-200">{{ hardware.streams }}</dd>
      </dl>

      <div v-if="hardware.profiles.length" class="mt-4">
        <h3 class="mb-2 text-xs text-slate-500">探测到的硬件档</h3>
        <pre class="overflow-x-auto rounded bg-ink-950 p-3 text-xs text-slate-300">{{
          hardware.profiles
        }}</pre>
      </div>
    </section>

    <section v-if="inTauri" class="card p-4">
      <h2 class="mb-3 text-sm font-medium text-slate-300">桌面播放器（mpv）</h2>

      <p
        v-if="mpv && !mpv.available"
        class="mb-3 rounded bg-amber-950/60 px-3 py-2 text-sm text-amber-300"
      >
        {{ mpv.reason ?? '没有找到 mpv' }} —— 已回落 Web 播放器，
        能力也按 Web 上报，服务端不会给出浏览器放不出来的 Direct Play。
      </p>

      <dl class="grid grid-cols-2 gap-y-2 text-sm">
        <dt class="text-slate-500">状态</dt>
        <dd class="text-slate-200">
          {{ mpv ? (mpv.available ? '可用' : '不可用') : '探测中…' }}
        </dd>
        <template v-if="mpv?.available">
          <dt class="text-slate-500">版本</dt>
          <dd class="font-mono text-xs text-slate-200">{{ mpv.version }}</dd>
          <dt class="text-slate-500">路径</dt>
          <dd class="break-all font-mono text-xs text-slate-200">{{ mpv.path }}</dd>
        </template>
      </dl>

      <p class="mt-3 text-xs leading-relaxed text-slate-500">
        mpv 跑在自己的窗口里（不是嵌在页面里）：这样 mpv 出问题只是退化到 Web 播放器，
        不会让整个客户端起不来。控制条仍然是这个页面里的。
      </p>
    </section>

    <section class="card p-4">
      <h2 class="mb-2 text-sm font-medium text-slate-300">关于这个版本</h2>
      <p class="text-sm leading-relaxed text-slate-400">
        前端默认使用 Web 播放器（<code class="text-slate-300">&lt;video&gt;</code> + hls.js）。
        桌面壳里若探测到 mpv 会自动切到 mpv 后端，并把能力按
        <code class="text-slate-300">tauri-mpv</code> 上报，服务端据此决定是否免转码；
        探测不到就按 Web 决策 —— 宁可多转一次码，也不给一个放不出来的地址。
      </p>
    </section>
  </div>
</template>
