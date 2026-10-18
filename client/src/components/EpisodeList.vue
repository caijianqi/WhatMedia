<script setup lang="ts">
/**
 * 剧集的季 / 集列表（详情页用）。
 *
 * 两条不那么显然但会挨骂的设计点：
 *
 * 1. **集是一次性取完，不做分页**。一部剧一季通常 12~24 集，
 *    为它做一个"加载更多"按钮是纯粹的仪式感 —— 用户想找的是某一集，
 *    翻页只会让他多点一次。上限取 500，超过这个数的季现实里不存在。
 * 2. **默认选中第一季，而不是"当前集所在季"**。详情页传给这里的只有剧集 id，
 *    没有"你在看第几集"。与其猜错把用户带到别的季，不如稳定地落在第一季。
 */
import { computed, onMounted, ref, watch } from 'vue'
import * as api from '../api/client'
import type { EpisodeSummary, SeasonSummary } from '../api/client'

const props = defineProps<{
  seriesId: string
}>()

const seasons = ref<SeasonSummary[]>([])
const activeSeason = ref<string>('')
const episodes = ref<EpisodeSummary[]>([])
const loading = ref(true)
const listLoading = ref(false)
const error = ref<string | null>(null)

const activeNumber = computed(
  () => seasons.value.find((s) => s.id === activeSeason.value)?.number ?? 1,
)

async function loadSeasons(): Promise<void> {
  loading.value = true
  error.value = null
  try {
    seasons.value = await api.seriesSeasons(props.seriesId)
    activeSeason.value = seasons.value[0]?.id ?? ''
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

async function loadEpisodes(): Promise<void> {
  if (!activeSeason.value) {
    episodes.value = []
    return
  }
  listLoading.value = true
  try {
    const p = await api.listEpisodes({ season_id: activeSeason.value, page_size: 500 })
    episodes.value = p.data
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    listLoading.value = false
  }
}

onMounted(async () => {
  await loadSeasons()
  await loadEpisodes()
})

watch(activeSeason, () => void loadEpisodes())
watch(() => props.seriesId, () => void loadSeasons())

/** 没拆出季的剧（目录不规整时会有）：给一个明确的说明，而不是空白。 */
const noSeasons = computed(() => !loading.value && seasons.value.length === 0)
</script>

<template>
  <section>
    <h2 class="section-title mb-4">剧集</h2>

    <p v-if="error" class="text-sm text-red-400">{{ error }}</p>
    <div v-else-if="loading" class="skeleton h-24" />
    <p v-else-if="noSeasons" class="text-sm text-wm-muted">
      这部剧还没有拆出季。重新扫描（目录结构规整后会按季集归类）就会出现在这里。
    </p>

    <template v-else>
      <!-- 季 tab。集数直接写在标签上，免得用户点进去才知道有几集。 -->
      <div class="mb-4 flex flex-wrap gap-2">
        <button
          v-for="s in seasons"
          :key="s.id"
          type="button"
          class="focus-ring rounded-full border px-3 py-1.5 text-sm transition duration-200 ease-smooth"
          :class="
            s.id === activeSeason
              ? 'border-brand/40 bg-brand/15 text-white'
              : 'border-wm-line-strong bg-ink-850/60 text-wm-muted hover:border-wm-line hover:text-wm-sub'
          "
          :aria-pressed="s.id === activeSeason"
          @click="activeSeason = s.id"
        >
          第 {{ s.number }} 季
          <span class="ml-1 text-xs text-wm-muted">{{ s.episode_count }} 集</span>
        </button>
      </div>

      <div v-if="listLoading" class="space-y-2">
        <div v-for="n in 6" :key="n" class="skeleton h-16" />
      </div>

      <ol v-else class="space-y-2">
        <li v-for="ep in episodes" :key="ep.id">
          <RouterLink
            :to="{ name: 'play', params: { id: ep.item_id } }"
            class="focus-ring card-hover flex items-center gap-4 p-3.5"
          >
            <!-- 集号。等宽数字让 8 和 10 对齐，否则一列数字会显得参差。 -->
            <span
              class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-ink-800 text-sm font-semibold tabular-nums text-wm-sub"
            >
              {{ ep.episode_number }}
            </span>
            <span class="min-w-0 flex-1">
              <span class="block truncate text-sm text-wm-sub">
                {{ ep.title || `第 ${ep.episode_number} 集` }}
              </span>
              <span class="mt-0.5 block text-xs text-wm-muted">
                第 {{ activeNumber }} 季 · 第 {{ ep.episode_number }} 集
              </span>
            </span>
            <svg
              viewBox="0 0 24 24"
              class="h-4 w-4 shrink-0 fill-slate-500"
              aria-hidden="true"
            >
              <path d="M8 5.5 19 12 8 18.5z" />
            </svg>
          </RouterLink>
        </li>
      </ol>
    </template>
  </section>
</template>
