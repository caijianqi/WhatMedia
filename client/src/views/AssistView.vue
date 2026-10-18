<script setup lang="ts">
/**
 * AI 助手页（P7）。
 *
 * 页面刻意做薄：所有权限语义都在 `ai/assistant.ts` 里，这里只负责把它们
 * 变成看得见的样子。**模板里不写"能不能执行"的判断** —— 那种判断一旦
 * 散进模板，迟早会在某个分支里漏掉，而漏掉的后果是用户在没看清预览的
 * 情况下把库扫了一遍。
 *
 * 三个视觉取舍：
 *
 *   · 待确认工单用整块高亮，而不是 toast。toast 会消失，工单不该消失。
 *   · 确认按钮只在确实有待确认工单时才**存在**，不是置灰。
 *     置灰的按钮会被理解成"暂时不能用"，用户会去找别的路径绕过。
 *   · 降级回答标"离线模式"。让人误以为模型真听懂了他的话，
 *     比承认"这次是规则匹配的"糟糕得多。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { AssistantSession, httpTransport, type AssistantState, type PendingAction } from '../ai/assistant'
import * as api from '../api/client'

/**
 * 操作者。真实部署里来自登录态。
 *
 * 这里给一个显式切换，是为了让"儿童 / 访客不能触发写操作"这条约束
 * **能被手点验证** —— 只看服务端返回 403，看不出界面是不是盖对了。
 */
const ROLES: api.AiRole[] = ['adult', 'child', 'guest']
const role = ref<api.AiRole>('adult')
const userId = 'local-user'

const session = new AssistantSession(httpTransport, { id: userId, role: role.value })
const state = ref<AssistantState>(session.snapshot)
const input = ref('')
const tools = ref<api.ToolSpecView[]>([])
/** AI 不可用（未启用 / 数据库没就绪）时给一句能行动的提示。 */
const unavailable = ref<string | null>(null)

let stop: (() => void) | null = null

const busy = computed(
  () => state.value.phase === 'asking' || state.value.phase === 'confirming',
)
/** 只有这个为真时确认按钮才会存在。 */
const canConfirm = computed(() => state.value.phase === 'awaitingConfirmation')

/**
 * 最后一轮。
 *
 * 刻意不写 `turns.at(-1)`：构建目标里有 safari13（电视 / 非 Windows 桌面壳），
 * `Array.prototype.at` 要 Safari 15.4 才有，esbuild 不会替我们降级内建方法。
 */
const lastTurn = computed(() => {
  const turns = state.value.turns
  return turns.length > 0 ? turns[turns.length - 1]! : null
})

/** 顶部那条琥珀色的工单条，始终展示最后一轮的工单。 */
const ticket = computed<PendingAction | null>(() => lastTurn.value?.pending ?? null)

function switchRole(next: api.AiRole): void {
  role.value = next
  session.setActor({ id: userId, role: next })
}

onMounted(() => {
  stop = session.subscribe((s) => (state.value = s))
  // 单独取工具清单：AI 关掉时它返回 503，但不该让整个页面打不开。
  api
    .listAiTools()
    .then((t) => (tools.value = t))
    .catch((e) => {
      unavailable.value = api.isAiDisabled(e)
        ? 'AI 助手未启用（服务端配置 [ai] 段的 enabled = false），或数据库还没就绪。'
        : e instanceof Error
          ? e.message
          : String(e)
    })
})

onUnmounted(() => stop?.())

async function send(): Promise<void> {
  const text = input.value.trim()
  if (!text || busy.value) return
  input.value = ''
  await session.ask(text)
}

async function confirm(): Promise<void> {
  if (!canConfirm.value) return
  await session.confirm()
}

const ROLE_LABEL: Record<api.AiRole, string> = {
  owner: '主人',
  administrator: '管理员',
  adult: '成人',
  child: '儿童',
  guest: '访客',
}

/** 提问示例。点一下就填进输入框 —— 比让人对着空框想"我该问什么"有用。 */
const EXAMPLES = [
  '找出没有海报的电影',
  '最近添加的剧集',
  '两小时以内的科幻片',
  '有哪些重复的',
]

function statusText(s: string): string {
  switch (s) {
    case 'confirmed':
      return '已执行'
    case 'rejected':
      return '已取消'
    case 'failed':
      return '执行失败'
    default:
      return '待确认'
  }
}

/** 工单处理完之后不再强调，但仍要看得见"刚刚发生了什么"。 */
const ticketSettled = computed(() => {
  const s = lastTurn.value?.status
  return s === 'confirmed' || s === 'rejected' || s === 'failed'
})

const riskLabel: Record<string, string> = {
  read_only: '只读',
  write: '写入',
  destructive: '危险',
}
</script>

<template>
  <div class="space-y-6">
    <header class="flex flex-wrap items-end justify-between gap-4">
      <div>
        <p class="text-xs font-medium uppercase tracking-[0.2em] text-brand-soft">AI</p>
        <h1 class="text-gradient mt-1.5 text-2xl font-semibold tracking-tight sm:text-3xl">
          媒体管家
        </h1>
        <p class="mt-2 text-sm text-wm-muted">
          用大白话问你的媒体库。会改动库的操作一律先给预览、再要你确认。
        </p>
      </div>

      <div class="flex items-center gap-2 rounded-lg border border-ink-800 bg-ink-900/60 px-2.5 py-1.5">
        <span class="text-xs text-wm-muted">当前角色</span>
        <button
          v-for="r in ROLES"
          :key="r"
          class="focus-ring rounded-md px-2 py-1 text-xs transition duration-200"
          :class="
            role === r
              ? 'bg-brand/20 font-medium text-brand-soft'
              : 'text-wm-muted hover:bg-ink-850 hover:text-wm-sub'
          "
          @click="switchRole(r)"
        >
          {{ ROLE_LABEL[r] }}
        </button>
      </div>
    </header>

    <p
      v-if="unavailable"
      class="rounded-lg border border-amber-900/60 bg-amber-950/40 px-3 py-2 text-sm text-amber-300"
    >
      {{ unavailable }}
    </p>

    <!-- 待确认工单：唯一的执行入口 -->
    <section
      v-if="ticket"
      class="relative overflow-hidden rounded-card border p-4 transition duration-300"
      :class="
        ticketSettled
          ? 'border-wm-line-strong bg-ink-900/60'
          : 'border-amber-700/60 bg-amber-950/25 shadow-glow'
      "
    >
      <!-- 未处理时在左侧留一条强调条，处理完就撤掉 -->
      <span
        v-if="!ticketSettled"
        class="absolute inset-y-0 left-0 w-1"
        style="background-image: linear-gradient(180deg, #f59e0b, #f97316)"
      />

      <div class="flex items-start gap-3">
        <span
          class="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg"
          :class="ticketSettled ? 'bg-ink-800 text-wm-muted' : 'bg-amber-950/80 text-amber-300'"
        >
          <svg
            viewBox="0 0 24 24"
            class="h-4 w-4"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M12 9v4M12 17h.01M10.3 3.9 2.4 18a1.8 1.8 0 0 0 1.6 2.7h15.9a1.8 1.8 0 0 0 1.6-2.7L13.7 3.9a1.8 1.8 0 0 0-3.4 0z" />
          </svg>
        </span>

        <div class="min-w-0 flex-1">
          <p
            class="text-sm font-medium"
            :class="ticketSettled ? 'text-wm-sub' : 'text-amber-200'"
          >
            {{ ticket.summary }}
          </p>
          <ul v-if="ticket.lines.length" class="mt-2 space-y-1 text-xs text-wm-muted">
            <li v-for="(line, n) in ticket.lines" :key="n">· {{ line }}</li>
          </ul>
          <p
            v-for="(w, n) in ticket.warnings"
            :key="`w-${n}`"
            class="mt-1 text-xs"
            :class="ticketSettled ? 'text-wm-muted' : 'text-amber-400'"
          >
            ⚠ {{ w }}
          </p>

          <div v-if="canConfirm" class="mt-3.5 flex flex-wrap items-center gap-2">
            <button
              data-test="confirm"
              class="btn !bg-amber-600 !py-1.5 text-white shadow-none hover:!bg-amber-500"
              :disabled="busy"
              @click="confirm"
            >
              {{ busy ? '执行中…' : '确认执行' }}
            </button>
            <button
              data-test="reject"
              class="btn-ghost !py-1.5"
              :disabled="busy"
              @click="session.reject()"
            >
              取消
            </button>
            <span class="text-xs text-wm-muted">{{ ticket.secondsLeft }} 秒内有效</span>
          </div>
          <p v-else class="mt-3 text-xs text-wm-muted">
            {{ statusText(lastTurn?.status ?? '') }}
          </p>
        </div>
      </div>
    </section>

    <!-- 会话 -->
    <section class="space-y-4">
      <!-- 空态：给几个能直接点的例子，比"还没问过"有用 -->
      <div v-if="state.turns.length === 0 && !busy" class="empty-state">
        <span
          class="flex h-10 w-10 items-center justify-center rounded-card"
          style="background-image: linear-gradient(135deg, rgba(91,140,255,0.25), rgba(167,139,250,0.25))"
        >
          <svg
            viewBox="0 0 24 24"
            class="h-5 w-5 text-brand-soft"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M12 3.5l1.9 4.6 4.6 1.9-4.6 1.9L12 16.5l-1.9-4.6L5.5 10l4.6-1.9z" />
          </svg>
        </span>
        <p class="text-sm text-wm-muted">还没问过任何问题</p>
        <div class="mt-1 flex flex-wrap justify-center gap-2">
          <button
            v-for="e in EXAMPLES"
            :key="e"
            class="focus-ring rounded-lg border border-wm-line-strong bg-ink-850/70 px-2.5 py-1 text-xs text-wm-sub transition hover:border-brand/50 hover:text-white"
            @click="input = e"
          >
            {{ e }}
          </button>
        </div>
      </div>

      <article v-for="(t, i) in state.turns" :key="i" class="space-y-2.5">
        <!-- 提问：右对齐的气泡 -->
        <div class="flex justify-end">
          <p
            class="max-w-[80%] rounded-2xl rounded-br-md px-3.5 py-2 text-sm text-wm-text"
            style="background-image: linear-gradient(135deg, rgba(91,140,255,0.22), rgba(124,108,245,0.22))"
          >
            {{ t.question }}
          </p>
        </div>

        <!-- 回答 -->
        <div class="card p-4">
          <p v-if="t.status === 'failed'" class="text-sm text-red-400">
            {{ t.error ?? '失败了' }}
          </p>
          <p v-else class="whitespace-pre-wrap text-sm leading-relaxed text-wm-text">
            {{ t.answer || '…' }}
          </p>
          <p class="mt-2.5 flex flex-wrap items-center gap-2 border-t border-ink-800 pt-2.5 text-xs text-wm-muted">
            <span
              v-if="t.degraded"
              data-test="degraded"
              class="chip !border-amber-300/25 !bg-amber-950/40 !text-amber-300"
            >
              离线模式（规则引擎）
            </span>
            <span v-if="t.agent" class="chip">{{ t.agent }}</span>
            <span v-for="tool in t.tools" :key="tool" class="text-wm-muted">· {{ tool }}</span>
          </p>
        </div>
      </article>
    </section>

    <!-- 输入 -->
    <form class="flex gap-2" @submit.prevent="send">
      <input
        v-model="input"
        data-test="input"
        class="focus-ring flex-1 rounded-card border border-wm-line-strong bg-ink-900/80 px-4 py-2.5 text-sm text-wm-text transition placeholder:text-wm-muted focus:border-brand/60"
        placeholder="问一句…"
        :disabled="busy"
      />
      <button
        type="submit"
        class="btn-primary"
        :disabled="busy || !input.trim()"
      >
        {{ state.phase === 'asking' ? '思考中…' : '发送' }}
      </button>
    </form>

    <p v-if="state.error" class="text-sm text-red-400">{{ state.error }}</p>

    <!-- 工具清单。危险工具未开启时根本不出现在这里 —— 这本身就是信息。 -->
    <section v-if="tools.length" class="card p-4">
      <h2 class="section-title !text-base">可用工具（{{ tools.length }}）</h2>
      <ul class="mt-3 grid gap-2 sm:grid-cols-2">
        <li
          v-for="t in tools"
          :key="t.name"
          class="rounded-lg border border-ink-800 bg-ink-850/40 px-3 py-2"
        >
          <div class="flex items-center gap-2">
            <code class="text-xs text-wm-sub">{{ t.name }}</code>
            <span
              class="chip !px-1.5 !py-0 !text-[10px]"
              :class="
                t.risk === 'read_only'
                  ? '!border-emerald-800/60 !bg-emerald-950/40 !text-emerald-300'
                  : '!border-amber-800/60 !bg-amber-950/40 !text-amber-300'
              "
            >
              {{ riskLabel[t.risk] ?? t.risk }}
            </span>
          </div>
          <p class="mt-1 text-xs text-wm-muted">{{ t.description }}</p>
        </li>
      </ul>
    </section>
  </div>
</template>
