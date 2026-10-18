<script setup lang="ts">
/**
 * 设置页。
 *
 * 这里展示的是**服务端自检的结果**，不是配置项编辑器。
 * 把硬件探测结果暴露给用户是有意的：当"为什么这个片要转码"发生时，
 * 用户能自己看到"服务端只检测到软件编码"，比去翻日志强得多。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import * as api from '../api/client'
import SettingControl from '../components/SettingControl.vue'
import { probeMpv, tauriAvailable, type MpvStatus } from '../player'

const route = useRoute()

const health = ref<api.HealthReport | null>(null)
const hardware = ref<api.HardwareProfileReport | null>(null)
const error = ref<string | null>(null)

// ── 可写设置 ──────────────────────────────────────────────
//
// 这一块与页面其余部分不同：它是**能改的**。
// 能改的只有服务端白名单里的键（见 crates/config 的 SETTING_SPECS），
// 路径、端口、数据库地址、密钥一个都不在里面 ——
// 那个限制在服务端，前端也别去猜"能不能改"，照 `entries` 渲染就行。
const settings = ref<api.SettingsView | null>(null)
const settingsError = ref<string | null>(null)

const serverEntries = computed(
  () => settings.value?.entries.filter((e) => e.scope === 'server') ?? [],
)
const userEntries = computed(() => settings.value?.entries.filter((e) => e.scope === 'user') ?? [])

async function loadSettings(): Promise<void> {
  try {
    settings.value = await api.getSettings()
    settingsError.value = null
  } catch (e) {
    settingsError.value = e instanceof Error ? e.message : String(e)
  }
}

/** 服务端写成功后会把新值回传，直接用它替换本地那一条。 */
async function saveSetting(e: api.SettingEntry, value: string): Promise<void> {
  try {
    const saved = await api.putSetting(e.key, value)
    const list = settings.value?.entries
    if (list) {
      const i = list.findIndex((x) => x.key === e.key)
      if (i >= 0) list[i] = saved
    }
  } catch (err) {
    // 保存失败要把原因说出来 —— 静默回滚会让人以为改成功了。
    settingsError.value =
      err instanceof api.ApiError ? err.detail.message : String(err)
  }
}

async function resetSetting(e: api.SettingEntry): Promise<void> {
  try {
    await api.resetSetting(e.key)
    await loadSettings()
  } catch (err) {
    settingsError.value = err instanceof api.ApiError ? err.detail.message : String(err)
  }
}

// ── 存储占用 ──────────────────────────────────────────────
const storage = ref<api.StorageView | null>(null)
const storageError = ref<string | null>(null)

async function loadStorage(): Promise<void> {
  try {
    storage.value = await api.storageStats()
  } catch (e) {
    storageError.value = e instanceof Error ? e.message : String(e)
  }
}

/**
 * 字节数转人话。
 *
 * 用 1024 为进制但标 GB/TB —— 磁盘厂商用 1000、系统用 1024，
 * 这里跟系统一致，因为用户是在拿它和系统里看到的大小对。
 */
function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)))
  return `${(n / 1024 ** i).toFixed(i === 0 ? 0 : 1)} ${units[i]}`
}

/** 各库占比。用来画"哪个库最占地方" —— 比一个总数有用得多。 */
function shareOf(bytes: number): number {
  const total = storage.value?.libraries.reduce((a, l) => a + l.bytes, 0) ?? 0
  return total > 0 ? (bytes / total) * 100 : 0
}

/**
 * AI 段只回答"现在能不能用、能用到什么程度"。
 *
 * 刻意不放 provider / 模型名的编辑框：那是服务端配置的事，
 * 而且在网页上改错一个 base_url 的代价比"去服务端改一下"大得多。
 */
const aiTools = ref<api.ToolSpecView[]>([])
const aiError = ref<string | null>(null)

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
  // AI 单独取：它挂了不该让"服务端状态"那一栏也变成空白。
  try {
    const tools = await api.listAiTools()
    aiTools.value = Array.isArray(tools) ? tools : []
  } catch (e) {
    aiError.value = api.isAiDisabled(e)
      ? '未启用：把服务端配置 [ai] 段的 enabled 设为 true'
      : e instanceof Error
        ? e.message
        : String(e)
  }
  // 这两块单独取：设置端点或存储端点挂了，不该让"服务端状态"那栏也空白。
  void loadSettings()
  void loadStorage()
  if (inTauri) mpv.value = await probeMpv()
})

function fmtUptime(s: number): string {
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分`
}

/** 硬件档中文名。`kind` 是后端自述的枚举，直接显示不够直白。 */
const kindLabels: Record<string, string> = {
  intel_qsv: 'Intel QSV',
  nvidia_nvenc: 'NVIDIA NVENC',
  amd_amf: 'AMD AMF',
  vaapi: 'VAAPI (Linux)',
  videotoolbox: 'VideoToolbox',
  rockchip: 'Rockchip MPP',
  software: '软件编码',
}

const dangerousCount = () => (aiTools.value ?? []).filter((t) => t.risk !== 'read_only').length

/**
 * 编码名。
 *
 * `VideoCodec` 里有一个 `{ other: string }` 变体（服务端遇到不认识的编码时
 * 透传原值），所以它不是纯粹的 string —— 直接拿来当 key 或插值都不行。
 */
function codecLabel(c: unknown): string {
  if (typeof c === 'string') return c
  const o = c as { other?: string }
  return o.other ?? String(c)
}
const initialTab = typeof route?.query?.tab === 'string' ? route.query.tab : 'all'
const activeTab = ref<string>(initialTab)

if (route) {
  watch(
    () => route.query.tab,
    (tab) => {
      if (typeof tab === 'string' && tab) {
        activeTab.value = tab
      }
    },
  )
}

const tabs = [
  { id: 'all', label: '全部项目', icon: '✦' },
  { id: 'server', label: '服务端', icon: '▣' },
  { id: 'hardware', label: '转码能力', icon: '⚡' },
  { id: 'storage', label: '存储与媒体库', icon: '💾' },
  { id: 'entries', label: '可写设置', icon: '⚙' },
  { id: 'ai', label: 'AI 助手', icon: '🤖' },
  { id: 'about', label: '关于系统', icon: 'ⓘ' },
]

// ── 媒体库连接与扫描指南交互 ─────────────────────────────────
const selectedOs = ref<'windows' | 'linux' | 'macos'>('windows')
const copied = ref(false)

const scanCommand = computed(() => {
  if (selectedOs.value === 'windows') {
    return 'whatmedia-cli scan --path "D:/Media" --probe'
  }
  if (selectedOs.value === 'macos') {
    return 'whatmedia-cli scan --path "/Users/username/Movies" --probe'
  }
  return 'whatmedia-cli scan --path "/media" --probe'
})

async function copyCommand(): Promise<void> {
  try {
    await navigator.clipboard.writeText(scanCommand.value)
    copied.value = true
    setTimeout(() => {
      copied.value = false
    }, 2000)
  } catch {
    // 降级支持剪贴板失败静默
  }
}
</script>

<template>
  <div class="settings-page">
    <div class="page-header">
      <div>
        <h1>设置</h1>
        <p>根据你的观看习惯调整偏好，并查看服务端转码与存储自检报告。</p>
      </div>
    </div>

    <p v-if="error" class="settings-error-banner">
      连不上服务端：{{ error }}
    </p>

    <div class="settings-layout">
      <!-- 左侧分类侧栏 (ui.html .settings-nav) -->
      <nav class="settings-nav" aria-label="设置分类">
        <button
          v-for="t in tabs"
          :key="t.id"
          type="button"
          :class="{ active: activeTab === t.id }"
          @click="activeTab = t.id"
        >
          <span class="nav-ico">{{ t.icon }}</span>
          <span>{{ t.label }}</span>
        </button>
      </nav>

      <!-- 右侧设置内容区 (ui.html .settings-content) -->
      <div class="settings-content">
        <!-- 1. 服务端健康状态 -->
        <section
          v-if="(activeTab === 'all' || activeTab === 'server') && health"
          class="settings-section"
        >
          <div class="section-title">服务端</div>
          <div class="settings-dl">
            <div class="settings-row">
              <div>
                <strong>状态</strong>
                <p>当前后台运行状态</p>
              </div>
              <div class="row-val">
                <span
                  class="status-dot"
                  :style="{ background: health.status === 'ok' ? 'var(--good)' : '#f87171' }"
                />
                <span>{{ health.status }}</span>
              </div>
            </div>

            <div class="settings-row">
              <div>
                <strong>版本</strong>
                <p>服务端构建版本</p>
              </div>
              <span class="row-val font-mono">{{ health.version }}</span>
            </div>

            <div class="settings-row">
              <div>
                <strong>运行环境</strong>
                <p>架构与操作系统平台</p>
              </div>
              <span class="row-val font-mono">{{ health.arch }} / {{ health.os }}</span>
            </div>

            <div class="settings-row">
              <div>
                <strong>运行时长</strong>
                <p>自上次服务启动至今</p>
              </div>
              <span class="row-val">{{ fmtUptime(health.uptime_secs) }}</span>
            </div>
          </div>
        </section>

        <!-- 2. 转码与硬件加速 -->
        <section
          v-if="(activeTab === 'all' || activeTab === 'hardware') && hardware"
          class="settings-section"
        >
          <div class="section-title">转码能力</div>

          <p v-if="!hardware.ffmpeg" class="warn-box">
            服务端没有检测到 ffmpeg —— 所有需要重新封装或转码的片源都放不了，
            只有浏览器能直解的 MP4/WebM + H264/VP9 + AAC/Opus 可以播。
          </p>

          <div class="settings-dl">
            <div class="settings-row">
              <div>
                <strong>ffmpeg 状态</strong>
                <p>转码执行文件</p>
              </div>
              <span class="row-val font-mono text-break">{{ hardware.ffmpeg ?? '未检测到' }}</span>
            </div>

            <div class="settings-row">
              <div>
                <strong>可用后端 / 设备节点</strong>
                <p>硬件探测设备</p>
              </div>
              <span class="row-val">{{ hardware.profiles.length }} 个后端 · {{ hardware.render_node ?? '—' }}</span>
            </div>

            <div class="settings-row">
              <div>
                <strong>并发与负载</strong>
                <p>最大并发会话 / 活动转码</p>
              </div>
              <span class="row-val">{{ hardware.active_transcodes }} / {{ hardware.capacity }} 路</span>
            </div>
          </div>

          <!-- 硬件加速档位 -->
          <div v-if="hardware.profiles.length" class="hw-profiles-block">
            <div class="block-subtitle">探测到的硬件档</div>
            <div class="hw-grid">
              <div
                v-for="(p, i) in hardware.profiles"
                :key="i"
                class="hw-card"
              >
                <div class="hw-card-head">
                  <strong>{{ kindLabels[p.kind] ?? p.kind }}</strong>
                  <span class="chip">{{ p.max_concurrent_sessions }} 路并发</span>
                </div>
                <div class="hw-encoders">
                  <span v-for="(c, n) in p.encoders" :key="n" class="chip">
                    {{ codecLabel(c) }}
                  </span>
                </div>
                <div class="hw-features">
                  <span v-if="p.supports_zero_copy" class="feature-good">✓ 零拷贝</span>
                  <span v-if="p.supports_tone_mapping" class="feature-cyan">✓ 色调映射</span>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- 3. 存储与媒体库 -->
        <section
          v-if="activeTab === 'all' || activeTab === 'storage'"
          class="settings-section"
        >
          <div class="section-title">存储与媒体库</div>

          <p v-if="storageError" class="warn-box">
            {{ storageError }}
          </p>

          <template v-if="storage">
            <div v-if="storage.libraries.length" class="storage-list">
              <div v-for="l in storage.libraries" :key="l.id" class="storage-item">
                <div class="storage-item-head">
                  <strong>{{ l.name }}</strong>
                  <span class="storage-item-num">
                    {{ fmtBytes(l.bytes) }} · {{ l.item_count }} 条目 / {{ l.file_count }} 文件
                  </span>
                </div>
                <div class="server-meter" style="height: 6px; margin: 8px 0;">
                  <span :style="{ width: `${Math.max(2, shareOf(l.bytes))}%` }" />
                </div>
                <div class="storage-path">{{ l.root_path }}</div>
                <p v-if="!l.reachable" class="storage-unreachable">
                  目录当前不可访问 —— 多半是盘没挂载
                </p>
              </div>
            </div>

            <!-- 媒体库接入与扫描命令行引导卡片 -->
            <div class="library-guide-card">
              <div class="guide-header">
                <div class="guide-badge-wrap">
                  <span class="guide-badge">开始使用</span>
                  <span class="guide-sub-badge">媒体库连接指南</span>
                </div>
                <div class="os-switcher">
                  <button
                    type="button"
                    class="os-btn"
                    :class="{ active: selectedOs === 'windows' }"
                    @click="selectedOs = 'windows'"
                  >
                    Windows
                  </button>
                  <button
                    type="button"
                    class="os-btn"
                    :class="{ active: selectedOs === 'linux' }"
                    @click="selectedOs = 'linux'"
                  >
                    Linux / Docker
                  </button>
                  <button
                    type="button"
                    class="os-btn"
                    :class="{ active: selectedOs === 'macos' }"
                    @click="selectedOs = 'macos'"
                  >
                    macOS
                  </button>
                </div>
              </div>

              <h3 class="guide-title">把你的媒体目录交给 WhatMedia</h3>
              <p class="guide-desc">
                扫描一个目录即建成媒体库，之后自动识别影片信息、抓取海报，
                并按设备能力决定直接播放还是转码。接入你的实际媒体后，界面将自动展示你的私人片库。
              </p>

              <div class="cli-box">
                <div class="cli-head">
                  <span class="cli-tip">在服务端所在机器的终端执行：</span>
                  <button type="button" class="copy-btn" @click="copyCommand">
                    <svg v-if="!copied" viewBox="0 0 24 24" width="13" height="13">
                      <rect x="9" y="9" width="13" height="13" rx="2" ry="2" fill="none" stroke="currentColor" stroke-width="2" />
                      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" fill="none" stroke="currentColor" stroke-width="2" />
                    </svg>
                    <svg v-else viewBox="0 0 24 24" width="13" height="13">
                      <polyline points="20 6 9 17 4 12" fill="none" stroke="var(--good)" stroke-width="2" />
                    </svg>
                    <span>{{ copied ? '已复制命令' : '复制命令' }}</span>
                  </button>
                </div>
                <pre class="cli-code"><code>{{ scanCommand }}</code></pre>
              </div>

              <div class="guide-tmdb-tip">
                <span class="tip-icon">💡</span>
                <div>
                  <strong>海报与元数据自动补全</strong>
                  <p>
                    想让海报和简介也自动补全，配置环境变量
                    <code>WHATMEDIA_METADATA_TMDB_API_KEY</code>
                    后重扫即可自动刮削。
                  </p>
                </div>
              </div>
            </div>

            <div class="settings-dl" style="margin-top: 18px;">
              <div class="settings-row">
                <div>
                  <strong>缓存（图片 + HLS 分片）</strong>
                  <p>临时解码与转码缓存</p>
                </div>
                <span class="row-val font-mono">
                  {{ storage.cache.unavailable ? '暂无' : fmtBytes(storage.cache.bytes) }}
                </span>
              </div>
              <div class="settings-row">
                <div>
                  <strong>合计已用存储</strong>
                  <p>当前所有媒体库总占用</p>
                </div>
                <strong class="row-val font-mono" style="color: var(--cyan); font-size: 14px;">
                  {{ fmtBytes(storage.total_bytes) }}
                </strong>
              </div>
            </div>
          </template>
        </section>

        <!-- 4. 可写设置 -->
        <section
          v-if="activeTab === 'all' || activeTab === 'entries'"
          class="settings-section"
        >
          <div class="section-title">可写设置</div>
          <p class="section-desc">
            这里能改的只有服务端白名单里的项。路径、端口、数据库地址与密钥
            <strong>不在其中</strong> —— 那几项改坏之后界面就打不开了，
            无从改回，所以只能去服务端的配置文件或环境变量里改。
          </p>

          <p v-if="settingsError" class="error-box">
            {{ settingsError }}
          </p>

          <p v-if="settings?.rejected.length" class="warn-box">
            以下覆盖项在启动时被忽略（值不合法或不在白名单内），当前未生效：
            <span v-for="(r, i) in settings.rejected" :key="i" class="code-line">
              · {{ r }}
            </span>
          </p>

          <div v-if="userEntries.length" class="entries-group">
            <div class="group-title">播放偏好（只影响你自己）</div>
            <SettingControl
              v-for="e in userEntries"
              :key="e.key"
              :entry="e"
              @save="(v) => saveSetting(e, v)"
              @reset="resetSetting(e)"
            />
          </div>

          <div v-if="serverEntries.length" class="entries-group">
            <div class="group-title">全服设置（仅管理员可见）</div>
            <SettingControl
              v-for="e in serverEntries"
              :key="e.key"
              :entry="e"
              @save="(v) => saveSetting(e, v)"
              @reset="resetSetting(e)"
            />
          </div>
        </section>

        <!-- 5. AI 助手 -->
        <section
          v-if="activeTab === 'all' || activeTab === 'ai'"
          class="settings-section"
        >
          <div class="section-title">AI 助手</div>

          <p v-if="aiError" class="warn-box">
            {{ aiError }}
          </p>

          <div class="ai-stat-grid">
            <div class="ai-stat-card">
              <strong :style="{ color: aiError ? 'var(--muted)' : 'var(--good)' }">
                {{ aiError ? '不可用' : '可用' }}
              </strong>
              <span>状态</span>
            </div>
            <div class="ai-stat-card">
              <strong>{{ aiTools?.length ?? 0 }}</strong>
              <span>可用工具</span>
            </div>
            <div class="ai-stat-card">
              <strong :style="{ color: dangerousCount() ? '#f59e0b' : 'var(--muted)' }">
                {{ dangerousCount() }}
              </strong>
              <span>写入类工具</span>
            </div>
          </div>

          <p class="section-desc" style="margin-top: 14px;">
            没配模型时 AI 不会失效，而是退回内置规则解析 —— 「找出没有海报的电影」
            这类问法照样答得出来，回答上会标“离线模式”。
            会改动媒体库的操作一律先给预览、再要你确认，没有例外。
            <span v-if="!aiError && dangerousCount() === 0">
              当前清单里全是只读工具：危险操作没开启时根本不会出现，这是设计而不是故障。
            </span>
          </p>
        </section>

        <!-- 6. 桌面播放器（如果处于 Tauri 环境中） -->
        <section
          v-if="(activeTab === 'all' || activeTab === 'about') && inTauri"
          class="settings-section"
        >
          <div class="section-title">桌面播放器（mpv）</div>

          <p v-if="mpv && !mpv.available" class="warn-box">
            {{ mpv.reason ?? '没有找到 mpv' }} —— 已回落 Web 播放器，
            能力也按 Web 上报，服务端不会给出浏览器放不出来的 Direct Play。
          </p>

          <div class="settings-dl">
            <div class="settings-row">
              <div>
                <strong>mpv 状态</strong>
                <p>外部渲染引擎</p>
              </div>
              <span class="row-val">
                <span
                  class="status-dot"
                  :style="{ background: mpv?.available ? 'var(--good)' : '#f87171' }"
                />
                {{ mpv ? (mpv.available ? '可用' : '不可用') : '探测中…' }}
              </span>
            </div>
            <template v-if="mpv?.available">
              <div class="settings-row">
                <div>
                  <strong>引擎版本</strong>
                  <p>本地 mpv 二进制版本</p>
                </div>
                <span class="row-val font-mono">{{ mpv.version }}</span>
              </div>
              <div class="settings-row">
                <div>
                  <strong>二进制路径</strong>
                  <p>执行路径</p>
                </div>
                <span class="row-val font-mono text-break">{{ mpv.path }}</span>
              </div>
            </template>
          </div>
        </section>

        <!-- 7. 关于 WhatMedia -->
        <section
          v-if="activeTab === 'all' || activeTab === 'about'"
          class="settings-section"
        >
          <div class="section-title">关于这个版本</div>
          <p class="section-desc" style="font-size: 13px; line-height: 1.8;">
            前端默认使用 Web 播放器（<code>&lt;video&gt;</code> + hls.js）。
            桌面壳里若探测到 mpv 会自动切到 mpv 后端，并把能力按
            <code>tauri-mpv</code> 上报，服务端据此决定是否免转码；探测不到就按 Web 决策 —— 宁可多转一次码，
            也不给一个放不出来的地址。
          </p>
        </section>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-page {
  padding-bottom: 30px;
}
.settings-error-banner {
  border-radius: 12px;
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(127, 29, 29, 0.35);
  padding: 12px 16px;
  font-size: 13px;
  color: #fca5a5;
  margin-bottom: 18px;
}

.nav-ico {
  font-size: 13px;
  width: 16px;
  display: inline-block;
  text-align: center;
}

.settings-section {
  padding: 16px 8px;
  border-bottom: 1px solid var(--line);
}
.settings-section:first-child {
  padding-top: 4px;
}
.settings-section:last-child {
  border-bottom: 0;
}

.section-desc {
  font-size: 12px;
  line-height: 1.7;
  color: var(--muted);
  margin: 8px 0 16px;
}

.settings-dl {
  display: flex;
  flex-direction: column;
}

.row-val {
  font-size: 12px;
  color: var(--text);
  display: inline-flex;
  align-items: center;
  gap: 8px;
  text-align: right;
}
.text-break {
  word-break: break-all;
  max-width: 320px;
}

.warn-box {
  margin: 12px 0;
  padding: 10px 14px;
  border-radius: 10px;
  border: 1px solid rgba(245, 158, 11, 0.35);
  background: rgba(245, 158, 11, 0.1);
  color: #fcd34d;
  font-size: 12px;
  line-height: 1.6;
}
.error-box {
  margin: 12px 0;
  padding: 10px 14px;
  border-radius: 10px;
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(127, 29, 29, 0.35);
  color: #fca5a5;
  font-size: 12px;
}
.code-line {
  display: block;
  font-family: monospace;
  font-size: 11px;
  margin-top: 4px;
}

/* 硬件卡片 */
.hw-profiles-block {
  margin-top: 18px;
}
.block-subtitle {
  font-size: 12px;
  color: var(--muted);
  margin-bottom: 10px;
}
.hw-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}
.hw-card {
  padding: 12px;
  border-radius: 11px;
  background: #141f34;
  border: 1px solid var(--line);
}
.hw-card-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.hw-card-head strong {
  font-size: 12px;
  color: var(--text);
}
.hw-encoders {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  margin: 8px 0;
}
.hw-features {
  display: flex;
  gap: 10px;
  font-size: 10px;
}
.feature-good {
  color: #34d399;
}
.feature-cyan {
  color: #38bdf8;
}

/* 存储条目 */
.storage-list {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin-top: 14px;
}
.storage-item {
  padding: 12px 14px;
  border-radius: 11px;
  background: #141f34;
  border: 1px solid var(--line);
}
.storage-item-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  font-size: 12px;
}
.storage-item-num {
  font-family: monospace;
  font-size: 11px;
  color: var(--muted);
}
.storage-path {
  font-family: monospace;
  font-size: 10px;
  color: var(--muted2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.storage-unreachable {
  font-size: 11px;
  color: #f87171;
  margin: 4px 0 0;
}
.empty-hint {
  font-size: 12px;
  color: var(--muted);
  margin-top: 12px;
}

/* 媒体库连接指南卡片 */
.library-guide-card {
  margin-top: 16px;
  padding: 22px 24px;
  border-radius: 14px;
  background: linear-gradient(145deg, rgba(20, 31, 52, 0.95), rgba(16, 25, 42, 0.98));
  border: 1px solid rgba(121, 91, 255, 0.22);
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.25);
  position: relative;
  overflow: hidden;
}
.library-guide-card::before {
  content: "";
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 2px;
  background: linear-gradient(90deg, var(--purple2), var(--cyan), transparent);
}
.guide-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 12px;
}
.guide-badge-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
}
.guide-badge {
  font-size: 11px;
  font-weight: 700;
  padding: 3px 9px;
  border-radius: 6px;
  background: rgba(121, 91, 255, 0.18);
  border: 1px solid rgba(154, 131, 255, 0.35);
  color: #c5b8ff;
  letter-spacing: 0.05em;
}
.guide-sub-badge {
  font-size: 11px;
  color: var(--muted);
}
.os-switcher {
  display: inline-flex;
  background: rgba(8, 14, 26, 0.7);
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 3px;
  gap: 3px;
}
.os-btn {
  padding: 4px 10px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 6px;
  border: none;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  transition: all 0.15s ease;
}
.os-btn:hover {
  color: #fff;
}
.os-btn.active {
  background: rgba(121, 91, 255, 0.3);
  color: #fff;
  border: 1px solid rgba(154, 131, 255, 0.4);
}
.guide-title {
  font-size: 17px;
  font-weight: 700;
  color: #fff;
  margin: 0 0 8px;
  letter-spacing: -0.3px;
}
.guide-desc {
  font-size: 12px;
  line-height: 1.7;
  color: var(--muted);
  margin: 0 0 16px;
  max-width: 640px;
}
.cli-box {
  background: rgba(8, 13, 24, 0.85);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 12px 14px;
}
.cli-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}
.cli-tip {
  font-size: 11px;
  color: var(--muted);
}
.copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 6px;
  color: #e2e8f0;
  font-size: 11px;
  padding: 4px 10px;
  cursor: pointer;
  transition: all 0.15s ease;
}
.copy-btn:hover {
  background: rgba(121, 91, 255, 0.2);
  border-color: rgba(154, 131, 255, 0.4);
  color: #fff;
}
.cli-code {
  margin: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
  color: #a5f3fc;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-all;
}
.guide-tmdb-tip {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-top: 14px;
  padding: 10px 12px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.02);
  border: 1px dashed rgba(255, 255, 255, 0.12);
  font-size: 11px;
  color: var(--muted);
  line-height: 1.6;
}
.guide-tmdb-tip .tip-icon {
  font-size: 14px;
  flex-shrink: 0;
}
.guide-tmdb-tip strong {
  display: block;
  color: var(--text);
  margin-bottom: 2px;
}
.guide-tmdb-tip p {
  margin: 0;
}
.guide-tmdb-tip code {
  background: rgba(0, 0, 0, 0.3);
  padding: 2px 6px;
  border-radius: 4px;
  color: #fdba74;
  font-family: monospace;
}

/* 设置分组 */
.entries-group {
  margin-top: 18px;
}
.group-title {
  font-size: 12px;
  font-weight: 700;
  color: var(--muted);
  margin-bottom: 6px;
}

/* AI 助手网格卡片 */
.ai-stat-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
  margin-top: 12px;
}
.ai-stat-card {
  padding: 14px;
  border-radius: 12px;
  background: #141f34;
  border: 1px solid var(--line);
  text-align: center;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.ai-stat-card strong {
  font-size: 18px;
}
.ai-stat-card span {
  font-size: 11px;
  color: var(--muted);
}

@media (max-width: 620px) {
  .hw-grid {
    grid-template-columns: 1fr;
  }
}
</style>

