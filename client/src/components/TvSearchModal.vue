<script setup lang="ts">
/**
 * 大屏 TV 专属遥控搜索面板。
 *
 * 针对遥控器 10-Foot 体验痛点：
 * 1. 26 键字母 + 0-9 数字大按键虚拟键盘，支持遥控器 D-Pad 快速游走。
 * 2. 支持拼音首字母检索与模糊搜索（如输入 "KB" 实时匹配《狂飙》）。
 * 3. 一键双轨多源聚合检索（本地媒体库 + 全网 VOD 聚合），无缝跳转播放或打开详情弹层。
 * 4. 1:1 遵循极光暗黑流体美学（Vanilla CSS），无缝融入大屏暗夜观影氛围。
 */
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import * as api from '../api/client'
import type { AggregatedVodItem, ItemSummary } from '../api/client'
import VodDetailModal from './VodDetailModal.vue'

const emit = defineEmits<{
  (e: 'close'): void
}>()

const router = useRouter()

const keyword = ref('')
const loading = ref(false)
const localResults = ref<ItemSummary[]>([])
const remoteResults = ref<AggregatedVodItem[]>([])
const selectedVod = ref<AggregatedVodItem | null>(null)

// 虚拟键盘键位：A-Z, 0-9
const KEYBOARD_KEYS = [
  'A', 'B', 'C', 'D', 'E', 'F',
  'G', 'H', 'I', 'J', 'K', 'L',
  'M', 'N', 'O', 'P', 'Q', 'R',
  'S', 'T', 'U', 'V', 'W', 'X',
  'Y', 'Z', '1', '2', '3', '4',
  '5', '6', '7', '8', '9', '0',
]

function appendKey(k: string) {
  keyword.value += k
}

function deleteKey() {
  keyword.value = keyword.value.slice(0, -1)
}

function clearKey() {
  keyword.value = ''
}

let searchTimer: ReturnType<typeof setTimeout> | null = null

watch(keyword, (newVal) => {
  if (searchTimer) clearTimeout(searchTimer)
  const trimmed = newVal.trim()
  if (!trimmed) {
    localResults.value = []
    remoteResults.value = []
    loading.value = false
    return
  }

  loading.value = true
  searchTimer = setTimeout(async () => {
    try {
      const res = await api.searchAggregate(trimmed)
      localResults.value = res.local?.items ?? []
      remoteResults.value = res.remote ?? []
    } catch {
      localResults.value = []
      remoteResults.value = []
    } finally {
      loading.value = false
    }
  }, 280)
})

function onSelectLocal(item: ItemSummary) {
  emit('close')
  void router.push({ name: 'detail', params: { id: item.id } })
}

function onSelectRemote(item: AggregatedVodItem) {
  selectedVod.value = item
}

function onCloseVodModal() {
  selectedVod.value = null
}

const hasResults = computed(() => localResults.value.length > 0 || remoteResults.value.length > 0)
</script>

<template>
  <div class="tv-search-backdrop" role="dialog" aria-modal="true">
    <div class="tv-search-dialog">
      <!-- 顶部搜索状态栏 -->
      <header class="tv-search-header">
        <div class="search-input-pill">
          <svg class="search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <span class="search-text" v-if="keyword">{{ keyword }}</span>
          <span class="search-placeholder" v-else>输入片名拼音首字母检索 (如 LLDQ / KB)</span>
          <span class="search-caret"></span>
        </div>

        <button class="close-btn" data-modal-close @click="$emit('close')" title="关闭搜索">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M18 6 6 18M6 6l12 12" />
          </svg>
        </button>
      </header>

      <!-- 双栏主体：左侧遥控键盘，右侧实时结果 -->
      <div class="tv-search-body">
        <!-- 左侧虚拟键盘区 -->
        <aside class="tv-keyboard-panel">
          <div class="tv-keyboard-grid">
            <button
              v-for="k in KEYBOARD_KEYS"
              :key="k"
              class="tv-key-btn"
              data-tv-focus
              @click="appendKey(k)"
            >
              {{ k }}
            </button>
          </div>

          <div class="tv-keyboard-actions">
            <button class="tv-key-action-btn" data-tv-focus @click="clearKey">
              清空
            </button>
            <button class="tv-key-action-btn del-btn" data-tv-focus @click="deleteKey">
              退格
            </button>
          </div>
        </aside>

        <!-- 右侧搜索结果区 -->
        <main class="tv-results-panel">
          <div v-if="loading" class="tv-search-loading">
            <div class="spinner"></div>
            <span>正在检索本地媒体与全网资源...</span>
          </div>

          <div v-else-if="!keyword" class="tv-search-hint">
            <svg class="hint-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6">
              <rect x="2" y="7" width="20" height="13" rx="2" />
              <path d="m17 2-5 5-5-5" />
            </svg>
            <p>使用遥控器输入片名简拼首字母或英文</p>
            <small>例如：《流浪地球》输入 LLDQ · 《狂飙》输入 KB</small>
          </div>

          <div v-else-if="!hasResults" class="tv-search-empty">
            <p>未找到与 "{{ keyword }}" 相关的内容</p>
          </div>

          <div v-else class="tv-results-scroll">
            <!-- 本地媒体库命中结果 -->
            <section v-if="localResults.length > 0" class="results-group">
              <h3 class="group-title">
                <span class="badge">本地媒体</span>
                共 {{ localResults.length }} 部
              </h3>
              <div class="results-grid">
                <button
                  v-for="item in localResults"
                  :key="item.id"
                  class="tv-result-card"
                  data-tv-focus
                  @click="onSelectLocal(item)"
                >
                  <div class="card-thumb">
                    <img v-if="item.poster_url" :src="item.poster_url" :alt="item.title" loading="lazy" />
                    <div v-else class="thumb-placeholder">{{ item.title.slice(0, 1) }}</div>
                  </div>
                  <div class="card-info">
                    <strong class="title">{{ item.title }}</strong>
                    <span class="meta">{{ item.year ?? '影视' }} · {{ item.media_type === 'series' ? '剧集' : '电影' }}</span>
                  </div>
                </button>
              </div>
            </section>

            <!-- 全网聚合命中结果 -->
            <section v-if="remoteResults.length > 0" class="results-group">
              <h3 class="group-title">
                <span class="badge remote-badge">全网聚合</span>
                共 {{ remoteResults.length }} 部
              </h3>
              <div class="results-grid">
                <button
                  v-for="vod in remoteResults"
                  :key="vod.id"
                  class="tv-result-card"
                  data-tv-focus
                  @click="onSelectRemote(vod)"
                >
                  <div class="card-thumb">
                    <img v-if="vod.picture" :src="vod.picture" :alt="vod.name" loading="lazy" />
                    <div v-else class="thumb-placeholder">{{ vod.name.slice(0, 1) }}</div>
                  </div>
                  <div class="card-info">
                    <strong class="title">{{ vod.name }}</strong>
                    <span class="meta">{{ vod.source_name }} · {{ vod.type_name ?? '影视' }}</span>
                  </div>
                </button>
              </div>
            </section>
          </div>
        </main>
      </div>
    </div>

    <!-- 聚合影视详情抽屉弹层 -->
    <VodDetailModal
      v-if="selectedVod"
      :item="selectedVod"
      @close="onCloseVodModal"
    />
  </div>
</template>

<style scoped>
.tv-search-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(4, 8, 16, 0.88);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 30px 48px;
  animation: fadeIn 0.2s ease-out;
}

.tv-search-dialog {
  width: 100%;
  max-width: 1360px;
  height: 88vh;
  background: #0b1322;
  border: 1px solid rgba(87, 200, 245, 0.25);
  border-radius: 20px;
  box-shadow: 0 25px 60px rgba(0, 0, 0, 0.7), 0 0 40px rgba(87, 200, 245, 0.15);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.tv-search-header {
  height: 84px;
  padding: 0 32px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(14, 23, 40, 0.95);
  flex-shrink: 0;
}

.search-input-pill {
  display: flex;
  align-items: center;
  gap: 14px;
  background: #141f35;
  border: 1.5px solid rgba(87, 200, 245, 0.35);
  border-radius: 40px;
  padding: 10px 24px;
  width: 580px;
}

.search-icon {
  width: 22px;
  height: 22px;
  color: #57c8f5;
  flex-shrink: 0;
}

.search-text {
  font-size: 20px;
  font-weight: 700;
  letter-spacing: 2px;
  color: #fff;
}

.search-placeholder {
  font-size: 15px;
  color: #6a7f9e;
}

.search-caret {
  display: inline-block;
  width: 2.5px;
  height: 22px;
  background: #57c8f5;
  animation: blink 1s infinite;
}

.close-btn {
  background: #16233b;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 50%;
  width: 44px;
  height: 44px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #a0b2ce;
  cursor: pointer;
  transition: all 0.2s;
}

.close-btn svg {
  width: 20px;
  height: 20px;
}

.tv-search-body {
  display: flex;
  flex: 1;
  overflow: hidden;
}

/* 键盘面板 */
.tv-keyboard-panel {
  width: 380px;
  padding: 24px 28px;
  background: rgba(11, 19, 34, 0.95);
  border-right: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  flex-direction: column;
  gap: 16px;
  flex-shrink: 0;
}

.tv-keyboard-grid {
  display: grid;
  grid-template-columns: repeat(6, 1fr);
  gap: 10px;
}

.tv-key-btn {
  height: 52px;
  background: #152239;
  border: 1px solid rgba(255, 255, 255, 0.09);
  border-radius: 10px;
  color: #d1deef;
  font-size: 18px;
  font-weight: 700;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s;
}

.tv-keyboard-actions {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}

.tv-key-action-btn {
  height: 50px;
  background: #1a2a46;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 10px;
  color: #e2eaf5;
  font-size: 16px;
  font-weight: 700;
  cursor: pointer;
  transition: all 0.15s;
}

.del-btn {
  color: #ff8e8e;
}

/* 焦点高亮 (大屏 3 米强对比度) */
.tv-key-btn.tv-focused,
.tv-key-action-btn.tv-focused,
.close-btn.tv-focused,
.tv-result-card.tv-focused {
  outline: none !important;
  border-color: #57c8f5 !important;
  box-shadow: 0 0 0 3px #ffffff, 0 0 24px rgba(87, 200, 245, 0.8) !important;
  transform: scale(1.08);
  z-index: 10;
  background: rgba(87, 200, 245, 0.35) !important;
  color: #ffffff !important;
}

/* 结果面板 */
.tv-results-panel {
  flex: 1;
  padding: 24px 32px;
  overflow-y: auto;
  position: relative;
}

.tv-search-loading,
.tv-search-hint,
.tv-search-empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: #7991b5;
  gap: 16px;
  text-align: center;
}

.hint-icon {
  width: 60px;
  height: 60px;
  color: #57c8f5;
  opacity: 0.6;
}

.spinner {
  width: 38px;
  height: 38px;
  border: 3px solid rgba(87, 200, 245, 0.2);
  border-top-color: #57c8f5;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

.tv-results-scroll {
  display: flex;
  flex-direction: column;
  gap: 28px;
}

.results-group .group-title {
  margin: 0 0 14px 0;
  font-size: 16px;
  color: #a4b7d4;
  display: flex;
  align-items: center;
  gap: 10px;
}

.badge {
  background: rgba(87, 200, 245, 0.2);
  color: #57c8f5;
  padding: 3px 10px;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 700;
}

.remote-badge {
  background: rgba(168, 85, 247, 0.2);
  color: #c084fc;
}

.results-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 16px;
}

.tv-result-card {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 10px;
  background: #141f33;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 12px;
  cursor: pointer;
  text-align: left;
  transition: all 0.18s;
}

.card-thumb {
  width: 52px;
  height: 72px;
  border-radius: 6px;
  overflow: hidden;
  background: #1e2c47;
  flex-shrink: 0;
}

.card-thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.thumb-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 20px;
  font-weight: 700;
  color: #57c8f5;
}

.card-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.card-info .title {
  font-size: 15px;
  color: #fff;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.card-info .meta {
  font-size: 12px;
  color: #7d93b3;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0; }
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
