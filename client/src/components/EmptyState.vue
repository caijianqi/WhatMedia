<script setup lang="ts">
/**
 * 空状态。
 *
 * 四种语义必须分开，因为**用户下一步要做的事完全不同**：
 *
 *   · `empty`  —— 库是空的：去扫描。
 *   · `none`   —— 筛掉了：换个条件。
 *   · `error`  —— 请求失败：重试。
 *   · `unset`  —— 功能没配置（比如没填 TMDB Key）：去设置。
 *
 * 合成一个"暂无数据"会丢掉最关键的信息：
 * 用户看到"暂无数据"时不知道该干什么，然后就走了。
 */
export type Kind = 'empty' | 'none' | 'error' | 'unset'

const props = withDefaults(
  defineProps<{
    kind?: Kind
    title?: string
    hint?: string
    /** 主要动作按钮的文案。不传就不渲染按钮。 */
    action?: string
    /** 次要动作（例如"去设置"）。 */
    alt?: string
  }>(),
  { kind: 'empty' },
)

const emit = defineEmits<{ (e: 'action'): void; (e: 'alt'): void }>()

const PRESET: Record<Kind, { title: string; hint: string; icon: string }> = {
  empty: {
    title: '媒体库还是空的',
    hint: '先告诉服务端去哪里找影片 —— 在设置页填一个目录，然后扫描一次。',
    icon: 'M4 7h16M4 12h16M4 17h10',
  },
  none: {
    title: '没有符合条件的条目',
    hint: '换一个类型或年份试试，或者清空筛选条件。',
    icon: 'M10 10l4 4m0-4l-4 4M5 5l14 14',
  },
  error: {
    title: '这一块没加载出来',
    hint: '服务端返回了错误。其余功能不受影响。',
    icon: 'M12 8v5M12 16.5v.5M12 3l9 17H3z',
  },
  unset: {
    title: '这项功能还没配置',
    hint: '缺必要的配置项，服务端现在提供不了这个能力。',
    icon: 'M4 7h10M18 7h2M4 12h4M12 12h8M4 17h12M20 17h0',
  },
}

const preset = () => PRESET[props.kind]
</script>

<template>
  <div class="empty-state">
    <svg
      viewBox="0 0 24 24"
      style="height:35px;width:35px;stroke:#7787a7;fill:none;stroke-width:1.4;margin-bottom:10px"
      aria-hidden="true"
    >
      <path :d="preset().icon" />
    </svg>

    <strong style="display:block;font-size:14px;color:#d6def0;margin-bottom:4px">
      {{ title ?? preset().title }}
    </strong>
    <p style="font-size:11px;line-height:1.7;color:var(--muted);max-width:440px;margin:0 auto">
      {{ hint ?? preset().hint }}
    </p>

    <div v-if="action || alt" style="margin-top:16px;display:flex;gap:10px;justify-content:center">
      <button v-if="action" type="button" class="btn btn-primary" @click="emit('action')">
        {{ action }}
      </button>
      <button v-if="alt" type="button" class="btn btn-secondary" @click="emit('alt')">
        {{ alt }}
      </button>
    </div>

    <slot />
  </div>
</template>
