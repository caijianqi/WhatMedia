<script setup lang="ts">
/**
 * 我的收藏（FavoritesView）。
 * 遵循 ui.html 原生规范：
 * - .page-header 与 .section-title
 * - 标准 6 列 .library-grid 卡片网格
 * - 纯净原生空状态 .empty-state
 */
import { onMounted, ref } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import * as api from '../api/client'
import EmptyState from '../components/EmptyState.vue'

const router = useRouter()
const rows = ref<api.FavoriteRow[]>([])
const loading = ref(true)
const failed = ref<string | null>(null)

async function load(): Promise<void> {
  loading.value = true
  failed.value = null
  try {
    const r = await api.listMyFavorites(1, 100)
    rows.value = r.data
  } catch (e) {
    failed.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

onMounted(load)

const KIND_LABEL: Record<string, string> = {
  movie: '电影',
  series: '电视剧',
  episode: '单集',
  anime: '动画',
  documentary: '纪录片',
  variety: '综艺',
  music: '音乐',
  musicvideo: 'MV',
  audiobook: '有声书',
  photo: '照片',
  livetv: '直播',
  mixed: '混合',
  other: '其它',
}
</script>

<template>
  <div>
    <div class="page-header">
      <div>
        <h1>我的收藏</h1>
        <p>收藏是每人一份的 —— 家里其他人看不到你收了什么。</p>
      </div>
    </div>

    <!-- 加载骨架 -->
    <div v-if="loading" class="library-grid">
      <div v-for="n in 6" :key="n" class="skeleton-poster" />
    </div>

    <EmptyState
      v-else-if="failed"
      kind="error"
      :hint="failed ?? ''"
      action="重试"
      @action="load"
    />

    <EmptyState
      v-else-if="rows.length === 0"
      kind="empty"
      title="还没有收藏任何东西"
      hint="在影片详情页点那颗心形按钮，它就会出现在这里。"
      alt="去逛媒体库"
      @alt="router.push({ name: 'library' })"
    />

    <!-- 原生 6 列网格展示收藏卡片 -->
    <div v-else class="library-grid">
      <RouterLink
        v-for="r in rows"
        :key="r.item_id"
        :to="{ name: 'item', params: { id: r.item_id } }"
        class="card fav-card"
      >
        <div class="fav-card-body">
          <div>
            <p class="fav-title">{{ r.title }}</p>
            <p class="fav-sub">
              {{ r.year ?? '—' }} · {{ KIND_LABEL[r.kind] ?? r.kind }}
            </p>
          </div>
          <svg
            viewBox="0 0 24 24"
            class="fav-heart"
            aria-hidden="true"
          >
            <path
              d="M12 20s-7-4.4-7-9.3A3.9 3.9 0 0 1 12 8a3.9 3.9 0 0 1 7 2.7C19 15.6 12 20 12 20z"
            />
          </svg>
        </div>
      </RouterLink>
    </div>
  </div>
</template>

<style scoped>
.skeleton-poster {
  aspect-ratio: 2 / 2.72;
  border-radius: 13px;
  background: linear-gradient(90deg, #162033 25%, #1e2c47 50%, #162033 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}

@keyframes shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}

.fav-card {
  padding: 14px;
  display: block;
  transition: all 0.2s ease;
  cursor: pointer;
  text-decoration: none;
}
.fav-card:hover {
  border-color: rgba(154, 131, 255, 0.4);
  transform: translateY(-2px);
  box-shadow: 0 12px 28px rgba(0, 0, 0, 0.3);
}

.fav-card-body {
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  gap: 12px;
}

.fav-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
  margin: 0;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.fav-sub {
  font-size: 11px;
  color: var(--muted);
  margin: 4px 0 0;
}

.fav-heart {
  width: 18px;
  height: 18px;
  align-self: flex-end;
  fill: #ff8eae;
  color: #ff8eae;
}
</style>
