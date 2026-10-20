<script setup lang="ts">
/**
 * 设置页。
 *
 * 这里展示的是**服务端自检的结果**，不是配置项编辑器。
 * 把硬件探测结果暴露给用户是有意的：当"为什么这个片要转码"发生时，
 * 用户能自己看到"服务端只检测到软件编码"，比去翻日志强得多。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import * as api from '../api/client'
import DirectoryPickerModal from '../components/DirectoryPickerModal.vue'
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

// 按业务分类的可写设置
const metadataEntries = computed(() =>
  serverEntries.value.filter((e) => e.key.startsWith('metadata.')),
)
const transcodeCacheEntries = computed(() =>
  serverEntries.value.filter(
    (e) => e.key.startsWith('transcode.') || e.key.startsWith('images.'),
  ),
)
const aiEntries = computed(() =>
  serverEntries.value.filter((e) => e.key.startsWith('ai.')),
)
const otherServerEntries = computed(() =>
  serverEntries.value.filter(
    (e) =>
      !e.key.startsWith('metadata.') &&
      !e.key.startsWith('transcode.') &&
      !e.key.startsWith('images.') &&
      !e.key.startsWith('ai.'),
  ),
)

// 分组折叠控制：播放偏好与元数据默认展开，低频运维默认折叠
const collapsedGroups = ref<Record<string, boolean>>({
  user: false,
  metadata: false,
  transcodeCache: true,
  ai: false,
  otherServer: true,
})

function toggleGroup(key: string): void {
  collapsedGroups.value[key] = !collapsedGroups.value[key]
}

// ── AI 模型服务商预设与连接测试 ──────────────────────────────
interface ProviderPreset {
  id: string
  name: string
  badge?: string
  icon: string
  desc: string
  defaultBaseUrl: string
  defaultModel: string
  needsKey: boolean
}

const providerPresets: ProviderPreset[] = [
  {
    id: 'deepseek',
    name: 'DeepSeek',
    badge: '首选推荐 · 高性价比',
    icon: '🐋',
    desc: '国内顶尖大模型，超强代码与逻辑推理能力，极高性价比。',
    defaultBaseUrl: 'https://api.deepseek.com/v1',
    defaultModel: 'deepseek-chat',
    needsKey: true,
  },
  {
    id: 'openai',
    name: 'OpenAI',
    badge: '业界标杆',
    icon: '✨',
    desc: '全球主流通用大模型（GPT-4o、GPT-4o-mini 等）。',
    defaultBaseUrl: 'https://api.openai.com/v1',
    defaultModel: 'gpt-4o-mini',
    needsKey: true,
  },
  {
    id: 'qwen',
    name: '通义千问 (Qwen)',
    badge: '阿里云',
    icon: '☁️',
    desc: '阿里云通义大模型，中文影视理解与自然语言检索表现出色。',
    defaultBaseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    defaultModel: 'qwen-plus',
    needsKey: true,
  },
  {
    id: 'glm',
    name: '智谱 GLM',
    badge: '智谱 AI',
    icon: '🧠',
    desc: '智谱清言通用大模型平台，具备出色的上下文理解能力。',
    defaultBaseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    defaultModel: 'glm-4-flash',
    needsKey: true,
  },
  {
    id: 'minimax',
    name: 'MiniMax',
    badge: '万亿 MoE · 超长上下文',
    icon: '🚀',
    desc: 'MiniMax 稀宇科技大模型，中文理解、逻辑推理及超长文本能力强劲。',
    defaultBaseUrl: 'https://api.minimax.chat/v1',
    defaultModel: 'MiniMax-Text-01',
    needsKey: true,
  },
  {
    id: 'ollama',
    name: 'Ollama',
    badge: '本地离线 · 免Key',
    icon: '🦙',
    desc: '本地私有化部署模型（Qwen2.5、Llama3 等），数据不出内网。',
    defaultBaseUrl: 'http://localhost:11434',
    defaultModel: 'qwen2.5:7b',
    needsKey: false,
  },
  {
    id: 'vllm',
    name: 'vLLM / llama.cpp',
    badge: '自建私有',
    icon: '⚡',
    desc: '局域网私有搭建的高性能 OpenAI 兼容推理服务。',
    defaultBaseUrl: 'http://localhost:8000/v1',
    defaultModel: 'qwen2.5-7b-instruct',
    needsKey: false,
  },
  {
    id: 'custom',
    name: '自定义网关 (Custom)',
    badge: '兼容代理',
    icon: '🔧',
    desc: '使用第三方中转网关、One-API 或专属代理接入点。',
    defaultBaseUrl: '',
    defaultModel: '',
    needsKey: true,
  },
]

const currentProvider = computed(() => {
  const e = aiEntries.value.find((x) => x.key === 'ai.provider')
  return e?.value || 'openai'
})

const currentPresetName = computed(() => {
  const p = providerPresets.find((x) => x.id === currentProvider.value)
  return p ? p.name : currentProvider.value
})

const currentModelName = computed(() => {
  const e = aiEntries.value.find((x) => x.key === 'ai.model')
  return e?.value || ''
})

async function applyProviderPreset(preset: ProviderPreset): Promise<void> {
  const pEntry = aiEntries.value.find((x) => x.key === 'ai.provider')
  if (pEntry) await saveSetting(pEntry, preset.id)

  const urlEntry = aiEntries.value.find((x) => x.key === 'ai.base_url')
  if (urlEntry && preset.defaultBaseUrl) {
    await saveSetting(urlEntry, preset.defaultBaseUrl)
  }

  const modelEntry = aiEntries.value.find((x) => x.key === 'ai.model')
  if (modelEntry && preset.defaultModel) {
    await saveSetting(modelEntry, preset.defaultModel)
  }
}

const testingAi = ref(false)
const testResult = ref<{ ok: boolean; message: string; latency?: number } | null>(null)

async function testAiConnection(): Promise<void> {
  testingAi.value = true
  testResult.value = null
  const start = Date.now()
  try {
    const res = await api.askAi('你好！请回复一句话测试连接。')
    const latency = Date.now() - start
    const snippet = res.answer
      ? res.answer.length > 80
        ? res.answer.slice(0, 80) + '…'
        : res.answer
      : '响应成功'
    testResult.value = {
      ok: true,
      message: `${res.degraded ? '⚠️ 本地规则兜底（未连上大模型）' : '✓ 模型连通成功'}（耗时 ${latency}ms）：${snippet}`,
      latency,
    }
  } catch (err) {
    testResult.value = {
      ok: false,
      message: err instanceof api.ApiError ? err.detail.message : String(err),
    }
  } finally {
    testingAi.value = false
  }
}

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
  void refreshTasks()
  void loadScrapeStats()
  void loadLiveSources()

  // 自动恢复上次配置或保存的媒体库物理路径
  try {
    const cachedPath = localStorage.getItem('whatmedia_last_scan_path')
    if (cachedPath && !scanForm.value.path) {
      scanForm.value.path = cachedPath
    }
  } catch {
    // 忽略 localStorage 受限环境
  }

  if (inTauri) mpv.value = await probeMpv()
})

onBeforeUnmount(() => {
  if (taskPollTimer) {
    clearInterval(taskPollTimer)
    taskPollTimer = null
  }
  if (scrapePollTimer) {
    clearInterval(scrapePollTimer)
    scrapePollTimer = null
  }
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
  { id: 'metadata', label: '元数据与TMDB', icon: '🍿' },
  { id: 'live', label: '电视直播', icon: '📡' },
  { id: 'entries', label: '可写设置', icon: '⚙' },
  { id: 'ai', label: 'AI 助手', icon: '🤖' },
  { id: 'about', label: '关于系统', icon: 'ⓘ' },
]

// ── 媒体库连接与扫描指南交互 ─────────────────────────────────
const selectedOs = ref<'windows' | 'linux' | 'macos'>('windows')
const copied = ref(false)

const scanCommand = computed(() => {
  if (selectedOs.value === 'windows') {
    return 'whatmedia scan --path "D:/Media" --probe'
  }
  if (selectedOs.value === 'macos') {
    return 'whatmedia scan --path "/Users/username/Movies" --probe'
  }
  return 'whatmedia scan --path "/media" --probe'
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

// ── Web 端直接添加与扫盘入库 ──────────────────────────────────
const showDirPicker = ref(false)
const scanForm = ref({
  name: '',
  path: '',
  probe: true,
  concurrency: 4,
})
const scanning = ref(false)
const scanFeedback = ref<{ type: 'ok' | 'err'; message: string } | null>(null)
const activeTasks = ref<api.ScanTaskView[]>([])
let taskPollTimer: ReturnType<typeof setInterval> | null = null

const savingLib = ref(false)

function openDirPicker(): void {
  showDirPicker.value = true
}

function onDirSelected(path: string): void {
  scanForm.value.path = path
  try {
    localStorage.setItem('whatmedia_last_scan_path', path)
  } catch {
    // 忽略静默
  }
  if (!scanForm.value.name.trim()) {
    // 自动将末级目录名填为媒体库名称
    const normalized = path.replace(/[\\/]+$/, '')
    const segs = normalized.split(/[\\/]/)
    const last = segs[segs.length - 1]
    if (last) {
      scanForm.value.name = last
    }
  }
}

/** 仅保存物理路径与媒体库配置，暂不启动耗资源的扫描。 */
async function saveLibraryPath(): Promise<void> {
  const p = scanForm.value.path.trim()
  if (!p) {
    scanFeedback.value = { type: 'err', message: '请指定服务端物理路径' }
    return
  }
  savingLib.value = true
  scanFeedback.value = null
  try {
    const normalized = p.replace(/[\\/]+$/, '')
    const segs = normalized.split(/[\\/]/)
    const last = segs[segs.length - 1]
    const name = scanForm.value.name.trim() || last || '媒体库'
    const lib = await api.createLibrary({
      name,
      path: p,
    })
    try {
      localStorage.setItem('whatmedia_last_scan_path', p)
    } catch {
      // 忽略静默
    }
    scanFeedback.value = {
      type: 'ok',
      message: `✓ 媒体库「${lib.name}」已成功持久化保存（物理路径：${lib.root_path}）`,
    }
    await loadStorage()
  } catch (err) {
    scanFeedback.value = {
      type: 'err',
      message: err instanceof api.ApiError ? err.detail.message : String(err),
    }
  } finally {
    savingLib.value = false
  }
}

async function triggerScan(targetPath?: string, targetName?: string): Promise<void> {
  const p = (targetPath || scanForm.value.path).trim()
  if (!p) {
    scanFeedback.value = { type: 'err', message: '请指定服务端物理路径' }
    return
  }
  scanning.value = true
  scanFeedback.value = null
  try {
    try {
      localStorage.setItem('whatmedia_last_scan_path', p)
    } catch {
      // 忽略静默
    }
    await api.startScan({
      path: p,
      name: (targetName || scanForm.value.name).trim() || undefined,
      probe: scanForm.value.probe,
      concurrency: scanForm.value.concurrency,
    })
    scanFeedback.value = {
      type: 'ok',
      message: `已向服务端提交入库任务：${p}，后台正在扫描中...`,
    }
    ensureTaskPolling()
  } catch (err) {
    scanFeedback.value = {
      type: 'err',
      message: err instanceof api.ApiError ? err.detail.message : String(err),
    }
  } finally {
    scanning.value = false
  }
}

// 过滤出正在扫描中的在途任务，已完成的自动隐藏，避免反复累加
const ongoingTasks = computed(() => {
  if (!Array.isArray(activeTasks.value)) return []
  return activeTasks.value.filter((t) => !t.progress.finished)
})

async function refreshTasks(): Promise<void> {
  try {
    const list = await api.listTasks()
    activeTasks.value = Array.isArray(list) ? list : []
    const hasUnfinished = activeTasks.value.some((t) => !t.progress.finished)
    if (!hasUnfinished && taskPollTimer) {
      clearInterval(taskPollTimer)
      taskPollTimer = null
      // 扫描全部完成，更新存储面板数据
      void loadStorage()
    }
  } catch {
    // 轮询异常静默
  }
}

function ensureTaskPolling(): void {
  void refreshTasks()
  if (!taskPollTimer) {
    taskPollTimer = setInterval(refreshTasks, 2000)
  }
}

// ── 元数据刮削与 TMDB 交互 ─────────────────────────────────
const scrapeLibraryId = ref<string>('')
const retryFailedOnly = ref(false)
const scraping = ref(false)
const scrapeFeedback = ref<{ type: 'ok' | 'err'; message: string } | null>(null)
const scrapeStats = ref<api.JobStats | null>(null)
const scrapeRuns = ref<api.ScrapeRunView[]>([])
let scrapePollTimer: ReturnType<typeof setInterval> | null = null

async function loadScrapeStats(): Promise<void> {
  try {
    const libId = scrapeLibraryId.value ? scrapeLibraryId.value : undefined
    const [st, runs] = await Promise.all([
      api.getScrapeStats(libId),
      api.listScrapeRuns().catch(() => [] as api.ScrapeRunView[]),
    ])
    scrapeStats.value = st
    scrapeRuns.value = runs
  } catch {
    // 静默允许
  }
}

async function triggerManualScrape(): Promise<void> {
  scraping.value = true
  scrapeFeedback.value = null
  try {
    const res = await api.startScrape({
      library_id: scrapeLibraryId.value ? scrapeLibraryId.value : null,
      retry_failed: retryFailedOnly.value,
    })
    scrapeFeedback.value = {
      type: 'ok',
      message: `✓ 刮削任务已受理：入队 ${res.queued} 项条目（批次：${res.run_id.slice(0, 8)}…），后台刮削中`,
    }
    ensureScrapePolling()
  } catch (err) {
    scrapeFeedback.value = {
      type: 'err',
      message: err instanceof api.ApiError ? err.detail.message : String(err),
    }
  } finally {
    scraping.value = false
  }
}

function ensureScrapePolling(): void {
  void loadScrapeStats()
  if (!scrapePollTimer) {
    scrapePollTimer = setInterval(async () => {
      await loadScrapeStats()
      const hasRunning = scrapeRuns.value.some((r) => r.running)
      if (!hasRunning && scrapePollTimer) {
        clearInterval(scrapePollTimer)
        scrapePollTimer = null
      }
    }, 2500)
  }
}

watch(scrapeLibraryId, () => {
  void loadScrapeStats()
})

// ── 电视直播订阅源配置 ──────────────────────────────────────
const liveSources = ref<api.LiveSource[]>([])
const loadingLiveSources = ref(false)
const liveError = ref<string | null>(null)
const liveNotice = ref<string | null>(null)
const liveBusy = ref(false)
const syncingSourceId = ref<string | null>(null)

const newSourceName = ref('')
const newSourceUrl = ref('')
const newSourceEpg = ref('')

async function loadLiveSources(): Promise<void> {
  loadingLiveSources.value = true
  liveError.value = null
  try {
    const list = await api.listLiveSources()
    liveSources.value = Array.isArray(list) ? list : []
  } catch (e) {
    liveError.value = e instanceof Error ? e.message : String(e)
  } finally {
    loadingLiveSources.value = false
  }
}

async function addLiveSource(): Promise<void> {
  const name = newSourceName.value.trim()
  const url = newSourceUrl.value.trim()
  if (!name || !url) {
    liveNotice.value = '请填写订阅源名称和有效的播放列表 URL'
    return
  }
  liveBusy.value = true
  liveNotice.value = null
  try {
    const src = await api.createLiveSource({
      name,
      kind: 'm3u',
      url,
      ...(newSourceEpg.value.trim() ? { epg_url: newSourceEpg.value.trim() } : {}),
    })
    newSourceName.value = ''
    newSourceUrl.value = ''
    newSourceEpg.value = ''
    liveNotice.value = `已添加订阅源「${src.name}」，正在自动同步频道...`
    try {
      const res = await api.syncLiveSource(src.id)
      liveNotice.value = `「${src.name}」同步完成，成功导入 ${res.channels} 个频道！`
    } catch (syncErr) {
      liveNotice.value = `添加成功，但首次同步遇到问题：${syncErr instanceof Error ? syncErr.message : String(syncErr)}`
    }
    await loadLiveSources()
  } catch (e) {
    liveNotice.value = `添加失败：${e instanceof Error ? e.message : String(e)}`
  } finally {
    liveBusy.value = false
  }
}

async function syncSingleLiveSource(id: string): Promise<void> {
  syncingSourceId.value = id
  liveNotice.value = null
  try {
    const res = await api.syncLiveSource(id)
    liveNotice.value = `同步成功，当前包含 ${res.channels} 个频道`
    await loadLiveSources()
  } catch (e) {
    liveNotice.value = `同步失败：${e instanceof Error ? e.message : String(e)}`
  } finally {
    syncingSourceId.value = null
  }
}

async function deleteSingleLiveSource(id: string, name: string): Promise<void> {
  if (!confirm(`确定要删除直播订阅源「${name}」吗？关联的所有电视频道将被清空。`)) {
    return
  }
  liveBusy.value = true
  liveNotice.value = null
  try {
    await api.deleteLiveSource(id)
    liveNotice.value = `已删除「${name}」`
    await loadLiveSources()
  } catch (e) {
    liveNotice.value = `删除失败：${e instanceof Error ? e.message : String(e)}`
  } finally {
    liveBusy.value = false
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
                  <div class="storage-title-wrap">
                    <strong>{{ l.name }}</strong>
                    <button
                      type="button"
                      class="rescan-btn"
                      title="立即触发增量重新入库"
                      :disabled="scanning"
                      @click="triggerScan(l.root_path, l.name)"
                    >
                      🔄 立即扫描
                    </button>
                  </div>
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

            <!-- Web 端直接添加媒体库与扫盘入库 -->
            <div class="web-scan-card">
              <div class="web-scan-head">
                <div>
                  <h3 class="web-scan-title">添加媒体库与扫盘入库</h3>
                  <p class="web-scan-sub">
                    无需在服务端机器命令行执行命令，直接在下方选择服务端物理路径即可快速入库。
                  </p>
                </div>
              </div>

              <div class="web-scan-form">
                <div class="scan-input-group">
                  <label class="scan-label">媒体库名称</label>
                  <input
                    v-model="scanForm.name"
                    type="text"
                    class="scan-input"
                    placeholder="选填，缺省使用文件夹名称（如：电影、剧集）"
                  />
                </div>

                <div class="scan-input-group">
                  <label class="scan-label">
                    <span>服务端物理路径</span>
                    <span class="required-star">*</span>
                  </label>
                  <div class="path-input-wrap">
                    <input
                      v-model="scanForm.path"
                      type="text"
                      class="scan-input path-input"
                      placeholder="服务端物理路径（如 D:/Media 或 /media）"
                    />
                    <button
                      type="button"
                      class="dir-picker-icon-btn"
                      title="打开服务端目录选择窗口"
                      @click="openDirPicker"
                    >
                      📁
                    </button>
                  </div>
                </div>

                <div class="scan-options-row">
                  <label class="scan-checkbox-label">
                    <input v-model="scanForm.probe" type="checkbox" />
                    <span>深度探测流与编码信息 (probe)</span>
                  </label>

                  <div class="scan-btn-group">
                    <button
                      type="button"
                      class="btn-save-lib"
                      :disabled="!scanForm.path.trim() || savingLib"
                      title="保存该物理路径为媒体库，暂不执行全盘扫描"
                      @click="saveLibraryPath"
                    >
                      <span>{{ savingLib ? '正在保存...' : '💾 保存媒体库' }}</span>
                    </button>

                    <button
                      type="button"
                      class="btn-start-scan"
                      :disabled="!scanForm.path.trim() || scanning"
                      title="保存该路径并立即启动全盘入库扫描"
                      @click="() => triggerScan()"
                    >
                      <span>{{ scanning ? '正在提交...' : '🚀 开始扫描入库' }}</span>
                    </button>
                  </div>
                </div>

                <div
                  v-if="scanFeedback"
                  class="scan-feedback"
                  :class="scanFeedback.type"
                >
                  <span>{{ scanFeedback.type === 'ok' ? '✓' : '⚠️' }}</span>
                  <span>{{ scanFeedback.message }}</span>
                </div>
              </div>

              <!-- 在途后台扫描任务（已完成自动隐藏，不累加常驻） -->
              <div v-if="ongoingTasks.length > 0" class="active-tasks-list">
                <div class="active-tasks-title">当前后台扫描任务 ({{ ongoingTasks.length }})</div>
                <div
                  v-for="task in ongoingTasks"
                  :key="task.id"
                  class="task-card"
                >
                  <div class="task-head">
                    <div class="task-lib-name">
                      <span class="task-dot" />
                      <strong>{{ task.library }}</strong>
                    </div>
                    <span class="task-status-text">
                      扫描中 ({{ task.progress.done }} / {{ task.progress.total || '?' }})
                    </span>
                  </div>

                  <div class="task-progress-bar">
                    <span
                      class="task-progress-inner"
                      :style="{
                        width: task.progress.total > 0
                          ? `${Math.min(100, Math.round((task.progress.done / task.progress.total) * 100))}%`
                          : '15%',
                      }"
                    />
                  </div>

                  <div class="task-stats-row">
                    <span>新增: {{ task.progress.added }}</span>
                    <span>更新: {{ task.progress.updated }}</span>
                    <span>跳过: {{ task.progress.skipped }}</span>
                    <span v-if="task.progress.failed > 0" class="stat-failed">失败: {{ task.progress.failed }}</span>
                  </div>
                </div>
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

        <!-- 4. 元数据与 TMDB 独立设置与手动刮削看板 -->
        <section
          v-if="activeTab === 'all' || activeTab === 'metadata'"
          class="settings-section"
        >
          <div class="section-title">元数据与 TMDB 刮削</div>
          <p class="section-desc">
            配置 TMDB 凭据与网络加速镜像，在界面一键触发全库或单库元数据刮削，并实时监控刮削进度指标。
          </p>

          <!-- 手动刮削控制台 -->
          <div class="web-scan-card scrape-console-card">
            <div class="web-scan-head">
              <div>
                <h3 class="web-scan-title">手动刮削控制台</h3>
                <p class="web-scan-sub">
                  无需等待计划任务，随时在界面向服务端提交刮削批次，精准补全影视标题、海报与演职员信息。
                </p>
              </div>
              <button
                type="button"
                class="refresh-stats-btn"
                title="刷新刮削统计指标"
                @click="loadScrapeStats"
              >
                🔄 刷新统计
              </button>
            </div>

            <div class="web-scan-form">
              <div class="scan-input-group">
                <label class="scan-label">目标媒体库</label>
                <select v-model="scrapeLibraryId" class="scan-input scan-select">
                  <option value="">全部媒体库 (全量扫描)</option>
                  <option
                    v-for="lib in storage?.libraries ?? []"
                    :key="lib.id"
                    :value="lib.id"
                  >
                    {{ lib.name }} ({{ lib.root_path }})
                  </option>
                </select>
              </div>

              <div class="scan-options-row">
                <label class="scan-checkbox-label">
                  <input v-model="retryFailedOnly" type="checkbox" />
                  <span>仅重试上次失败条目 (retry_failed)</span>
                </label>

                <div class="scan-btn-group">
                  <button
                    type="button"
                    class="btn-start-scan"
                    :disabled="scraping"
                    title="立即向服务端提交异步刮削任务"
                    @click="triggerManualScrape"
                  >
                    <span>{{ scraping ? '正在提交...' : '🚀 开始手动刮削' }}</span>
                  </button>
                </div>
              </div>

              <div
                v-if="scrapeFeedback"
                class="scan-feedback"
                :class="scrapeFeedback.type"
              >
                <span>{{ scrapeFeedback.type === 'ok' ? '✓' : '⚠️' }}</span>
                <span>{{ scrapeFeedback.message }}</span>
              </div>
            </div>

            <!-- 刮削指标卡片网格 -->
            <div v-if="scrapeStats" class="scrape-metrics-grid">
              <div class="scrape-metric-item">
                <span class="metric-num text-good">{{ scrapeStats.done }}</span>
                <span class="metric-lbl">已完成 (Done)</span>
              </div>
              <div class="scrape-metric-item">
                <span class="metric-num text-cyan">{{ scrapeStats.running }}</span>
                <span class="metric-lbl">刮削中 (Running)</span>
              </div>
              <div class="scrape-metric-item">
                <span class="metric-num">{{ scrapeStats.pending }}</span>
                <span class="metric-lbl">排队中 (Pending)</span>
              </div>
              <div class="scrape-metric-item">
                <span class="metric-num" :class="{ 'text-danger': scrapeStats.failed > 0 }">{{ scrapeStats.failed }}</span>
                <span class="metric-lbl">失败项 (Failed)</span>
              </div>
              <div class="scrape-metric-item">
                <span class="metric-num text-warn">{{ scrapeStats.review }}</span>
                <span class="metric-lbl">待复核 (Review)</span>
              </div>
              <div class="scrape-metric-item">
                <span class="metric-num text-muted">{{ scrapeStats.skipped }}</span>
                <span class="metric-lbl">跳过 (Skipped)</span>
              </div>
            </div>

            <!-- 在途活动刮削批次 -->
            <div v-if="scrapeRuns.filter(r => r.running).length > 0" class="active-tasks-list">
              <div class="active-tasks-title">当前后台刮削批次 ({{ scrapeRuns.filter(r => r.running).length }})</div>
              <div
                v-for="run in scrapeRuns.filter(r => r.running)"
                :key="run.id"
                class="task-card"
              >
                <div class="task-head">
                  <div class="task-lib-name">
                    <span class="task-dot" />
                    <strong>批次 {{ run.id.slice(0, 8) }}…</strong>
                  </div>
                  <span class="task-status-text">刮削执行中</span>
                </div>
                <div v-if="run.stats" class="task-stats-row">
                  <span>已匹配: {{ run.stats.matched }}</span>
                  <span>抓取图片: {{ run.stats.images }}</span>
                  <span>待确认: {{ run.stats.review }}</span>
                  <span>跳过: {{ run.stats.skipped }}</span>
                  <span v-if="run.stats.failed > 0" class="stat-failed">失败: {{ run.stats.failed }}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- TMDB 凭据与网络加速设置卡片 -->
          <div class="entries-card highlight-card">
            <div class="entries-card-head">
              <div class="card-head-left">
                <span class="group-icon">🍿</span>
                <div>
                  <div class="group-title">TMDB 凭据与网络加速配置</div>
                  <div class="group-sub">配置 The Movie Database (TMDB) 凭据与代理镜像，配置后刮削即时生效</div>
                </div>
              </div>
            </div>

            <div class="entries-card-body">
              <div class="tmdb-guide-banner">
                <div class="tmdb-guide-content">
                  <div class="tmdb-guide-title">
                    <strong>The Movie Database (TMDB) 接入指引</strong>
                    <span class="tmdb-badge">v3 / v4 官方支持</span>
                  </div>
                  <p class="tmdb-guide-text">
                    WhatMedia 使用 TMDB 获取影视中文标题、分季分集剧照、演职员表及剧情简介。
                    个人账号可在 TMDB 官网免费申请 API 密钥。若国内网络访问受限，可在下方配置反代加速镜像。
                  </p>
                </div>
                <a
                  href="https://www.themoviedb.org/settings/api"
                  target="_blank"
                  rel="noopener noreferrer"
                  class="tmdb-apply-btn"
                >
                  申请 TMDB Key ↗
                </a>
              </div>

              <SettingControl
                v-for="e in metadataEntries"
                :key="e.key"
                :entry="e"
                @save="(v) => saveSetting(e, v)"
                @reset="resetSetting(e)"
              />
            </div>
          </div>
        </section>

        <!-- 5. 电视直播订阅源配置 -->
        <section
          v-if="activeTab === 'all' || activeTab === 'live'"
          class="settings-section"
          data-test="live-settings"
        >
          <div class="section-title">电视直播 (Live TV)</div>
          <p class="section-desc">
            配置电视直播订阅源。支持通用的 M3U / M3U8 播放列表以及包含 <code>#genre#</code> 分类的 TXT 频道列表，自动解析频道并在电视大屏沉浸播放。
          </p>

          <!-- 错误或警告提示 -->
          <p v-if="liveError" class="error-box">
            加载直播订阅源失败：{{ liveError }}
          </p>

          <!-- 添加直播源控制台 -->
          <div class="web-scan-card live-console-card">
            <div class="web-scan-head">
              <div>
                <h3 class="web-scan-title">添加直播订阅源</h3>
                <p class="web-scan-sub">
                  支持在线 HTTP/HTTPS 链接。添加后系统将自动拉取并解析频道列表，支持多线路聚合与故障自动秒切。
                </p>
              </div>
              <button
                type="button"
                class="refresh-stats-btn"
                title="刷新已配置的直播源"
                :disabled="loadingLiveSources"
                @click="loadLiveSources"
              >
                🔄 刷新列表
              </button>
            </div>

            <form class="web-scan-form" @submit.prevent="addLiveSource">
              <div class="scan-input-group">
                <label class="scan-label" for="live-source-name">订阅源名称</label>
                <input
                  id="live-source-name"
                  v-model="newSourceName"
                  type="text"
                  class="scan-input"
                  placeholder="例如：家庭电视源 / 央视卫视精选"
                  :disabled="liveBusy"
                />
              </div>

              <div class="scan-input-group">
                <label class="scan-label" for="live-source-url">播放列表 URL (M3U / TXT)</label>
                <input
                  id="live-source-url"
                  v-model="newSourceUrl"
                  type="url"
                  class="scan-input font-mono"
                  placeholder="https://example.com/playlist.m3u 或 result.txt"
                  :disabled="liveBusy"
                />
              </div>

              <div class="scan-input-group">
                <label class="scan-label" for="live-source-epg">EPG 节目单地址 (可选 XMLTV)</label>
                <input
                  id="live-source-epg"
                  v-model="newSourceEpg"
                  type="url"
                  class="scan-input font-mono"
                  placeholder="https://example.com/epg.xml (选填)"
                  :disabled="liveBusy"
                />
              </div>

              <div class="scan-options-row">
                <div class="scan-btn-group">
                  <button
                    type="submit"
                    class="btn-start-scan"
                    :disabled="liveBusy || !newSourceName.trim() || !newSourceUrl.trim()"
                  >
                    <span>{{ liveBusy ? '正在处理...' : '➕ 添加并同步订阅源' }}</span>
                  </button>
                </div>
              </div>

              <div
                v-if="liveNotice"
                class="scan-feedback"
                :class="liveNotice.includes('失败') ? 'err' : 'ok'"
              >
                <span>{{ liveNotice.includes('失败') ? '⚠️' : '✓' }}</span>
                <span>{{ liveNotice }}</span>
              </div>
            </form>
          </div>

          <!-- 已配置的直播源列表 -->
          <div class="live-sources-block">
            <h4 class="live-block-title">已配置的订阅源 ({{ liveSources.length }})</h4>

            <div v-if="loadingLiveSources" class="live-loading-state">
              正在加载直播订阅源...
            </div>

            <div v-else-if="!liveSources.length" class="empty-live-box">
              <span class="empty-live-ico">📡</span>
              <span>暂未配置任何直播订阅源。请在上方输入播放列表 URL 添加订阅。</span>
            </div>

            <div v-else class="live-sources-grid">
              <div
                v-for="s in liveSources"
                :key="s.id"
                class="live-source-card"
                data-test="live-source-item"
              >
                <div class="live-source-card-main">
                  <div class="live-source-card-top">
                    <span class="live-source-name">{{ s.name }}</span>
                    <span class="live-source-badge">{{ s.kind }}</span>
                  </div>
                  <div class="live-source-url font-mono" :title="s.url ?? undefined">{{ s.url }}</div>
                  <div v-if="s.epg_url" class="live-source-epg font-mono" :title="s.epg_url ?? undefined">
                    <span class="epg-tag">EPG</span> {{ s.epg_url }}
                  </div>
                  <div class="live-source-meta">
                    <span class="live-source-channels">📺 {{ s.channel_count }} 个频道</span>
                    <span class="live-source-synced">
                      最后同步: {{ s.last_sync_at ? new Date(s.last_sync_at).toLocaleString() : '未同步' }}
                    </span>
                  </div>
                  <div v-if="s.sync_error" class="live-source-err">
                    ⚠️ {{ s.sync_error }}
                  </div>
                </div>

                <div class="live-source-actions">
                  <button
                    type="button"
                    class="btn-live-sync"
                    :disabled="syncingSourceId === s.id || liveBusy"
                    @click="syncSingleLiveSource(s.id)"
                  >
                    {{ syncingSourceId === s.id ? '同步中...' : '🔄 立即同步' }}
                  </button>
                  <button
                    type="button"
                    class="btn-live-delete"
                    :disabled="liveBusy"
                    @click="deleteSingleLiveSource(s.id, s.name)"
                  >
                    🗑️ 删除
                  </button>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- 6. 可写设置 -->
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

          <!-- 分组 1：播放与界面偏好（用户级偏好，默认展开） -->
          <div v-if="userEntries.length" class="entries-card">
            <div class="entries-card-head" @click="toggleGroup('user')">
              <div class="card-head-left">
                <span class="group-icon">🎬</span>
                <div>
                  <div class="group-title">播放与界面偏好</div>
                  <div class="group-sub">只影响您当前账户的播放控制与视图偏好，即时生效</div>
                </div>
              </div>
              <span class="fold-arrow">{{ collapsedGroups.user ? '▼ 展开' : '▲ 收起' }}</span>
            </div>
            <div v-show="!collapsedGroups.user" class="entries-card-body">
              <SettingControl
                v-for="e in userEntries"
                :key="e.key"
                :entry="e"
                @save="(v) => saveSetting(e, v)"
                @reset="resetSetting(e)"
              />
            </div>
          </div>

          <!-- 分组 2：元数据与 TMDB 刮削凭据（全服级，核心高频，默认展开） -->
          <div v-if="metadataEntries.length" class="entries-card highlight-card">
            <div class="entries-card-head" @click="toggleGroup('metadata')">
              <div class="card-head-left">
                <span class="group-icon">🍿</span>
                <div>
                  <div class="group-title">元数据与 TMDB 刮削凭据</div>
                  <div class="group-sub">配置 The Movie Database (TMDB) 凭据与网络加速，用于精准识别剧集与海报刮削</div>
                </div>
              </div>
              <span class="fold-arrow">{{ collapsedGroups.metadata ? '▼ 展开' : '▲ 收起' }}</span>
            </div>

            <div v-show="!collapsedGroups.metadata" class="entries-card-body">
              <!-- TMDB 引导与申请指引卡片 -->
              <div class="tmdb-guide-banner">
                <div class="tmdb-guide-content">
                  <div class="tmdb-guide-title">
                    <strong>The Movie Database (TMDB) 接入指引</strong>
                    <span class="tmdb-badge">v3 / v4 官方支持</span>
                  </div>
                  <p class="tmdb-guide-text">
                    WhatMedia 使用 TMDB 获取影视中文标题、分季分集剧照、演职员表及剧情简介。
                    个人账号可在 TMDB 官网免费申请 API 密钥。若国内网络访问受限，可在下方配置反代加速镜像。
                  </p>
                </div>
                <a
                  href="https://www.themoviedb.org/settings/api"
                  target="_blank"
                  rel="noopener noreferrer"
                  class="tmdb-apply-btn"
                >
                  申请 TMDB Key ↗
                </a>
              </div>

              <SettingControl
                v-for="e in metadataEntries"
                :key="e.key"
                :entry="e"
                @save="(v) => saveSetting(e, v)"
                @reset="resetSetting(e)"
              />
            </div>
          </div>

          <!-- 分组 3：转码参数与存储缓存运维（全服级，默认折叠） -->
          <div v-if="transcodeCacheEntries.length" class="entries-card">
            <div class="entries-card-head" @click="toggleGroup('transcodeCache')">
              <div class="card-head-left">
                <span class="group-icon">⚡</span>
                <div>
                  <div class="group-title">转码参数与存储缓存运维</div>
                  <div class="group-sub">FFmpeg 转码并发上限、HLS 分片时长与图片缓存容量策略</div>
                </div>
              </div>
              <span class="fold-arrow">{{ collapsedGroups.transcodeCache ? '▼ 展开' : '▲ 收起' }}</span>
            </div>
            <div v-show="!collapsedGroups.transcodeCache" class="entries-card-body">
              <SettingControl
                v-for="e in transcodeCacheEntries"
                :key="e.key"
                :entry="e"
                @save="(v) => saveSetting(e, v)"
                @reset="resetSetting(e)"
              />
            </div>
          </div>

          <!-- 分组 4：高级系统与 AI 全服参数（全服级，默认折叠） -->
          <div v-if="otherServerEntries.length" class="entries-card">
            <div class="entries-card-head" @click="toggleGroup('otherServer')">
              <div class="card-head-left">
                <span class="group-icon">⚙</span>
                <div>
                  <div class="group-title">高级运维与全局参数</div>
                  <div class="group-sub">AI 助手降级策略与其他服务端全局白名单参数</div>
                </div>
              </div>
              <span class="fold-arrow">{{ collapsedGroups.otherServer ? '▼ 展开' : '▲ 收起' }}</span>
            </div>
            <div v-show="!collapsedGroups.otherServer" class="entries-card-body">
              <SettingControl
                v-for="e in otherServerEntries"
                :key="e.key"
                :entry="e"
                @save="(v) => saveSetting(e, v)"
                @reset="resetSetting(e)"
              />
            </div>
          </div>
        </section>

        <!-- 5. AI 助手 -->
        <section
          v-if="activeTab === 'all' || activeTab === 'ai'"
          class="settings-section"
        >
          <div class="section-title">AI 助手</div>
          <p class="section-desc">
            全站内嵌智能媒体助理，支持自然语言检索影视、对话解答、识别异常条目与辅助管理。
          </p>

          <p v-if="aiError" class="warn-box">
            {{ aiError }}
          </p>

          <div class="ai-stat-grid">
            <div class="ai-stat-card">
              <strong :style="{ color: aiError ? 'var(--muted)' : 'var(--good)' }">
                {{ aiError ? '未装配' : '就绪' }}
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

          <!-- 模型服务商快捷预设卡片选择器 -->
          <div class="ai-presets-wrap">
            <div class="ai-presets-title">选择大模型服务商 (Provider 预设)</div>
            <div class="ai-presets-grid">
              <div
                v-for="p in providerPresets"
                :key="p.id"
                class="ai-preset-card"
                :class="{ active: currentProvider === p.id }"
                @click="applyProviderPreset(p)"
              >
                <div class="preset-head">
                  <span class="preset-icon">{{ p.icon }}</span>
                  <span v-if="p.badge" class="preset-badge">{{ p.badge }}</span>
                </div>
                <div class="preset-name">{{ p.name }}</div>
                <div class="preset-desc">{{ p.desc }}</div>
                <div class="preset-footer">
                  <span v-if="currentProvider === p.id" class="preset-selected-tag">✓ 当前使用</span>
                  <span v-else class="preset-select-btn">选用预设</span>
                </div>
              </div>
            </div>
          </div>

          <!-- AI 模型核心参数配置卡片 -->
          <div v-if="aiEntries.length" class="entries-card highlight-card">
            <div class="entries-card-head">
              <div class="card-head-left">
                <span class="group-icon">🤖</span>
                <div>
                  <div class="group-title">AI 模型连接与运行参数</div>
                  <div class="group-sub">当前服务商：<strong>{{ currentPresetName }}</strong>（模型：{{ currentModelName || '默认' }}）</div>
                </div>
              </div>
              <button
                type="button"
                class="btn-test-ai"
                :disabled="testingAi"
                title="向当前模型发送一条问候测试请求以验证可用性"
                @click="testAiConnection"
              >
                {{ testingAi ? '正在测试…' : '⚡ 测试连通性' }}
              </button>
            </div>

            <div class="entries-card-body">
              <div
                v-if="testResult"
                class="ai-test-feedback"
                :class="{ ok: testResult.ok, err: !testResult.ok }"
              >
                <span>{{ testResult.ok ? '✓' : '⚠️' }}</span>
                <span>{{ testResult.message }}</span>
              </div>

              <SettingControl
                v-for="e in aiEntries"
                :key="e.key"
                :entry="e"
                @save="(v) => saveSetting(e, v)"
                @reset="resetSetting(e)"
              />
            </div>
          </div>

          <p class="section-desc" style="margin-top: 14px;">
            没配模型时 AI 不会失效，而是退回内置规则解析 —— 「找出没有海报的电影」
            这类问法照样答得出来，回答上会标“离线模式”。
            会改动媒体库的操作一律先给预览、再要你确认，没有例外。
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

    <!-- 服务端文件系统物理路径选择弹窗 -->
    <DirectoryPickerModal
      v-model="showDirPicker"
      :initial-path="scanForm.path"
      @select="onDirSelected"
    />
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

/* ─────────────────────────────────────────────────────────────
 * 可写设置卡片与分组折叠
 * ───────────────────────────────────────────────────────────── */
.entries-card {
  margin-top: 14px;
  background: var(--panel2);
  border: 1px solid var(--line);
  border-radius: var(--radius);
  overflow: hidden;
  box-shadow: 0 4px 18px rgba(0, 0, 0, 0.18);
  transition: border-color 0.2s ease, box-shadow 0.2s ease;
}

.entries-card.highlight-card {
  border-color: rgba(87, 200, 245, 0.35);
  box-shadow: 0 4px 22px rgba(87, 200, 245, 0.08);
}

.entries-card-head {
  padding: 14px 18px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  cursor: pointer;
  user-select: none;
  background: rgba(255, 255, 255, 0.02);
  transition: background 0.15s ease;
}

.entries-card-head:hover {
  background: rgba(255, 255, 255, 0.05);
}

.card-head-left {
  display: flex;
  align-items: center;
  gap: 12px;
}

.group-icon {
  font-size: 20px;
}

.group-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  display: flex;
  align-items: center;
  gap: 8px;
}

.group-sub {
  font-size: 12px;
  color: var(--muted);
  margin-top: 2px;
}

.fold-arrow {
  font-size: 12px;
  color: var(--cyan);
  font-weight: 500;
  padding: 4px 10px;
  border-radius: 6px;
  background: rgba(87, 200, 245, 0.08);
  border: 1px solid rgba(87, 200, 245, 0.2);
  flex-shrink: 0;
}

.entries-card-body {
  padding: 6px 18px 14px;
  border-top: 1px solid var(--line);
}

/* TMDB 引导条 */
.tmdb-guide-banner {
  margin: 12px 0 16px;
  padding: 12px 16px;
  background: linear-gradient(135deg, rgba(13, 37, 63, 0.8), rgba(1, 180, 228, 0.12));
  border: 1px solid rgba(1, 180, 228, 0.35);
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.tmdb-guide-content {
  flex: 1;
}

.tmdb-guide-title {
  display: flex;
  align-items: center;
  gap: 8px;
  color: #fff;
  font-size: 13px;
}

.tmdb-badge {
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 4px;
  background: #01b4e4;
  color: #0d253f;
  font-weight: bold;
  letter-spacing: 0.5px;
}

.tmdb-guide-text {
  margin: 4px 0 0;
  font-size: 12px;
  color: rgba(255, 255, 255, 0.75);
  line-height: 1.5;
}

.tmdb-apply-btn {
  flex-shrink: 0;
  padding: 6px 14px;
  background: #01b4e4;
  color: #0d253f;
  font-size: 12px;
  font-weight: 600;
  border-radius: 6px;
  text-decoration: none;
  transition: all 0.15s ease;
}

.tmdb-apply-btn:hover {
  background: #90cea1;
  box-shadow: 0 2px 10px rgba(1, 180, 228, 0.4);
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

/* ─────────────────────────────────────────────────────────────
 * Web 端直接添加与扫盘入库
 * ───────────────────────────────────────────────────────────── */
.web-scan-card {
  margin: 14px 0;
  padding: 18px 20px;
  background: var(--panel2);
  border: 1px solid var(--line);
  border-radius: var(--radius);
  box-shadow: 0 4px 20px rgba(0, 0, 0, 0.2);
}

.web-scan-title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
}

.web-scan-sub {
  margin: 4px 0 14px;
  font-size: 12.5px;
  color: var(--muted);
  line-height: 1.5;
}

.web-scan-form {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.scan-input-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.scan-label {
  font-size: 12px;
  font-weight: 500;
  color: var(--muted);
  display: flex;
  align-items: center;
  gap: 4px;
}

.required-star {
  color: #f87171;
}

.scan-input {
  width: 100%;
  padding: 9px 12px;
  background: var(--bg);
  border: 1px solid var(--line);
  border-radius: 8px;
  color: var(--text);
  font-size: 13px;
  transition: border-color 0.15s ease;
}

.scan-input:focus {
  border-color: var(--cyan);
}

.path-input-wrap {
  position: relative;
  display: flex;
  align-items: center;
}

.path-input-wrap .path-input {
  padding-right: 44px;
}

.dir-picker-icon-btn {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--panel3);
  border: 1px solid var(--line);
  border-radius: 6px;
  font-size: 16px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.dir-picker-icon-btn:hover {
  background: var(--panel);
  border-color: var(--cyan);
  box-shadow: 0 0 8px rgba(87, 200, 245, 0.3);
}

.scan-options-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-top: 4px;
}

.scan-checkbox-label {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--muted);
  cursor: pointer;
}

.scan-checkbox-label input[type="checkbox"] {
  accent-color: var(--cyan);
  width: 16px;
  height: 16px;
  cursor: pointer;
}

.scan-btn-group {
  display: flex;
  align-items: center;
  gap: 10px;
}

.btn-save-lib {
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(174, 193, 226, 0.2);
  border-radius: 8px;
  color: var(--text);
  font-weight: 500;
  font-size: 13px;
  transition: all 0.2s ease;
  cursor: pointer;
}

.btn-save-lib:hover:not(:disabled) {
  background: rgba(87, 200, 245, 0.15);
  border-color: var(--cyan);
  color: #fff;
  box-shadow: 0 3px 12px rgba(87, 200, 245, 0.25);
}

.btn-save-lib:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.btn-start-scan {
  padding: 8px 18px;
  background: linear-gradient(135deg, var(--purple), #5334db);
  border: none;
  border-radius: 8px;
  color: #fff;
  font-weight: 500;
  font-size: 13px;
  box-shadow: 0 3px 12px rgba(121, 91, 255, 0.3);
  transition: all 0.15s ease;
  cursor: pointer;
}

.btn-start-scan:hover:not(:disabled) {
  background: linear-gradient(135deg, var(--purple2), var(--purple));
  box-shadow: 0 4px 16px rgba(121, 91, 255, 0.45);
}

.btn-start-scan:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  box-shadow: none;
}

.scan-feedback {
  padding: 8px 12px;
  border-radius: 8px;
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.scan-feedback.ok {
  background: rgba(85, 214, 166, 0.1);
  border: 1px solid rgba(85, 214, 166, 0.3);
  color: var(--good);
}

.scan-feedback.err {
  background: rgba(248, 113, 113, 0.1);
  border: 1px solid rgba(248, 113, 113, 0.3);
  color: #f87171;
}

.storage-title-wrap {
  display: flex;
  align-items: center;
  gap: 10px;
}

.rescan-btn {
  padding: 3px 8px;
  font-size: 11px;
  background: var(--panel3);
  border: 1px solid var(--line);
  color: var(--cyan);
  border-radius: 6px;
  transition: all 0.12s ease;
}

.rescan-btn:hover:not(:disabled) {
  background: rgba(87, 200, 245, 0.12);
  border-color: var(--cyan);
}

.rescan-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 在途扫描任务 */
.active-tasks-list {
  margin-top: 14px;
  padding-top: 14px;
  border-top: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.active-tasks-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.task-card {
  padding: 10px 14px;
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 8px;
}

.task-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 6px;
  font-size: 12.5px;
}

.task-lib-name {
  display: flex;
  align-items: center;
  gap: 6px;
}

.task-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--cyan);
  animation: pulse 1.2s infinite;
}

.task-dot.finished {
  background: var(--good);
  animation: none;
}

@keyframes pulse {
  0% { transform: scale(0.9); opacity: 0.6; }
  50% { transform: scale(1.15); opacity: 1; }
  100% { transform: scale(0.9); opacity: 0.6; }
}

.task-status-text {
  font-size: 12px;
  color: var(--muted);
}

.task-progress-bar {
  height: 6px;
  background: var(--panel3);
  border-radius: 3px;
  overflow: hidden;
  margin: 6px 0;
}

.task-progress-inner {
  display: block;
  height: 100%;
  background: linear-gradient(90deg, var(--cyan), var(--purple));
  border-radius: 3px;
  transition: width 0.3s ease;
}

.task-stats-row {
  display: flex;
  gap: 12px;
  font-size: 11.5px;
  color: var(--muted);
}

.stat-failed {
  color: #f87171;
}

/* ─────────────────────────────────────────────────────────────
 * AI 服务商预设与模型配置
 * ───────────────────────────────────────────────────────────── */
.ai-presets-wrap {
  margin-top: 18px;
}

.ai-presets-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
  margin-bottom: 10px;
}

.ai-presets-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
  gap: 12px;
}

.ai-preset-card {
  padding: 14px 16px;
  background: var(--panel2);
  border: 1px solid var(--line);
  border-radius: var(--radius);
  cursor: pointer;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  transition: all 0.2s ease;
  position: relative;
}

.ai-preset-card:hover {
  background: var(--panel3);
  border-color: rgba(87, 200, 245, 0.4);
  transform: translateY(-2px);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
}

.ai-preset-card.active {
  border-color: var(--cyan);
  background: rgba(87, 200, 245, 0.08);
  box-shadow: 0 0 16px rgba(87, 200, 245, 0.2);
}

.preset-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}

.preset-icon {
  font-size: 22px;
}

.preset-badge {
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 4px;
  background: rgba(87, 200, 245, 0.15);
  color: var(--cyan);
  border: 1px solid rgba(87, 200, 245, 0.3);
  font-weight: 500;
}

.preset-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  margin-bottom: 4px;
}

.preset-desc {
  font-size: 12px;
  color: var(--muted);
  line-height: 1.45;
  margin-bottom: 12px;
  flex: 1;
}

.preset-footer {
  display: flex;
  justify-content: flex-end;
}

.preset-selected-tag {
  font-size: 11px;
  font-weight: 600;
  color: var(--cyan);
}

.preset-select-btn {
  font-size: 11px;
  color: var(--muted);
  padding: 2px 8px;
  border-radius: 4px;
  border: 1px solid var(--line);
  background: var(--panel);
}

.ai-preset-card:hover .preset-select-btn {
  color: var(--text);
  border-color: var(--cyan);
}

.btn-test-ai {
  padding: 5px 12px;
  background: linear-gradient(135deg, rgba(87, 200, 245, 0.2), rgba(121, 91, 255, 0.25));
  border: 1px solid var(--cyan);
  border-radius: 8px;
  color: #fff;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.btn-test-ai:hover:not(:disabled) {
  background: linear-gradient(135deg, rgba(87, 200, 245, 0.35), rgba(121, 91, 255, 0.45));
  box-shadow: 0 2px 12px rgba(87, 200, 245, 0.3);
}

.btn-test-ai:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.ai-test-feedback {
  margin: 10px 0 14px;
  padding: 10px 14px;
  border-radius: 8px;
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 8px;
  line-height: 1.4;
}

.ai-test-feedback.ok {
  background: rgba(85, 214, 166, 0.12);
  border: 1px solid rgba(85, 214, 166, 0.35);
  color: var(--good);
}

.ai-test-feedback.err {
  background: rgba(248, 113, 113, 0.12);
  border: 1px solid rgba(248, 113, 113, 0.35);
  color: #f87171;
}

/* ─────────────────────────────────────────────────────────────
 * 手动刮削控制台与看板样式
 * ───────────────────────────────────────────────────────────── */
.scrape-console-card {
  margin: 14px 0 18px;
}

.refresh-stats-btn {
  padding: 5px 12px;
  background: var(--panel3);
  border: 1px solid var(--line);
  border-radius: 7px;
  color: var(--text);
  font-size: 11.5px;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.refresh-stats-btn:hover {
  background: var(--line);
  border-color: var(--cyan);
}

.scan-select {
  cursor: pointer;
}

.scrape-metrics-grid {
  display: grid;
  grid-template-columns: repeat(6, 1fr);
  gap: 10px;
  margin-top: 14px;
}

.scrape-metric-item {
  background: var(--bg);
  border: 1px solid var(--line);
  border-radius: 9px;
  padding: 10px 8px;
  text-align: center;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.metric-num {
  font-size: 17px;
  font-weight: 700;
  font-family: var(--mono);
  color: var(--text);
}

.metric-lbl {
  font-size: 11px;
  color: var(--muted);
}

.text-good {
  color: var(--good);
}

.text-cyan {
  color: var(--cyan);
}

.text-warn {
  color: #f59e0b;
}

.text-danger {
  color: #f87171;
}

.text-muted {
  color: var(--muted);
}

@media (max-width: 768px) {
  .scrape-metrics-grid {
    grid-template-columns: repeat(3, 1fr);
  }
}

/* ── 电视直播源配置 ── */
.live-console-card {
  margin-top: 14px;
}

.live-sources-block {
  margin-top: 24px;
}

.live-block-title {
  margin: 0 0 12px;
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.empty-live-box {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 16px 20px;
  border-radius: 10px;
  background: var(--surface);
  border: 1px dashed var(--line);
  color: var(--muted);
  font-size: 13px;
}

.empty-live-ico {
  font-size: 20px;
}

.live-loading-state {
  padding: 16px;
  color: var(--muted);
  font-size: 13px;
}

.live-sources-grid {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.live-source-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 20px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: 10px;
  transition: border-color 0.15s ease;
}

.live-source-card:hover {
  border-color: rgba(255, 255, 255, 0.2);
}

.live-source-card-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.live-source-card-top {
  display: flex;
  align-items: center;
  gap: 8px;
}

.live-source-name {
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
}

.live-source-badge {
  display: inline-block;
  padding: 2px 7px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  background: rgba(87, 200, 245, 0.15);
  color: var(--cyan);
  border: 1px solid rgba(87, 200, 245, 0.3);
}

.live-source-url,
.live-source-epg {
  font-size: 12px;
  color: var(--muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.epg-tag {
  display: inline-block;
  font-size: 10px;
  padding: 1px 4px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.08);
  color: var(--muted);
  margin-right: 4px;
}

.live-source-meta {
  display: flex;
  align-items: center;
  gap: 16px;
  font-size: 12px;
  color: var(--muted);
}

.live-source-channels {
  color: var(--text);
  font-weight: 500;
}

.live-source-err {
  font-size: 12px;
  color: #f87171;
  background: rgba(248, 113, 113, 0.1);
  padding: 4px 8px;
  border-radius: 6px;
  display: inline-block;
}

.live-source-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.btn-live-sync {
  padding: 6px 14px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid var(--line);
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.btn-live-sync:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.15);
  border-color: rgba(255, 255, 255, 0.3);
}

.btn-live-delete {
  padding: 6px 14px;
  border-radius: 8px;
  background: rgba(248, 113, 113, 0.1);
  border: 1px solid rgba(248, 113, 113, 0.25);
  color: #f87171;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.btn-live-delete:hover:not(:disabled) {
  background: rgba(248, 113, 113, 0.2);
}

.btn-live-sync:disabled,
.btn-live-delete:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

@media (max-width: 768px) {
  .live-source-card {
    flex-direction: column;
    align-items: flex-start;
  }
  .live-source-actions {
    width: 100%;
    justify-content: flex-end;
  }
}
</style>

