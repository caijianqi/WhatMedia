<script setup lang="ts">
/**
 * 手动识别 / 影视消歧与重新匹配弹窗。
 * 允许用户在 TMDB 多个候选（例如动画版 vs 电视剧版）之间自主选择，
 * 支持点击图片进行高清大图预览消歧，也支持直接输入 TMDB ID 精确匹配入库。
 */
import { onMounted, onUnmounted, ref, watch } from 'vue'
import * as api from '../api/client'
import { toast } from '../stores/toast'

const props = defineProps<{
  open: boolean
  itemId: string
  initialTitle?: string
  initialYear?: number | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'identified'): void
}>()

const searchTitle = ref('')
const searchYear = ref<number | ''>('')
const manualId = ref('')
const loading = ref(false)
const applying = ref(false)
const candidates = ref<api.CandidateView[]>([])
const selectedProviderId = ref<string | null>(null)
const imgErrorMap = ref<Record<string, boolean>>({})

// 高清大图预览状态
const previewCandidate = ref<api.CandidateView | null>(null)
const previewImgError = ref(false)

function getCandidatePoster(c: api.CandidateView): string {
  const raw = c.poster || c.poster_url
  if (!raw) return ''
  return api.imageProxyUrl(raw)
}

/**
 * 升档为高清大图 URL（将 TMDB /w500/ 替换为 /original/ 高清原图档位）。
 */
function getHighResPoster(url?: string | null): string {
  if (!url) return ''
  let hiRes = url
  if (hiRes.includes('/w500/')) {
    hiRes = hiRes.replace('/w500/', '/original/')
  }
  return api.imageProxyUrl(hiRes)
}

function openPreview(c: api.CandidateView) {
  const raw = c.poster || c.poster_url
  if (!raw) return
  previewCandidate.value = c
  previewImgError.value = false
}

function closePreview() {
  previewCandidate.value = null
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    if (previewCandidate.value) {
      closePreview()
    } else if (props.open) {
      emit('close')
    }
  }
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
})

async function fetchCandidates() {
  if (!props.itemId) return
  loading.value = true
  candidates.value = []
  selectedProviderId.value = null
  imgErrorMap.value = {}
  try {
    const list = await api.getCandidates({
      item_id: props.itemId,
      title: searchTitle.value.trim() || undefined,
      year: typeof searchYear.value === 'number' ? searchYear.value : undefined,
    })
    candidates.value = list
    if (list.length > 0) {
      selectedProviderId.value = list[0].provider_id
    }
  } catch (err) {
    toast(`获取候选失败：${err instanceof Error ? err.message : String(err)}`, { type: 'error' })
  } finally {
    loading.value = false
  }
}

watch(
  () => props.open,
  (val) => {
    if (val) {
      searchTitle.value = props.initialTitle || ''
      searchYear.value = props.initialYear ?? ''
      manualId.value = ''
      void fetchCandidates()
    }
  },
  { immediate: true },
)

async function applySelected(targetId?: string) {
  const pId = targetId || selectedProviderId.value || manualId.value.trim()
  if (!pId) {
    toast('请选择一个匹配项或输入 TMDB ID', { type: 'warn' })
    return
  }
  applying.value = true
  try {
    await api.identify(props.itemId, {
      provider: 'tmdb',
      provider_id: pId,
    })
    toast('已成功应用元数据与海报', { type: 'info' })
    emit('identified')
    emit('close')
  } catch (err) {
    toast(`应用匹配失败：${err instanceof Error ? err.message : String(err)}`, { type: 'error' })
  } finally {
    applying.value = false
  }
}
</script>

<template>
  <div v-if="open" class="modal-backdrop" @click.self="emit('close')">
    <div class="modal-card">
      <div class="modal-header">
        <div class="modal-title-wrap">
          <span class="icon-chip">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" fill="none" />
            </svg>
          </span>
          <div>
            <h3 class="modal-title">重新识别与消歧匹配</h3>
            <p class="modal-subtitle">点击封面可放大查看海报细节，支持精确挑选版本或直接输入 ID</p>
          </div>
        </div>
        <button type="button" class="close-btn" @click="emit('close')">
          <svg viewBox="0 0 24 24"><path d="M18 6L6 18M6 6l12 12" stroke="currentColor" stroke-width="2" stroke-linecap="round" /></svg>
        </button>
      </div>

      <!-- 搜索调节条 -->
      <div class="search-bar">
        <div class="input-group">
          <label class="sr-only">搜索标题</label>
          <input
            v-model="searchTitle"
            type="text"
            class="input-title"
            placeholder="输入影片或剧集名称..."
            @keyup.enter="fetchCandidates"
          />
        </div>
        <div class="input-group year-group">
          <label class="sr-only">年份</label>
          <input
            v-model.number="searchYear"
            type="number"
            class="input-year"
            placeholder="年份"
            @keyup.enter="fetchCandidates"
          />
        </div>
        <button
          type="button"
          class="btn btn-secondary btn-search"
          :disabled="loading"
          @click="fetchCandidates"
        >
          {{ loading ? '搜索中...' : '重新搜索' }}
        </button>
      </div>

      <!-- 候选列表 -->
      <div class="candidates-container">
        <div v-if="loading" class="empty-hint">
          <div class="spinner" />
          <span>正在连接 TMDB 检索候选条目...</span>
        </div>
        <div v-else-if="candidates.length === 0" class="empty-hint">
          <span>未找到相关匹配，请尝试调整标题或在下方直接输入 TMDB ID</span>
        </div>
        <div v-else class="candidate-list">
          <div
            v-for="c in candidates"
            :key="c.provider_id"
            class="candidate-card"
            :class="{ active: selectedProviderId === c.provider_id }"
            @click="selectedProviderId = c.provider_id"
            @dblclick="applySelected(c.provider_id)"
          >
            <!-- 海报缩略图：点击放大 -->
            <div
              class="cand-poster"
              :class="{ 'has-zoom': !!(c.poster || c.poster_url) }"
              title="点击查看高清海报"
              @click.stop="openPreview(c)"
            >
              <img
                v-if="getCandidatePoster(c) && !imgErrorMap[c.provider_id]"
                :src="getCandidatePoster(c)"
                :alt="c.title"
                loading="lazy"
                referrerpolicy="no-referrer"
                @error="imgErrorMap[c.provider_id] = true"
              />
              <div v-else class="poster-none" title="暂无海报">
                <span class="poster-initials">{{ (c.title || 'WM').slice(0, 2) }}</span>
              </div>
              <div v-if="getCandidatePoster(c) && !imgErrorMap[c.provider_id]" class="zoom-badge">
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0zM10 7v6m3-3H7" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none" />
                </svg>
              </div>
            </div>

            <div class="cand-info">
              <div class="cand-head">
                <strong class="cand-title">{{ c.title }}</strong>
                <span v-if="c.year" class="cand-year">({{ c.year }})</span>
                <span class="cand-id-tag">TMDB: {{ c.provider_id }}</span>
                <span v-if="c.confidence === 'auto'" class="cand-conf-tag conf-auto">高置信</span>
                <span v-else class="cand-conf-tag conf-review">待复核</span>
              </div>
              <p v-if="c.original_title && c.original_title !== c.title" class="cand-orig">
                原名：{{ c.original_title }}
              </p>
              <p class="cand-overview">{{ c.overview || '暂无概述' }}</p>
            </div>
            <div class="cand-select-radio">
              <span class="radio-circle" :class="{ checked: selectedProviderId === c.provider_id }" />
            </div>
          </div>
        </div>
      </div>

      <!-- 底部操作与手动 ID 输入 -->
      <div class="modal-footer">
        <div class="manual-direct">
          <span style="font-size:12px;color:var(--muted)">直接填 TMDB ID:</span>
          <input
            v-model="manualId"
            type="text"
            class="input-manual"
            placeholder="例如 106449"
          />
        </div>
        <div class="footer-actions">
          <button type="button" class="btn btn-secondary" @click="emit('close')">取消</button>
          <button
            type="button"
            class="btn btn-primary"
            :disabled="applying || (!selectedProviderId && !manualId.trim())"
            @click="applySelected()"
          >
            {{ applying ? '应用中...' : '确认采用此匹配' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 高清海报大图预览 Lightbox -->
    <Teleport to="body">
      <div
        v-if="previewCandidate"
        class="lightbox-backdrop"
        @click.self="closePreview"
      >
        <div class="lightbox-dialog">
          <div class="lightbox-header">
            <div class="lightbox-info">
              <strong class="lightbox-title">{{ previewCandidate.title }}</strong>
              <span v-if="previewCandidate.year" class="lightbox-year">({{ previewCandidate.year }})</span>
              <span class="lightbox-badge">TMDB ID: {{ previewCandidate.provider_id }}</span>
              <span class="lightbox-hd-tag">高清原图</span>
            </div>
            <button type="button" class="lightbox-close" title="关闭 (Esc)" @click="closePreview">
              <svg viewBox="0 0 24 24"><path d="M18 6L6 18M6 6l12 12" stroke="currentColor" stroke-width="2" stroke-linecap="round" /></svg>
            </button>
          </div>

          <div class="lightbox-body">
            <img
              v-if="!previewImgError"
              :src="getHighResPoster(previewCandidate.poster || previewCandidate.poster_url)"
              :alt="previewCandidate.title"
              class="lightbox-img"
              referrerpolicy="no-referrer"
              @error="previewImgError = true"
            />
            <div v-else class="lightbox-error">
              <p>高清海报加载失败，原图可能已被移除</p>
            </div>
          </div>

          <div class="lightbox-footer">
            <span class="lightbox-tip">按 Esc 或点击背景任意处退出预览</span>
            <button
              type="button"
              class="btn btn-primary btn-sm"
              @click="applySelected(previewCandidate.provider_id); closePreview()"
            >
              采用此版本
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.modal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(4, 9, 20, 0.75);
  backdrop-filter: blur(10px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
}

.modal-card {
  width: 100%;
  max-width: 680px;
  max-height: 85vh;
  background: #0f172a;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 20px;
  display: flex;
  flex-direction: column;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.6);
  animation: modalIn 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  overflow: hidden;
}

@keyframes modalIn {
  from { opacity: 0; transform: scale(0.96) translateY(8px); }
  to { opacity: 1; transform: scale(1) translateY(0); }
}

.modal-header {
  padding: 18px 22px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}

.modal-title-wrap {
  display: flex;
  align-items: center;
  gap: 12px;
}

.icon-chip {
  width: 38px;
  height: 38px;
  border-radius: 10px;
  background: rgba(99, 102, 241, 0.15);
  color: #818cf8;
  display: flex;
  align-items: center;
  justify-content: center;
}

.icon-chip svg {
  width: 18px;
  height: 18px;
}

.modal-title {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
  color: #f1f5f9;
}

.modal-subtitle {
  margin: 2px 0 0;
  font-size: 12px;
  color: #94a3b8;
}

.close-btn {
  background: transparent;
  border: none;
  color: #94a3b8;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.15s;
}

.close-btn:hover {
  background: rgba(255, 255, 255, 0.08);
  color: #fff;
}

.close-btn svg {
  width: 18px;
  height: 18px;
}

.search-bar {
  padding: 14px 22px;
  display: flex;
  gap: 10px;
  background: rgba(255, 255, 255, 0.02);
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

.input-title {
  flex: 1;
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 10px;
  padding: 8px 12px;
  font-size: 13px;
  color: #fff;
  outline: none;
  width: 100%;
}

.input-title:focus,
.input-year:focus,
.input-manual:focus {
  border-color: #6366f1;
}

.input-year {
  width: 80px;
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 10px;
  padding: 8px 10px;
  font-size: 13px;
  color: #fff;
  outline: none;
}

.candidates-container {
  flex: 1;
  overflow-y: auto;
  padding: 14px 22px;
  min-height: 260px;
}

.empty-hint {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 220px;
  gap: 12px;
  color: #64748b;
  font-size: 13px;
}

.spinner {
  width: 24px;
  height: 24px;
  border: 2px solid rgba(99, 102, 241, 0.2);
  border-top-color: #6366f1;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.candidate-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.candidate-card {
  display: flex;
  gap: 14px;
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 12px;
  cursor: pointer;
  transition: all 0.15s;
  align-items: center;
}

.candidate-card:hover {
  background: rgba(255, 255, 255, 0.06);
  border-color: rgba(99, 102, 241, 0.4);
}

.candidate-card.active {
  background: rgba(99, 102, 241, 0.12);
  border-color: #6366f1;
}

/* 升级后的海报容器尺寸：62px × 90px */
.cand-poster {
  position: relative;
  width: 62px;
  height: 90px;
  border-radius: 8px;
  overflow: hidden;
  background: #1e293b;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
  transition: transform 0.15s ease, box-shadow 0.15s ease;
}

.cand-poster.has-zoom {
  cursor: zoom-in;
}

.cand-poster.has-zoom:hover {
  transform: scale(1.04);
  box-shadow: 0 6px 16px rgba(99, 102, 241, 0.3);
}

.cand-poster img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: filter 0.15s;
}

.cand-poster:hover img {
  filter: brightness(0.85);
}

.zoom-badge {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(15, 23, 42, 0.45);
  opacity: 0;
  transition: opacity 0.15s ease;
  color: #fff;
}

.cand-poster:hover .zoom-badge {
  opacity: 1;
}

.zoom-badge svg {
  width: 20px;
  height: 20px;
  filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.6));
}

.poster-none {
  width: 100%;
  height: 100%;
  background: linear-gradient(135deg, rgba(99, 102, 241, 0.2), rgba(15, 23, 42, 0.8));
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 8px;
  border: 1px dashed rgba(255, 255, 255, 0.12);
}

.poster-initials {
  font-size: 14px;
  font-weight: 700;
  color: rgba(255, 255, 255, 0.55);
  letter-spacing: 0.5px;
}

.cand-info {
  flex: 1;
  min-width: 0;
}

.cand-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.cand-title {
  font-size: 14px;
  color: #f8fafc;
}

.cand-year {
  font-size: 12px;
  color: #94a3b8;
}

.cand-id-tag {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.08);
  color: #cbd5e1;
}

.cand-conf-tag {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
}

.conf-auto {
  background: rgba(34, 197, 94, 0.15);
  color: #4ade80;
}

.conf-review {
  background: rgba(234, 179, 8, 0.15);
  color: #facc15;
}

.cand-orig {
  margin: 2px 0 0;
  font-size: 11px;
  color: #64748b;
}

.cand-overview {
  margin: 4px 0 0;
  font-size: 12px;
  color: #94a3b8;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  line-height: 1.4;
}

.cand-select-radio {
  padding-left: 8px;
}

.radio-circle {
  display: inline-block;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: 2px solid rgba(255, 255, 255, 0.3);
  position: relative;
  transition: all 0.15s;
}

.radio-circle.checked {
  border-color: #6366f1;
  background: #6366f1;
}

.modal-footer {
  padding: 14px 22px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: rgba(15, 23, 42, 0.95);
  gap: 12px;
}

.manual-direct {
  display: flex;
  align-items: center;
  gap: 8px;
}

.input-manual {
  width: 100px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 6px;
  padding: 4px 8px;
  font-size: 12px;
  color: #fff;
  outline: none;
}

.footer-actions {
  display: flex;
  gap: 10px;
}

.btn-sm {
  padding: 6px 14px;
  font-size: 12px;
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  border: 0;
}

/* ─────────────────────────────────────────────────────────────
   高清大图预览 Lightbox 浮层样式
   ───────────────────────────────────────────────────────────── */
.lightbox-backdrop {
  position: fixed;
  inset: 0;
  z-index: 2000;
  background: rgba(2, 6, 23, 0.88);
  backdrop-filter: blur(20px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  animation: fadeIn 0.18s ease-out;
}

.lightbox-dialog {
  max-width: 90vw;
  max-height: 92vh;
  background: #0b1329;
  border: 1px solid rgba(99, 102, 241, 0.3);
  border-radius: 18px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 25px 60px rgba(0, 0, 0, 0.8), 0 0 40px rgba(99, 102, 241, 0.2);
  animation: scaleUp 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes scaleUp {
  from { opacity: 0; transform: scale(0.92); }
  to { opacity: 1; transform: scale(1); }
}

.lightbox-header {
  padding: 12px 18px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.02);
}

.lightbox-info {
  display: flex;
  align-items: center;
  gap: 10px;
}

.lightbox-title {
  font-size: 15px;
  color: #fff;
}

.lightbox-year {
  font-size: 13px;
  color: #94a3b8;
}

.lightbox-badge {
  font-size: 11px;
  background: rgba(255, 255, 255, 0.08);
  padding: 2px 8px;
  border-radius: 4px;
  color: #cbd5e1;
}

.lightbox-hd-tag {
  font-size: 11px;
  background: rgba(99, 102, 241, 0.2);
  color: #818cf8;
  padding: 2px 8px;
  border-radius: 4px;
  font-weight: 600;
}

.lightbox-close {
  background: transparent;
  border: none;
  color: #94a3b8;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s;
}

.lightbox-close:hover {
  background: rgba(255, 255, 255, 0.1);
  color: #fff;
}

.lightbox-close svg {
  width: 18px;
  height: 18px;
}

.lightbox-body {
  padding: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #020617;
  overflow: hidden;
}

.lightbox-img {
  max-width: 80vw;
  max-height: 70vh;
  object-fit: contain;
  border-radius: 10px;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
}

.lightbox-error {
  min-width: 280px;
  min-height: 380px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #64748b;
  font-size: 13px;
}

.lightbox-footer {
  padding: 10px 18px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: rgba(255, 255, 255, 0.02);
  gap: 16px;
}

.lightbox-tip {
  font-size: 12px;
  color: #64748b;
}
</style>
