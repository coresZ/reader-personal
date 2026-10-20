<template>
  <Teleport to="body">
    <div v-if="active" class="source-browser-mask" :style="maskStyle" @click.self="dismiss" @keydown.esc.stop.prevent="dismiss">
      <section ref="dialog" class="source-browser-panel" role="dialog" aria-modal="true" :aria-label="title" tabindex="-1" :style="panelStyle">
        <header class="source-browser-head" :class="{ draggable: draggable }" @pointerdown="startDrag" @pointermove="moveDrag" @pointerup="endDrag" @pointercancel="endDrag">
          <span class="source-browser-title">{{ title }}</span>
          <button type="button" class="source-browser-close" @click="closeActive">{{ active.awaitResult ? '取消' : stack.length > 1 ? '返回' : '关闭' }}</button>
          <button v-if="error && !active.ready" type="button" @click="reload">重新加载</button>
          <button v-if="active.awaitResult" type="button" class="source-browser-confirm" :disabled="!active.ready || confirming" @click="confirm">{{ confirming ? '正在保存…' : '确认' }}</button>
        </header>
        <p v-if="error" class="source-browser-error" role="alert">{{ error }}</p>
        <p v-else-if="message" class="source-browser-message" role="status">{{ message }}</p>
        <iframe
          v-for="panel in stack" v-show="panel === active" :key="panel.panelId"
          :ref="(element) => setFrame(panel.panelId, element)"
          class="source-browser-frame" :title="panel.title || '书源网页'" :src="panelUrl(panel)"
          sandbox="allow-scripts allow-forms allow-popups allow-modals allow-same-origin"
          @load="checkReady(panel)"
        ></iframe>
      </section>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, type ComponentPublicInstance, type CSSProperties } from 'vue'
import { closeSourcePanel, type ShowBrowserRequest } from '../api/sourceClick'
import { readerOrigin } from '../api/scheme'

const props = defineProps<{ request: ShowBrowserRequest | null; background?: string; color?: string }>()
const emit = defineEmits<{
  close: []
  loginData: [values: Record<string, unknown> | null]
  refreshLogin: []
  toast: [message: string]
  /** 网页里点了书源仓库的「一键导入」链接(legado:// 或 yuedu://)。 */
  importLink: [url: string]
}>()
type Panel = ShowBrowserRequest & {
  panelId: string
  ready: boolean
  documentId?: string
  readyRequestId?: string
  navigationUrl?: string
}
const stack = ref<Panel[]>([])
const active = computed(() => stack.value.at(-1))
const title = computed(() => active.value?.title || active.value?.url || '书源网页')
const options = computed(() => active.value?.options || {})
const draggable = computed(() => options.value.isDraggable !== false)
const dialog = ref<HTMLElement | null>(null)
const frames = new Map<string, HTMLIFrameElement>()
const readyTimers = new Map<string, number>()
const confirming = ref(false)
const error = ref('')
const message = ref('')
const resizedHeight = ref<number | null>(null)
let confirmation: { panelId: string; requestId: string; timer: number } | null = null
let drag: { y: number; height: number } | null = null
let previousFocus: HTMLElement | null = null

function numberOption(name: string, fallback: number, min: number, max: number) {
  const value = options.value[name]
  return typeof value === 'number' && Number.isFinite(value) ? Math.max(min, Math.min(max, value)) : fallback
}
const maskStyle = computed(() => ({
  background: `rgba(0,0,0,${options.value.shouldDimBackground === false ? 0 : numberOption('backgroundDimAmount', .42, 0, 1)})`,
}))
const panelStyle = computed<CSSProperties>(() => ({
  height: resizedHeight.value ? `${resizedHeight.value}px` : `${numberOption('heightPercentage', .82, .2, 1) * 100}dvh`,
  width: typeof options.value.widthPercentage === 'number' ? `${numberOption('widthPercentage', 1, .2, 1) * 100}vw` : 'min(880px, 100%)',
  maxWidth: typeof options.value.maxWidth === 'number' ? `${numberOption('maxWidth', 880, 280, 4000)}px` : undefined,
  maxHeight: typeof options.value.maxHeight === 'number' ? `${numberOption('maxHeight', 900, 160, 4000)}px` : '100dvh',
  borderRadius: `${numberOption('expandedCornersRadius', 16, 0, 80)}px ${numberOption('expandedCornersRadius', 16, 0, 80)}px 0 0`,
  background: props.background || 'var(--color-bg-elevated)',
  color: props.color || 'var(--color-text)',
}))

function requestId() { return globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random()}` }
function clearReadyTimer(panelId: string) {
  window.clearTimeout(readyTimers.get(panelId))
  readyTimers.delete(panelId)
}
function clearConfirmation(panelId?: string) {
  if (panelId && confirmation?.panelId !== panelId) return
  if (confirmation) window.clearTimeout(confirmation.timer)
  confirmation = null
  confirming.value = false
}
function release(panel: Panel) {
  clearReadyTimer(panel.panelId)
  clearConfirmation(panel.panelId)
  void closeSourcePanel(panel.panelId).catch(() => {})
}
function resetPresentation() {
  clearConfirmation()
  error.value = ''
  message.value = ''
  resizedHeight.value = null
  drag = null
}
function unavailable(panel: Panel, reason: string) {
  clearReadyTimer(panel.panelId)
  clearConfirmation(panel.panelId)
  panel.ready = false
  panel.readyRequestId = undefined
  if (panel === active.value) {
    error.value = reason
    message.value = ''
  }
}
function markLoading(panel: Panel) {
  clearReadyTimer(panel.panelId)
  clearConfirmation(panel.panelId)
  panel.ready = false
  panel.readyRequestId = undefined
  if (panel === active.value) {
    error.value = ''
    message.value = '正在加载网页…'
  }
  readyTimers.set(panel.panelId, window.setTimeout(() => {
    unavailable(panel, '网页加载超时，请重新加载或取消后重试')
  }, 30_000))
}
function postToPanel(panel: Panel, data: Record<string, unknown>): boolean {
  const target = frames.get(panel.panelId)?.contentWindow
  if (!target) return false
  try { target.postMessage({ ...data, panelId: panel.panelId }, readerOrigin); return true }
  catch { return false }
}
function checkReady(panel: Panel) {
  markLoading(panel)
  clearReadyTimer(panel.panelId)
  const id = requestId()
  panel.readyRequestId = id
  readyTimers.set(panel.panelId, window.setTimeout(() => {
    if (panel.readyRequestId === id) unavailable(panel, '当前网页无法确认，请重新加载或取消后重试')
  }, 5_000))
  if (!postToPanel(panel, { type: 'reader:ready', requestId: id })) {
    unavailable(panel, '网页尚未打开，请重新加载后重试')
  }
}
watch(() => props.request, async (request) => {
  stack.value.forEach(release)
  stack.value = []
  frames.clear()
  resetPresentation()
  if (!request?.panelId) return
  previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
  stack.value = [{ ...request, panelId: request.panelId, ready: false }]
  markLoading(stack.value[0])
  await nextTick()
  dialog.value?.focus()
}, { immediate: true })

function setFrame(id: string, element: Element | ComponentPublicInstance | null) {
  if (element instanceof HTMLIFrameElement) frames.set(id, element)
  else frames.delete(id)
}
function panelUrl(panel: Panel) { return `${readerOrigin}/sourcePanel?__readerPanelId=${encodeURIComponent(panel.panelId)}` }
function normalizedPageUrl(value: string) {
  try { return new URL(value).href }
  catch { return value }
}
function reload() {
  const panel = active.value
  const frame = panel && frames.get(panel.panelId)
  if (!panel || !frame) return
  markLoading(panel)
  const url = new URL(panelUrl(panel))
  const target = panel.navigationUrl || panel.url
  if (/^https?:\/\//i.test(target) && normalizedPageUrl(target) !== normalizedPageUrl(panel.url)) {
    url.searchParams.set('__readerPanelUrl', target)
  }
  frame.src = url.href
}
function closeActive() {
  const panel = stack.value.pop()
  if (panel) { release(panel); frames.delete(panel.panelId) }
  resetPresentation()
  if (!stack.value.length) {
    emit('close')
    void nextTick(() => previousFocus?.focus())
  } else if (!active.value?.ready) {
    const parent = active.value
    if (parent) void nextTick(() => checkReady(parent))
  }
}
function dismiss() { if (options.value.dismissOnTouchOutside !== false) closeActive() }
function confirm() {
  const panel = active.value
  if (!panel?.ready || !panel.documentId || confirming.value) return
  const id = requestId()
  confirming.value = true
  error.value = ''
  confirmation = {
    panelId: panel.panelId, requestId: id,
    timer: window.setTimeout(() => {
      if (confirmation?.requestId === id) unavailable(panel, '网页未响应确认，请重新加载后重试')
    }, 12_000),
  }
  if (!postToPanel(panel, { type: 'reader:confirm', requestId: id, documentId: panel.documentId })) {
    unavailable(panel, '网页未响应确认，请重新加载后重试')
  }
}

function handleMessage(event: MessageEvent) {
  const data = event.data
  if (event.origin !== readerOrigin || !data || typeof data !== 'object') return
  const panel = stack.value.find((item) => item.panelId === data.panelId)
  if (!panel || event.source !== frames.get(panel.panelId)?.contentWindow) return
  if (data.type === 'ready') {
    // A fresh challenge ties readiness to the document that is loaded now.
    if (!data.requestId) { checkReady(panel); return }
    if (data.requestId !== panel.readyRequestId || typeof data.documentId !== 'string') return
    clearReadyTimer(panel.panelId)
    panel.documentId = data.documentId
    panel.ready = true
    if (typeof data.url === 'string' && /^https?:\/\//i.test(data.url)) panel.navigationUrl = data.url
    if (typeof data.title === 'string' && data.title) panel.title = data.title
    if (panel === active.value) { error.value = ''; message.value = '' }
    return
  }
  if (panel.documentId && data.documentId !== panel.documentId) return
  if (data.type === 'navigating') { markLoading(panel); return }
  if (data.type === 'navigationCancelled') { checkReady(panel); return }
  if (data.type === 'navigationError') {
    unavailable(panel, String(data.message || '网页跳转失败，请重新加载后重试'))
    return
  }
  if (data.type === 'location') {
    if (typeof data.url === 'string' && /^https?:\/\//i.test(data.url)) panel.navigationUrl = data.url
    return
  }
  if (panel !== active.value) return
  switch (data.type) {
    case 'complete':
      if (confirmation?.panelId === panel.panelId && data.requestId === confirmation.requestId) closeActive()
      break
    case 'close': closeActive(); break
    case 'dismiss': dismiss(); break
    case 'error':
      if (data.requestId && data.requestId !== confirmation?.requestId) return
      error.value = String(data.message || '网页操作失败')
      clearConfirmation(panel.panelId)
      break
    case 'toast': message.value = String(data.message || ''); emit('toast', message.value); break
    case 'importProtocol':
      // 书源仓库的「一键导入」: 面板脚本截下 legado:// / yuedu:// 后转过来。
      // 导入逻辑不放在这里 —— 面板只负责把链接送达, 由上层决定拉取与入库。
      if (typeof data.url === 'string' && data.url) emit('importLink', data.url)
      break
    case 'config':
      if (data.options && typeof data.options === 'object' && !Array.isArray(data.options)) {
        panel.options = { ...panel.options, ...data.options }
        resizedHeight.value = null
      }
      break
    case 'loginData':
      if (data.values === null || (typeof data.values === 'object' && !Array.isArray(data.values))) emit('loginData', data.values)
      break
    case 'refreshLogin': emit('refreshLogin'); break
    case 'showBrowser': {
      const request = data.request as ShowBrowserRequest | undefined
      if (!request?.panelId || stack.value.some((item) => item.panelId === request.panelId)) return
      stack.value.push({ ...request, panelId: request.panelId, ready: false })
      resetPresentation()
      markLoading(active.value!)
      break
    }
  }
}

function startDrag(event: PointerEvent) {
  if (!draggable.value || event.button !== 0 || (event.target as Element).closest('button')) return
  drag = { y: event.clientY, height: dialog.value?.getBoundingClientRect().height || 600 }
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function moveDrag(event: PointerEvent) {
  if (drag) resizedHeight.value = Math.max(200, Math.min(window.innerHeight, drag.height + drag.y - event.clientY))
}
function endDrag() { drag = null }
onMounted(() => window.addEventListener('message', handleMessage))
onBeforeUnmount(() => {
  window.removeEventListener('message', handleMessage)
  stack.value.forEach(release)
  clearConfirmation()
})
</script>

<style scoped>
.source-browser-mask { position: fixed; inset: 0; z-index: calc(var(--z-modal, 1000) + 4); display: flex; align-items: flex-end; justify-content: center; }
.source-browser-panel { display: flex; flex-direction: column; overflow: hidden; outline: none; box-shadow: var(--shadow-xl, 0 -12px 34px #0004); }
.source-browser-head { display: flex; align-items: center; flex: none; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--color-border-light); }
.source-browser-head.draggable { cursor: ns-resize; touch-action: none; }
.source-browser-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; }
.source-browser-head button { flex: none; min-height: 34px; padding: 6px 16px; border: 1px solid var(--color-border); border-radius: var(--radius-md, 8px); background: var(--color-bg); color: inherit; font-size: 13px; cursor: pointer; }
.source-browser-head .source-browser-confirm { background: var(--color-primary-soft-bg); color: var(--color-primary-soft-text); border-color: var(--color-primary-soft-border); }
.source-browser-head button:disabled { opacity: .5; cursor: default; }
.source-browser-frame { flex: 1; min-height: 0; width: 100%; border: 0; background: #fff; }
.source-browser-error, .source-browser-message { flex: none; margin: 0; padding: 10px 16px; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13px; }
.source-browser-error { color: var(--color-danger, #b23b38); }
</style>
