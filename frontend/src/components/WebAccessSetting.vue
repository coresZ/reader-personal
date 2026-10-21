<script setup lang="ts">
// Web 访问设置卡片。
//
// 只在桌面端渲染 —— 浏览器里这条配置没有意义(Web 端自己就是被访问的那一侧)。
//
// 只有三项: 是否启用 / 端口 / 是否允许局域网。与手机端 Legado 的 HTTP Server
// 对齐: 它就是"开关 + 端口", 不做账号密码。少一层登录页, 手机打开即用。
//
// **改动即生效, 没有保存按钮。** 但两类控件的时机不同:
//   - 开关: 点一下就应用;
//   - 端口: 只在**失焦/回车**时应用。不能随输入实时应用 —— 用户输 11223 的
//     过程中会依次经过 1、11、112、1122, 每个值都会重启一次监听, 既慢又可能
//     撞上被占用的低位端口。

import { computed, onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

interface WebStatus {
  enabled: boolean
  running: boolean
  port: number
  lan: boolean
  url: string | null
  error: string | null
}

const status = ref<WebStatus | null>(null)
const loading = ref(true)
const applying = ref(false)
const errorMessage = ref('')
const copied = ref(false)

const enabledDraft = ref(false)
const lanDraft = ref(false)
const portDraft = ref(1122)

const busy = computed(() => loading.value || applying.value)

const statusText = computed(() => {
  if (!status.value) return '正在读取状态'
  if (status.value.error) return status.value.error
  if (!status.value.enabled) return '未启用'
  if (!status.value.running) return '已启用，但未在监听'
  return status.value.lan ? '局域网可访问' : '仅本机可访问'
})

const statusTone = computed<'accent' | 'muted' | 'danger'>(() => {
  if (!status.value) return 'muted'
  if (status.value.error) return 'danger'
  return status.value.running ? 'accent' : 'muted'
})

/** 端口合法区间。空值(NaN)也算非法。 */
function portOk(value: number): boolean {
  return Number.isInteger(value) && value >= 1 && value <= 65535
}

function applyStatus(next: WebStatus) {
  status.value = next
  enabledDraft.value = next.enabled
  lanDraft.value = next.lan
  portDraft.value = next.port
}

async function refresh() {
  loading.value = true
  errorMessage.value = ''
  try {
    applyStatus(await invoke<WebStatus>('get_web_status'))
  } catch (err) {
    errorMessage.value = err instanceof Error ? err.message : String(err)
  } finally {
    loading.value = false
  }
}

/**
 * 把当前草稿提交给后端并重启监听。
 *
 * 并发处理: 应用期间用户还可能再点开关, 那样会同时发两个 restart 请求,
 * 后到的那个未必是最后的状态。这里用 `queued` 记住"应用期间又改过",
 * 等当前这轮结束后再补一次, 保证最终状态与界面一致。
 */
let queued = false

async function commit(): Promise<void> {
  if (applying.value) {
    queued = true
    return
  }
  applying.value = true
  errorMessage.value = ''
  try {
    do {
      queued = false
      const next = await invoke<WebStatus>('apply_web_settings', {
        settings: {
          enabled: enabledDraft.value,
          port: portDraft.value,
          lan: lanDraft.value,
        },
      })
      applyStatus(next)
      if (next.error) errorMessage.value = next.error
    } while (queued)
  } catch (err) {
    errorMessage.value = err instanceof Error ? err.message : String(err)
    // 提交失败时拉回后端的真实状态, 避免界面停在一个并未生效的值上。
    try {
      applyStatus(await invoke<WebStatus>('get_web_status'))
    } catch {
      // 连状态都读不到时保留当前草稿, 让用户能看到自己填的内容。
    }
  } finally {
    applying.value = false
  }
}

/** 开关类: 点完立刻应用。 */
async function toggleEnabled() {
  if (busy.value) return
  enabledDraft.value = !enabledDraft.value
  await commit()
}

async function toggleLan() {
  if (busy.value) return
  lanDraft.value = !lanDraft.value
  await commit()
}

/**
 * 端口: 只在失焦/回车时应用。
 *
 * 非法值(空、0、>65535)不提交, 直接回退到上一次生效的端口并提示 ——
 * 让它提交会得到后端 400, 提示反而更绕。
 */
async function commitPort() {
  if (!portOk(portDraft.value)) {
    portDraft.value = status.value?.port ?? 1122
    errorMessage.value = '端口需为 1–65535 之间的整数'
    return
  }
  if (status.value && portDraft.value === status.value.port) return
  errorMessage.value = ''
  await commit()
}

async function copyUrl() {
  const url = status.value?.url
  if (!url) return
  try {
    await navigator.clipboard.writeText(url)
    copied.value = true
    window.setTimeout(() => (copied.value = false), 1500)
  } catch {
    // 剪贴板被拒时静默: 地址本来就显示在界面上, 用户可以手抄。
  }
}

onMounted(refresh)
</script>

<template>
  <div class="web-access">
    <div class="web-access-head">
      <span class="web-access-title">Web 访问</span>
      <!-- 自动应用时没有"保存"动作可供确认, 用一个轻量状态字告诉用户后台正在重启监听。 -->
      <span v-if="applying" class="applying">应用中…</span>
    </div>

    <!-- 两行开关用同一套内边距(都在 `.web-access` 里, 由它统一给出左右留白),
         不再叠 `padding` + `padding-left`, 否则开关会左右错开。 -->
    <div class="setting-switch-row">
      <div class="setting-switch-copy">
        <span>启用 Web 访问</span>
        <small>用手机或平板的浏览器打开本机地址，即可阅读书架与正文</small>
      </div>
      <button
        class="switch-control"
        :class="{ on: enabledDraft }"
        type="button"
        role="switch"
        :aria-checked="enabledDraft"
        aria-label="启用 Web 访问"
        :disabled="busy"
        @click="toggleEnabled"
      >
        <span></span>
      </button>
    </div>

    <template v-if="enabledDraft">
      <div class="setting-switch-row">
        <div class="setting-switch-copy">
          <span>允许局域网访问</span>
          <small>{{ lanDraft ? '同一 WiFi 下的设备都能打开' : '只有这台电脑自己能打开' }}</small>
        </div>
        <button
          class="switch-control"
          :class="{ on: lanDraft }"
          type="button"
          role="switch"
          :aria-checked="lanDraft"
          aria-label="允许局域网访问"
          :disabled="busy"
          @click="toggleLan"
        >
          <span></span>
        </button>
      </div>

      <label class="field">
        <span class="field-label">端口</span>
        <input
          v-model.number="portDraft"
          type="number"
          min="1"
          max="65535"
          :disabled="busy"
          @blur="commitPort"
          @keydown.enter.prevent="commitPort"
        />
      </label>

      <div class="status-card" :class="statusTone">
        <span>{{ statusText }}</span>
        <small v-if="status?.url" class="url-row">
          <code>{{ status.url }}</code>
          <button class="copy-btn" type="button" @click="copyUrl">
            {{ copied ? '已复制' : '复制' }}
          </button>
        </small>
        <small v-else-if="status?.running">手机与电脑需连接同一局域网</small>
      </div>

      <p v-if="errorMessage" class="hint error">{{ errorMessage }}</p>
    </template>

    <!-- 关闭状态下也要有地方说明"改动已生效", 否则用户关完不知道到底有没有生效。 -->
    <div v-else class="status-card" :class="statusTone">
      <span>{{ applying ? '正在应用…' : statusText }}</span>
      <small>改动会立即生效，无需保存</small>
    </div>
  </div>
</template>

<style scoped>
/* 只写本卡片独有的部分。共享控件(开关/状态卡/按钮)见 styles/global.css。 */

/* 两行开关、端口、状态卡、按钮都在这一层, 由 `gap` 统一给间距。
 *
 * 这里**不再叠** `padding-left` + `border-left` 的分组框: 那会让「局域网」那行的
 * 开关比「启用」那行往里缩, 两行开关不在一条竖线上。分组层次改由行间空隙体现。 */
.web-access {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
  padding: var(--space-3);
  border: 1px solid var(--color-border-light);
  border-radius: var(--radius-lg);
  background: var(--color-bg);
}

.web-access-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}

.web-access-title {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--color-text);
}

/* 自动应用没有"保存"这个确认动作, 用一行小字补上反馈。 */
.applying {
  font-size: var(--text-xs);
  color: var(--color-text-tertiary);
  animation: applying-pulse 1.2s ease-in-out infinite;
}

@keyframes applying-pulse {
  0%, 100% { opacity: 0.45; }
  50% { opacity: 1; }
}

@media (prefers-reduced-motion: reduce) {
  .applying { animation: none; opacity: 0.75; }
}

/* 展开的配置区样式已并入 `.web-access` 的统一间距, 这里不再单独包裹。 */

.field {
  display: grid;
  gap: 6px;
  min-width: 0;
  max-width: 140px;
}

.field-label {
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.field input {
  width: 100%;
  min-height: 38px;
  padding: 0 var(--space-3);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-elevated);
  color: var(--color-text);
  font-size: var(--text-sm);
}

.field input:focus {
  outline: none;
  border-color: var(--color-primary);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 15%, transparent);
}

.field input:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.hint {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.6;
}

.hint.error {
  color: var(--color-danger);
}

/* 状态卡里的地址行: 地址要能一眼读出来, 且不换行把卡片撑乱。 */
.url-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.url-row code {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  background: var(--color-bg-sunken);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  word-break: break-all;
}

.copy-btn {
  padding: 0;
  border: 0;
  background: none;
  color: var(--color-primary);
  font-size: var(--text-xs);
  cursor: pointer;
}

.copy-btn:hover {
  text-decoration: underline;
}

.status-card.danger span {
  color: var(--color-danger);
}
</style>
