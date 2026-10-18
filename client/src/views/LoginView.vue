<script setup lang="ts">
/**
 * 登录 / 首次安装页（LoginView）。
 * 1:1 遵循 ui.html 原生视觉规范：
 * - 极光光斑暗黑背景、毛玻璃卡片（.login-card）与品牌投影标志（.brand-mark）
 * - 原生设计系统输入框（.form-input）与紫光主按钮（.btn-primary）
 * - 兼顾初次建库设置（isSetup）与日常多端登录
 * - 完整支持儿童账号审批拦截（403 APPROVAL_REQUIRED）高保真反馈
 */
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useSessionStore } from '../stores/session'

const session = useSessionStore()
const router = useRouter()

const username = ref('')
const password = ref('')
const busy = ref(false)

const isSetup = computed(() => session.needsSetup)
const canSubmit = computed(
  () => !busy.value && username.value.trim().length > 0 && password.value.length >= 6,
)

async function submit(): Promise<void> {
  busy.value = true
  try {
    const ok = isSetup.value
      ? await session.setup(username.value.trim(), password.value)
      : await session.login(username.value.trim(), password.value)
    if (ok) await router.replace({ name: 'home' })
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="login-wrapper">
    <!-- 背景流体微光氛围光斑 -->
    <div class="glow-orb orb-purple" aria-hidden="true" />
    <div class="glow-orb orb-cyan" aria-hidden="true" />

    <!-- 居中毛玻璃登录卡片 -->
    <div class="login-card">
      <div class="card-brand">
        <span class="brand-mark">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M7 4.5c0-1 1.1-1.6 2-.9l10 7.1a1.6 1.6 0 0 1 0 2.6L9 20.4c-.9.7-2 .1-2-.9z" />
          </svg>
        </span>
        <div class="brand-text">
          <h1 class="brand-title">WhatMedia</h1>
          <p class="brand-subtitle">
            {{ isSetup ? '首次使用 · 创建管理员账户' : '家庭影院 · 登录后继续探索媒体库' }}
          </p>
        </div>
      </div>

      <form class="login-form" @submit.prevent="submit">
        <label class="form-group">
          <span class="form-label">用户名</span>
          <input
            v-model="username"
            class="form-input"
            type="text"
            autocomplete="username"
            placeholder="输入管理员或普通用户名"
            required
            autofocus
          />
        </label>

        <label class="form-group">
          <span class="form-label">口令</span>
          <input
            v-model="password"
            class="form-input"
            type="password"
            autocomplete="current-password"
            placeholder="••••••"
            minlength="6"
            required
          />
          <span v-if="isSetup" class="form-hint">
            至少 6 个字符。这个账户将成为服务器的管理员 —— 之后其他人必须由它邀请。
          </span>
        </label>

        <button
          class="btn btn-primary submit-btn"
          type="submit"
          :disabled="!canSubmit"
        >
          <span v-if="busy" class="btn-spinner" />
          <span>{{ busy ? '正在验证…' : isSetup ? '创建并进入影院' : '登 录' }}</span>
        </button>
      </form>

      <!-- 错误或审批等待反馈卡条 -->
      <div v-if="session.error" class="alert-box">
        <svg viewBox="0 0 24 24" class="alert-icon">
          <path d="M12 8v5M12 16.5v.5M12 3l9 17H3z" />
        </svg>
        <div class="alert-content">
          <p class="alert-message">{{ session.error }}</p>
          <span v-if="session.approvalId" class="alert-detail">
            设备审批编号：{{ session.approvalId }}
          </span>
        </div>
      </div>

      <div class="card-footer">
        <p>
          媒体库默认不对公网开放。口令用 argon2id 存摘要，会话票据只落 SHA-256 —— 数据库被拷走也换不回登录态。
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.login-wrapper {
  position: relative;
  min-height: 100vh;
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  overflow: hidden;
  background: radial-gradient(ellipse at 70% -15%, rgba(58, 91, 172, 0.22), transparent 45%), var(--bg);
}

/* 氛围光斑 */
.glow-orb {
  position: absolute;
  border-radius: 50%;
  pointer-events: none;
  filter: blur(80px);
  opacity: 0.6;
}
.orb-purple {
  top: -80px;
  left: -80px;
  width: 440px;
  height: 440px;
  background: radial-gradient(circle, rgba(121, 91, 255, 0.28) 0%, transparent 70%);
}
.orb-cyan {
  bottom: -90px;
  right: -90px;
  width: 480px;
  height: 480px;
  background: radial-gradient(circle, rgba(87, 200, 245, 0.18) 0%, transparent 70%);
}

/* 居中毛玻璃卡片 */
.login-card {
  position: relative;
  z-index: 2;
  width: min(440px, 100%);
  padding: 38px 34px 30px;
  border-radius: 22px;
  background: rgba(14, 23, 41, 0.88);
  border: 1px solid rgba(174, 193, 226, 0.14);
  backdrop-filter: blur(28px);
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.45), 0 0 1px rgba(255, 255, 255, 0.12);
}

/* 品牌头部 */
.card-brand {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 28px;
}
.brand-mark {
  width: 44px;
  height: 44px;
  border-radius: 13px;
  background: linear-gradient(135deg, #a78bfa, #6252ff 55%, #39bdf8);
  display: grid;
  place-items: center;
  box-shadow: 0 6px 22px rgba(121, 91, 255, 0.35);
  flex-shrink: 0;
}
.brand-mark svg {
  width: 22px;
  height: 22px;
  fill: white;
}
.brand-text {
  min-width: 0;
}
.brand-title {
  font-size: 24px;
  font-weight: 800;
  letter-spacing: -0.6px;
  margin: 0;
  line-height: 1.2;
  color: var(--text);
}
.brand-subtitle {
  font-size: 11px;
  color: var(--muted);
  margin: 4px 0 0;
  line-height: 1.4;
}

/* 表单主体 */
.login-form {
  display: flex;
  flex-direction: column;
  gap: 18px;
}
.form-group {
  display: block;
}
.form-label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  color: #adbcd9;
  letter-spacing: 0.6px;
  text-transform: uppercase;
  margin-bottom: 7px;
}
.form-input {
  width: 100%;
  height: 44px;
  padding: 0 14px;
  border-radius: 11px;
  border: 1px solid var(--line);
  background: rgba(19, 29, 49, 0.82);
  color: var(--text);
  font-size: 13px;
  box-sizing: border-box;
  transition: all 0.18s ease;
}
.form-input:focus {
  outline: none;
  border-color: var(--purple2);
  background: rgba(22, 34, 58, 0.95);
  box-shadow: 0 0 0 3px rgba(121, 91, 255, 0.22);
}
.form-input::placeholder {
  color: #637392;
}
.form-hint {
  display: block;
  margin-top: 6px;
  font-size: 11px;
  line-height: 1.5;
  color: var(--muted);
}

/* 提交按钮 */
.submit-btn {
  width: 100%;
  height: 44px;
  margin-top: 8px;
  font-size: 13px;
  font-weight: 700;
  letter-spacing: 0.5px;
}
.submit-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
  transform: none !important;
  box-shadow: none !important;
}

.btn-spinner {
  width: 14px;
  height: 14px;
  border: 2px solid rgba(255, 255, 255, 0.3);
  border-top-color: white;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}

/* 报警/提示条 */
.alert-box {
  margin-top: 18px;
  padding: 12px 14px;
  border-radius: 11px;
  border: 1px solid rgba(245, 158, 11, 0.3);
  background: rgba(245, 158, 11, 0.12);
  display: flex;
  gap: 10px;
  align-items: flex-start;
}
.alert-icon {
  width: 16px;
  height: 16px;
  stroke: #f59e0b;
  fill: none;
  stroke-width: 2;
  flex-shrink: 0;
  margin-top: 2px;
}
.alert-content {
  flex: 1;
}
.alert-message {
  margin: 0;
  font-size: 12px;
  color: #fde68a;
  line-height: 1.5;
}
.alert-detail {
  display: block;
  margin-top: 3px;
  font-size: 11px;
  color: #fcd34d;
  opacity: 0.85;
}

/* 卡片底栏说明 */
.card-footer {
  margin-top: 24px;
  padding-top: 18px;
  border-top: 1px solid var(--line);
}
.card-footer p {
  margin: 0;
  font-size: 11px;
  line-height: 1.65;
  color: var(--muted2);
}
</style>
