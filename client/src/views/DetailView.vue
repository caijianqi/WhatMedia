<script setup lang="ts">
/**
 * 条目详情页。
 *
 * **这里只做决策预览（`decidePlayback`），不建会话。**
 * 建会话有副作用 —— 会启动 ffmpeg 转码任务。
 * 如果详情页一打开就建会话，那用户只是点进来看看也会在服务端留下一路转码。
 * 真正的会话在播放器页创建。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import * as api from '../api/client'
import type { DecideView, ItemDetail } from '../api/client'

const route = useRoute()
const id = computed(() => String(route.params.id))

const detail = ref<ItemDetail | null>(null)
const decision = ref<DecideView | null>(null)
const resumeMs = ref<number | null>(null)
const loading = ref(true)
const error = ref<string | null>(null)

async function load() {
  loading.value = true
  error.value = null
  try {
    const [d, dec, res] = await Promise.all([
      api.getItem(id.value),
      api.decidePlayback({ item_id: id.value, client: 'web' }),
      api.resumeList(50).catch(() => [] as api.ResumePoint[]),
    ])
    detail.value = d
    decision.value = dec
    resumeMs.value = res.find((r) => r.item_id === id.value)?.position_ms ?? null
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

onMounted(load)
watch(id, load)

const backdrop = computed(() => `/api/v1/items/${id.value}/images/backdrop`)

const strategyLabel: Record<string, string> = {
  direct_play: '直接播放',
  remux: '重新封装',
  transcode: '需要转码',
  unsupported: '无法播放',
}

const strategyTone: Record<string, string> = {
  direct_play: 'bg-emerald-950 text-emerald-300',
  remux: 'bg-sky-950 text-sky-300',
  transcode: 'bg-amber-950 text-amber-300',
  unsupported: 'bg-red-950 text-red-300',
}

function fmtMs(ms?: number | null): string {
  if (!ms) return '—'
  const total = Math.round(ms / 1000)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分`
}
</script>

<template>
  <div>
    <p v-if="error" class="rounded bg-red-950/60 px-3 py-2 text-sm text-red-300">{{ error }}</p>

    <div v-else-if="loading" class="space-y-4">
      <div class="h-64 animate-pulse rounded-lg bg-ink-850" />
      <div class="h-6 w-1/3 animate-pulse rounded bg-ink-850" />
    </div>

    <div v-else-if="detail">
      <div class="relative -mx-4 -mt-4 h-56 overflow-hidden sm:h-72">
        <img :src="backdrop" alt="" class="h-full w-full object-cover opacity-40" />
        <div class="absolute inset-0 bg-gradient-to-t from-ink-950 to-transparent" />
      </div>

      <div class="mt-4 flex flex-wrap items-end gap-4">
        <div class="min-w-0 flex-1">
          <h1 class="text-2xl font-semibold text-white">{{ detail.title }}</h1>
          <p class="mt-1 text-sm text-slate-400">
            <span v-if="detail.original_title">{{ detail.original_title }} · </span>
            {{ detail.year ?? '年份未知' }}
            <span v-if="detail.runtime_ms"> · {{ fmtMs(detail.runtime_ms) }}</span>
            <span v-if="detail.rating"> · 评分 {{ detail.rating.toFixed(1) }}</span>
          </p>
        </div>

        <div class="flex items-center gap-2">
          <RouterLink
            :to="{ name: 'play', params: { id: detail.id }, query: { start: resumeMs ?? 0 } }"
            class="focus-ring rounded bg-brand px-5 py-2.5 font-medium text-white transition hover:bg-brand-soft"
          >
            {{ resumeMs ? '继续播放' : '播放' }}
          </RouterLink>
          <RouterLink
            v-if="resumeMs"
            :to="{ name: 'play', params: { id: detail.id }, query: { start: 0 } }"
            class="focus-ring rounded bg-ink-800 px-4 py-2.5 text-sm text-slate-200 hover:bg-ink-700"
          >
            从头播放
          </RouterLink>
        </div>
      </div>

      <!-- 播放策略预览 -->
      <div v-if="decision" class="card mt-5 p-4">
        <div class="flex flex-wrap items-center gap-3">
          <span
            class="rounded px-2 py-1 text-xs font-medium"
            :class="strategyTone[decision.strategy] ?? 'bg-ink-800 text-slate-300'"
          >
            {{ strategyLabel[decision.strategy] ?? decision.strategy }}
          </span>
          <span class="text-sm text-slate-300">{{ decision.reason }}</span>
        </div>

        <ul v-if="decision.reasons.length" class="mt-3 space-y-1">
          <li
            v-for="r in decision.reasons"
            :key="r.code"
            class="text-xs text-slate-500"
          >
            <code class="text-slate-400">{{ r.code }}</code> {{ r.message }}
          </li>
        </ul>

        <p v-if="decision.strategy === 'unsupported'" class="mt-3 text-sm text-red-300">
          这个片源在当前客户端上放不了。换桌面端（mpv）通常可以直出。
        </p>
      </div>

      <p v-if="detail.overview" class="mt-5 max-w-3xl leading-relaxed text-slate-300">
        {{ detail.overview }}
      </p>

      <!-- 版本与文件 -->
      <section v-if="detail.versions.length" class="mt-8">
        <h2 class="mb-3 text-lg font-semibold text-slate-100">文件</h2>
        <div class="space-y-3">
          <div
            v-for="v in detail.versions"
            :key="v.id"
            class="card p-3"
          >
            <div class="flex flex-wrap items-center gap-2 text-sm">
              <span class="font-medium text-slate-200">{{ v.name }}</span>
              <span v-if="v.resolution" class="rounded bg-ink-800 px-2 py-0.5 text-xs text-slate-300">
                {{ v.resolution }}
              </span>
              <span v-if="v.hdr_type !== 'sdr'" class="rounded bg-ink-800 px-2 py-0.5 text-xs text-amber-300">
                {{ v.hdr_type }}
              </span>
              <span v-if="v.is_default" class="text-xs text-slate-500">默认</span>
            </div>

            <div v-for="f in v.files" :key="f.id" class="mt-2 text-xs text-slate-500">
              <div class="truncate">{{ f.path }}</div>
              <div class="mt-1 flex flex-wrap gap-3">
                <span>{{ (f.size_bytes / 1024 ** 3).toFixed(2) }} GB</span>
                <span v-if="f.duration_ms">{{ fmtMs(f.duration_ms) }}</span>
                <span :class="f.is_available ? 'text-emerald-400' : 'text-red-400'">
                  {{ f.is_available ? '在线' : '离线' }}
                </span>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>
