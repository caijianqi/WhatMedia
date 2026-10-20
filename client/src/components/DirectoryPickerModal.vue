<script setup lang="ts">
/**
 * 服务端文件系统物理路径选择弹窗。
 *
 * 允许用户通过图形界面在 Web 端直接浏览服务端主机的磁盘驱动器和各级目录，
 * 无需手动敲击终端或手动输入易错的物理路径。
 */
import { computed, ref, watch } from 'vue'
import * as api from '../api/client'

const props = withDefaults(
  defineProps<{
    modelValue?: boolean
    open?: boolean
    initialPath?: string
  }>(),
  {
    modelValue: false,
    open: false,
    initialPath: '',
  },
)

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
  (e: 'select', path: string): void
  (e: 'close'): void
}>()

const isVisible = computed(() => props.open || props.modelValue)

const loading = ref(false)
const error = ref<string | null>(null)
const currentPath = ref('')
const parentPath = ref<string | null>(null)
const directories = ref<api.DirEntryView[]>([])
const selectedPath = ref('')

async function fetchDirectory(targetPath?: string): Promise<void> {
  loading.value = true
  error.value = null
  try {
    const res = await api.browseFsDir(targetPath)
    currentPath.value = res.current
    parentPath.value = res.parent ?? null
    directories.value = res.directories
    selectedPath.value = res.current || (res.directories[0]?.path ?? '')
  } catch (err) {
    error.value = err instanceof api.ApiError ? err.detail.message : String(err)
  } finally {
    loading.value = false
  }
}

watch(
  () => isVisible.value,
  (visible) => {
    if (visible) {
      void fetchDirectory(props.initialPath || undefined)
    }
  },
  { immediate: true },
)

function handleSelectEntry(entry: api.DirEntryView): void {
  selectedPath.value = entry.path
}

function handleOpenEntry(entry: api.DirEntryView): void {
  void fetchDirectory(entry.path)
}

function handleGoParent(): void {
  if (parentPath.value !== null) {
    void fetchDirectory(parentPath.value || undefined)
  }
}

function handleRefresh(): void {
  void fetchDirectory(currentPath.value || undefined)
}

function handleClose(): void {
  emit('update:modelValue', false)
  emit('close')
}

function handleConfirm(): void {
  const chosen = selectedPath.value || currentPath.value
  if (!chosen) return
  emit('select', chosen)
  handleClose()
}
</script>

<template>
  <div v-if="isVisible" class="dir-picker-overlay" @click.self="handleClose">
    <div class="dir-picker-modal" role="dialog" aria-modal="true" aria-labelledby="picker-title">
      <!-- 弹窗标题栏 -->
      <div class="picker-header">
        <div>
          <h2 id="picker-title" class="picker-title">选择服务端媒体文件夹</h2>
          <p class="picker-sub">直接浏览服务端主机上的磁盘与目录，一键选定入库路径</p>
        </div>
        <button type="button" class="close-btn" aria-label="关闭弹窗" @click="handleClose">
          ✕
        </button>
      </div>

      <!-- 当前路径导航与操作栏 -->
      <div class="picker-nav-bar">
        <button
          type="button"
          class="nav-btn"
          :disabled="loading || parentPath === null"
          title="返回上一级目录"
          @click="handleGoParent"
        >
          <span class="icon">⬆</span>
          <span>上一级</span>
        </button>

        <div class="current-path-box">
          <span class="path-label">位置:</span>
          <span class="path-text font-mono" :title="currentPath || '根目录'">
            {{ currentPath ? currentPath : '（系统根目录 / 磁盘分区）' }}
          </span>
        </div>

        <button
          type="button"
          class="nav-btn icon-only"
          :disabled="loading"
          title="刷新目录列表"
          @click="handleRefresh"
        >
          🔄
        </button>
      </div>

      <!-- 目录内容列表容器 -->
      <div class="picker-body">
        <div v-if="loading" class="picker-loading">
          <span class="spinner" />
          <span>正在读取服务端目录...</span>
        </div>

        <div v-else-if="error" class="picker-error">
          <div class="error-msg">⚠️ 读取目录失败：{{ error }}</div>
          <button type="button" class="retry-btn" @click="handleRefresh">重试</button>
        </div>

        <div v-else-if="directories.length === 0" class="picker-empty">
          <span class="empty-icon">📁</span>
          <span>当前文件夹下没有子文件夹</span>
        </div>

        <div v-else class="dir-list" role="listbox">
          <div
            v-for="entry in directories"
            :key="entry.path"
            class="dir-item"
            :class="{ active: selectedPath === entry.path }"
            role="option"
            :aria-selected="selectedPath === entry.path"
            @click="handleSelectEntry(entry)"
            @dblclick="handleOpenEntry(entry)"
          >
            <span class="dir-icon">{{ entry.path.endsWith(':\\') ? '💾' : '📁' }}</span>
            <span class="dir-name">{{ entry.name }}</span>
            <button
              type="button"
              class="open-sub-btn"
              title="进入此目录"
              @click.stop="handleOpenEntry(entry)"
            >
              打开 ➔
            </button>
          </div>
        </div>
      </div>

      <!-- 底部选中结果与确认操作 -->
      <div class="picker-footer">
        <div class="selected-path-info">
          <span class="muted-label">已选物理路径：</span>
          <span class="chosen-path font-mono">{{ selectedPath || currentPath || '（未选择）' }}</span>
        </div>
        <div class="footer-actions">
          <button type="button" class="btn-cancel" @click="handleClose">
            取消
          </button>
          <button
            type="button"
            class="btn-confirm"
            :disabled="!(selectedPath || currentPath)"
            @click="handleConfirm"
          >
            确定使用此路径
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.dir-picker-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(5, 8, 16, 0.78);
  backdrop-filter: blur(10px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  animation: fadeIn 0.15s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

.dir-picker-modal {
  width: 100%;
  max-width: 680px;
  max-height: 85vh;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--line-strong);
  border-radius: var(--radius);
  box-shadow: 0 25px 70px rgba(0, 0, 0, 0.55);
  overflow: hidden;
}

.picker-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  padding: 20px 24px 16px;
  border-bottom: 1px solid var(--line);
}

.picker-title {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  color: var(--text);
}

.picker-sub {
  margin: 4px 0 0;
  font-size: 13px;
  color: var(--muted);
}

.close-btn {
  background: transparent;
  border: none;
  font-size: 18px;
  color: var(--muted);
  padding: 4px 8px;
  border-radius: 8px;
  transition: all 0.15s ease;
}

.close-btn:hover {
  color: var(--text);
  background: rgba(255, 255, 255, 0.08);
}

.picker-nav-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 24px;
  background: var(--panel2);
  border-bottom: 1px solid var(--line);
}

.nav-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border: 1px solid var(--line);
  background: var(--panel3);
  color: var(--text);
  border-radius: 8px;
  font-size: 13px;
  transition: all 0.15s ease;
}

.nav-btn:hover:not(:disabled) {
  border-color: var(--cyan);
  background: rgba(87, 200, 245, 0.1);
  color: var(--cyan);
}

.nav-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.nav-btn.icon-only {
  padding: 6px 10px;
}

.current-path-box {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  padding: 6px 12px;
  background: var(--bg);
  border: 1px solid var(--line);
  border-radius: 8px;
}

.path-label {
  font-size: 12px;
  color: var(--muted2);
  flex-shrink: 0;
}

.path-text {
  font-size: 12.5px;
  color: var(--cyan);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.picker-body {
  flex: 1;
  min-height: 280px;
  max-height: 380px;
  overflow-y: auto;
  padding: 12px 18px;
}

.picker-loading,
.picker-empty,
.picker-error {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 240px;
  gap: 12px;
  color: var(--muted);
  font-size: 14px;
}

.spinner {
  width: 24px;
  height: 24px;
  border: 2px solid rgba(87, 200, 245, 0.2);
  border-top-color: var(--cyan);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.picker-error {
  color: #f87171;
}

.retry-btn {
  padding: 6px 16px;
  background: rgba(248, 113, 113, 0.12);
  border: 1px solid rgba(248, 113, 113, 0.3);
  color: #f87171;
  border-radius: 8px;
}

.dir-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.dir-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  border-radius: 10px;
  border: 1px solid transparent;
  cursor: pointer;
  transition: all 0.12s ease;
}

.dir-item:hover {
  background: var(--panel2);
  border-color: var(--line);
}

.dir-item.active {
  background: rgba(87, 200, 245, 0.12);
  border-color: var(--cyan);
}

.dir-icon {
  font-size: 18px;
  line-height: 1;
}

.dir-name {
  flex: 1;
  font-size: 14px;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.open-sub-btn {
  opacity: 0;
  padding: 4px 10px;
  font-size: 12px;
  background: var(--panel3);
  border: 1px solid var(--line);
  color: var(--muted);
  border-radius: 6px;
  transition: all 0.15s ease;
}

.dir-item:hover .open-sub-btn {
  opacity: 1;
}

.open-sub-btn:hover {
  color: var(--cyan);
  border-color: var(--cyan);
}

.picker-footer {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 24px;
  background: var(--panel2);
  border-top: 1px solid var(--line);
}

.selected-path-info {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex: 1;
}

.muted-label {
  font-size: 12px;
  color: var(--muted2);
  flex-shrink: 0;
}

.chosen-path {
  font-size: 13px;
  color: var(--good);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.footer-actions {
  display: flex;
  gap: 10px;
}

.btn-cancel {
  padding: 8px 18px;
  border: 1px solid var(--line);
  background: transparent;
  color: var(--text);
  border-radius: 8px;
  font-size: 13px;
  transition: background 0.15s ease;
}

.btn-cancel:hover {
  background: rgba(255, 255, 255, 0.05);
}

.btn-confirm {
  padding: 8px 20px;
  border: none;
  background: linear-gradient(135deg, var(--purple), #5334db);
  color: #fff;
  border-radius: 8px;
  font-weight: 500;
  font-size: 13px;
  box-shadow: 0 4px 14px rgba(121, 91, 255, 0.35);
  transition: all 0.15s ease;
}

.btn-confirm:hover:not(:disabled) {
  background: linear-gradient(135deg, var(--purple2), var(--purple));
  box-shadow: 0 6px 18px rgba(121, 91, 255, 0.45);
}

.btn-confirm:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  box-shadow: none;
}
</style>
