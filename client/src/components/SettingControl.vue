<script setup lang="ts">
/**
 * 一个可写设置的控件。
 *
 * 控件形态由服务端给的 `kind` 决定，不是前端猜的 ——
 * 类型与取值范围都在服务端那份白名单里，前端照着渲染就行。
 * 前端自己判断"这个值像不像布尔"会导致两边规则漂移。
 *
 * 保存失败的提示就地显示在这一行下面，而不是弹全局 toast：
 * 出错时用户的眼睛正在看这一行，把原因放在这里才找得到。
 */
import { computed, ref, watch } from 'vue'
import type { SettingEntry } from '../api/client'

const props = defineProps<{ entry: SettingEntry }>()
const emit = defineEmits<{ (e: 'save', value: string): void; (e: 'reset'): void }>()

const draft = ref(props.entry.value)
const saving = ref(false)
const err = ref<string | null>(null)
const showSecret = ref(false)

const isSecretKey = computed(() => {
  const k = props.entry.key.toLowerCase()
  return k.includes('key') || k.includes('bearer') || k.includes('token') || k.includes('secret')
})

// 服务端返回了新值时同步过来（保存成功 / 重新拉取）。
watch(
  () => props.entry.value,
  (v) => {
    draft.value = v
  },
)

function asBool(v: string): boolean {
  return v === 'true'
}

function commit(v: string): void {
  draft.value = v
  saving.value = true
  err.value = null
  emit('save', v)
  // 保存结果由父组件回写 entry.value；这里只负责把按钮进入"保存中"。
  window.setTimeout(() => {
    saving.value = false
  }, 400)
}

/** 父组件保存失败时调它。 */
function fail(msg: string): void {
  err.value = msg
  saving.value = false
}

defineExpose({ fail })
</script>

<template>
  <div class="setting-item-row">
    <div class="setting-item-main">
      <div class="setting-item-info">
        <p class="setting-item-label">{{ entry.label }}</p>
        <p class="setting-item-hint">{{ entry.hint }}</p>
        <p class="setting-item-key">{{ entry.key }}</p>
      </div>

      <div class="setting-item-ctl">
        <!-- 布尔：开关 (ui.html 原生 .switch 规范) -->
        <button
          v-if="entry.kind === 'bool'"
          type="button"
          class="switch"
          :class="{ on: asBool(draft) }"
          role="switch"
          :aria-checked="asBool(draft)"
          :aria-label="entry.label"
          @click="commit(asBool(draft) ? 'false' : 'true')"
        />

        <!-- 枚举：下拉 -->
        <select
          v-else-if="entry.kind === 'enum'"
          class="select setting-select"
          :value="draft"
          @change="commit(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="c in entry.choices" :key="c" :value="c">{{ c }}</option>
        </select>

        <!-- 数字 -->
        <input
          v-else-if="entry.kind === 'int' || entry.kind === 'float'"
          class="input setting-num-input"
          type="number"
          :min="entry.min ?? undefined"
          :max="entry.max ?? undefined"
          :step="entry.kind === 'float' ? '0.1' : '1'"
          :value="draft"
          @change="commit(($event.target as HTMLInputElement).value)"
        />

        <!-- 敏感密钥文本 -->
        <div v-else-if="isSecretKey" class="secret-input-wrap">
          <input
            class="input setting-text-input secret-input"
            :type="showSecret ? 'text' : 'password'"
            :value="draft"
            :placeholder="entry.default || '请输入密钥…'"
            autocomplete="off"
            spellcheck="false"
            @change="commit(($event.target as HTMLInputElement).value)"
          />
          <button
            type="button"
            class="secret-toggle-btn"
            :title="showSecret ? '隐藏' : '显示明文'"
            @click="showSecret = !showSecret"
          >
            {{ showSecret ? '🙈' : '👁️' }}
          </button>
        </div>

        <!-- 常规文本 -->
        <input
          v-else
          class="input setting-text-input"
          type="text"
          :value="draft"
          :placeholder="entry.default"
          @change="commit(($event.target as HTMLInputElement).value)"
        />
      </div>
    </div>

    <div class="setting-item-footer">
      <span v-if="saving" class="status-saving">保存中…</span>
      <span v-else-if="err" class="status-err">{{ err }}</span>
      <span v-else-if="entry.overridden" class="status-changed">已改动</span>
      <button
        v-if="entry.overridden"
        type="button"
        class="reset-btn"
        @click="emit('reset')"
      >
        恢复默认（{{ entry.default }}）
      </button>
      <!-- 如实说明改完需重启 -->
      <span v-if="entry.restart_required" class="status-restart">改完需重启服务端</span>
    </div>
  </div>
</template>

<style scoped>
.setting-item-row {
  padding: 14px 4px;
  border-bottom: 1px solid var(--line);
}
.setting-item-row:last-child {
  border-bottom: 0;
}
.setting-item-main {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}
.setting-item-info {
  min-width: 0;
}
.setting-item-label {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
  margin: 0;
}
.setting-item-hint {
  font-size: 11px;
  line-height: 1.6;
  color: var(--muted);
  margin: 3px 0 0;
}
.setting-item-key {
  font-family: monospace;
  font-size: 10px;
  color: var(--muted2);
  margin: 4px 0 0;
}
.setting-item-ctl {
  flex-shrink: 0;
}
.setting-select {
  padding: 6px 28px 6px 10px;
  font-size: 12px;
}
.setting-num-input {
  width: 100px;
  padding: 6px 10px;
  text-align: right;
  font-size: 12px;
}
.setting-text-input {
  width: 200px;
  padding: 6px 10px;
  font-size: 12px;
}
.setting-item-footer {
  margin-top: 8px;
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 11px;
}
.status-saving {
  color: var(--muted);
}
.status-err {
  color: #f87171;
}
.status-changed {
  color: #bcaeff;
}
.reset-btn {
  border: 0;
  background: transparent;
  color: var(--muted);
  text-decoration: underline;
  text-decoration-style: dotted;
  padding: 0;
  cursor: pointer;
}
.reset-btn:hover {
  color: var(--text);
}
.status-restart {
  color: #fbbf24;
}
.secret-input-wrap {
  position: relative;
  display: flex;
  align-items: center;
}
.secret-input-wrap .secret-input {
  width: 220px;
  padding-right: 32px;
  font-family: monospace;
}
.secret-toggle-btn {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 2px 4px;
  font-size: 13px;
  opacity: 0.7;
  transition: opacity 0.15s ease;
  user-select: none;
}
.secret-toggle-btn:hover {
  opacity: 1;
}
</style>

