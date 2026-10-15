<template>
  <Transition name="fade">
    <div
      v-if="show"
      ref="panelRef"
      class="reader-search-panel"
      :class="{ 'is-dragged': !!dragPosition, 'is-dragging': dragging }"
      :style="[{ background: theme.popup, color: theme.fontColor }, panelStyle]"
    >
      <div
        class="reader-search-header"
        title="按住拖动面板"
        @pointerdown="startDrag"
        @pointermove="moveDrag"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      >
        <div class="reader-search-title">章节搜索</div>
        <div class="reader-search-header-actions">
          <button
            v-if="dragPosition"
            type="button"
            class="reset-panel-position"
            title="复位面板位置"
            aria-label="复位面板位置"
            @click="resetDrag"
          >复位</button>
          <button @click="$emit('close')" class="close-search">×</button>
        </div>
      </div>
      <div class="reader-search-form">
        <input
          :value="query"
          placeholder="搜索当前章节内容..."
          ref="inputRef"
          @input="$emit('update:query', ($event.target as HTMLInputElement).value)"
          @keyup.enter="$emit('search')"
        />
        <button class="search-submit" @click="$emit('search')" :disabled="!query">
          搜索
        </button>
      </div>
      <div class="search-status-row">
        <span v-if="count">{{ activeIndex + 1 }} / {{ count }}</span>
        <span v-else>{{ status }}</span>
        <div class="search-actions">
          <button @click="$emit('prev')" :disabled="!count">↑</button>
          <button @click="$emit('next')" :disabled="!count">↓</button>
        </div>
      </div>
      <div class="reader-search-results">
        <div v-if="!query" class="search-empty">输入关键词开始搜索</div>
        <div v-else-if="!results.length" class="search-empty">当前章节没有匹配内容</div>
        <button
          v-for="(result, idx) in results"
          :key="`${result.chapterIndex}-${idx}-${result.snippet}`"
          class="search-result-item"
          :class="{ active: idx === activeIndex }"
          @click="$emit('jump', result, idx)"
        >
          <div class="search-result-title">{{ result.chapterTitle }}</div>
          <div class="search-result-snippet">{{ result.snippet }}</div>
        </button>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import type { SearchResultItem } from '../../composables/useReaderSearch'
import type { ThemePreset } from '../../stores/reader'

const props = defineProps<{
  show: boolean
  theme: ThemePreset | { popup: string; fontColor: string }
  query: string
  results: SearchResultItem[]
  activeIndex: number
  count: number
  status: string
}>()

const emit = defineEmits<{
  close: []
  search: []
  next: []
  prev: []
  'update:query': [value: string]
  jump: [result: SearchResultItem, idx: number]
}>()

const inputRef = ref<HTMLInputElement>()
const panelRef = ref<HTMLElement>()

watch(() => props.show, (visible) => {
  if (visible) {
    setTimeout(() => inputRef.value?.focus(), 0)
  }
})

/* ─── 拖动 ───
 *  面板默认停在标题栏下方的固定位置, 会压住正文; 宽窗口下正文居中留白时尤其
 *  碍事。这里让标题栏当把手把面板挪开。
 *
 *  位置状态一旦产生就改用 left/top 定位(见 panelStyle): 默认定位是 top + right,
 *  不固化成 left/top 的话第一帧会从原位置"跳"到左上角。 */
const MIN_PANEL_WIDTH = 240
const VIEWPORT_MARGIN = 8

interface DragPosition { x: number; y: number; width: number }

const dragPosition = ref<DragPosition | null>(null)
const dragging = ref(false)
/** 拖动中的指针 id。只认这一根指针: 触摸时第二根手指也会派发 pointerdown,
 *  不按 id 过滤就会覆盖起点、把面板瞬移到第二指处, 让它抬起还会中断拖动。 */
let dragPointerId: number | null = null
let dragOrigin: { pointerX: number; pointerY: number; left: number; top: number } | null = null

const panelStyle = computed(() => {
  const position = dragPosition.value
  if (!position) return {}
  return {
    left: `${position.x}px`,
    top: `${position.y}px`,
    // 默认定位与移动端媒体查询都用到 right/bottom, 不显式清掉会与 left/top 冲突
    // (两者同时生效会把面板横向拉伸)。
    right: 'auto',
    bottom: 'auto',
    width: `${position.width}px`,
  }
})

/** 与 CSS 的宽度规则保持一致(见下方 .reader-search-panel 及其 768px 断点)。 */
function preferredPanelWidth() {
  if (typeof window === 'undefined') return 420
  return window.innerWidth <= 768
    ? Math.max(MIN_PANEL_WIDTH, window.innerWidth - 32)
    : Math.min(420, window.innerWidth - 120)
}

/** 拖动上界: 面板默认位置就在标题栏下方, 允许拖到标题栏底下会让把手被盖住点不回来。 */
function topLimit() {
  const raw = typeof window === 'undefined'
    ? ''
    : getComputedStyle(document.documentElement).getPropertyValue('--titlebar-height')
  const parsed = Number.parseFloat(raw)
  return (Number.isFinite(parsed) && parsed > 0 ? parsed : 32) + VIEWPORT_MARGIN
}

/** 夹进视口: 拖出屏幕的面板既看不到也点不回来。 */
function clampToViewport(x: number, y: number, width?: number): DragPosition {
  const rect = panelRef.value?.getBoundingClientRect()
  const panelWidth = width ?? rect?.width ?? MIN_PANEL_WIDTH
  const panelHeight = rect?.height ?? 0
  const finalWidth = Math.min(panelWidth, Math.max(MIN_PANEL_WIDTH, window.innerWidth - VIEWPORT_MARGIN * 2))
  const limit = topLimit()
  return {
    x: Math.min(Math.max(VIEWPORT_MARGIN, x), Math.max(VIEWPORT_MARGIN, window.innerWidth - finalWidth - VIEWPORT_MARGIN)),
    y: Math.min(Math.max(limit, y), Math.max(limit, window.innerHeight - panelHeight - VIEWPORT_MARGIN)),
    width: finalWidth,
  }
}

function startDrag(event: PointerEvent) {
  if (event.button !== 0) return
  // 把手上的按钮(关闭/复位)照旧点击, 不参与拖动。
  if ((event.target as Element | null)?.closest('button')) return
  // 已经有一根指针在拖时, 忽略后来者(多点触摸)。
  if (dragPointerId !== null) return
  const panel = panelRef.value
  if (!panel) return
  const rect = panel.getBoundingClientRect()
  dragPosition.value = { x: rect.left, y: rect.top, width: rect.width }
  dragOrigin = { pointerX: event.clientX, pointerY: event.clientY, left: rect.left, top: rect.top }
  dragPointerId = event.pointerId
  dragging.value = true
  // 阻止焦点转移与文本选择: 拖动把手不该让搜索框失焦(移动端会顺势收起键盘)。
  event.preventDefault()
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}

function moveDrag(event: PointerEvent) {
  if (!dragging.value || !dragOrigin || event.pointerId !== dragPointerId) return
  dragPosition.value = clampToViewport(
    dragOrigin.left + (event.clientX - dragOrigin.pointerX),
    dragOrigin.top + (event.clientY - dragOrigin.pointerY),
    dragPosition.value?.width,
  )
}

function endDrag(event: PointerEvent) {
  if (!dragging.value || event.pointerId !== dragPointerId) return
  dragging.value = false
  dragOrigin = null
  dragPointerId = null
  const target = event.currentTarget as HTMLElement | null
  if (target?.hasPointerCapture(event.pointerId)) target.releasePointerCapture(event.pointerId)
}

/** 双击把手复位到默认位置(回到 CSS 的 top/right 定位)。 */
function resetDrag() {
  dragPosition.value = null
  dragOrigin = null
  dragging.value = false
  dragPointerId = null
}

/** 窗口尺寸变化后原来的坐标可能已经越界, 按新视口重新夹一次并跟上看板宽度。 */
function handleViewportResize() {
  const current = dragPosition.value
  if (!current) return
  dragPosition.value = clampToViewport(current.x, current.y, preferredPanelWidth())
}

onMounted(() => window.addEventListener('resize', handleViewportResize))
onUnmounted(() => window.removeEventListener('resize', handleViewportResize))
</script>

<style scoped>
.reader-search-panel {
  position: fixed;
  top: calc(var(--titlebar-height, 32px) + 24px + var(--safe-area-top));
  right: 80px;
  width: min(420px, calc(100vw - 120px));
  /* 视口很矮时(移动端软键盘弹起、迷你窗口)不能让面板高过可视区: 超出部分既看不全,
   * 也会让拖动的位置夹取失去上界。结果列表自己滚动。 */
  max-height: calc(100vh - var(--titlebar-height, 32px) - 40px);
  padding: 14px 16px;
  border-radius: 16px;
  box-shadow: 0 4px 20px rgba(0,0,0,0.15);
  display: flex;
  flex-direction: column;
  gap: 12px;
  /* 必须压过阅读页控件(ReaderMobileControls 是 z-index: 30)与滚动容器(z-index: 1):
   * 同层级时后渲染的悬浮钮会从面板右下角透出来, 看着像是搜索窗口里的图标。 */
  z-index: 35;
  border: 1px solid rgba(0,0,0,0.05);
}

.reader-search-header,
.search-status-row,
.reader-search-form {
  display: flex;
  align-items: center;
}

.reader-search-header,
.search-status-row {
  justify-content: space-between;
  gap: 12px;
}

/* 标题栏是拖动把手 */
.reader-search-header {
  cursor: move;
  touch-action: none;
  user-select: none;
  -webkit-user-select: none;
}

/* 拖过位置的面板自由浮动, 不再受标题栏下方的固定定位约束 */
.reader-search-panel.is-dragged {
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.18);
}

/* 拖动中: 整块面板都不参与文本选择, 跨到结果列表上也不会误选 */
.reader-search-panel.is-dragging {
  user-select: none;
  -webkit-user-select: none;
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.22);
}

.reader-search-title {
  font-size: 15px;
  font-weight: 700;
}

.reader-search-header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.reset-panel-position {
  border: none;
  background: transparent;
  color: inherit;
  font-size: 12px;
  line-height: 1;
  padding: 4px 6px;
  border-radius: 6px;
  opacity: 0.6;
  cursor: pointer;
}

.reset-panel-position:hover {
  opacity: 0.9;
  background: rgba(0, 0, 0, 0.06);
}

.reader-search-form {
  gap: 10px;
}

.reader-search-form input {
  flex: 1;
  min-width: 0;
  background: rgba(0, 0, 0, 0.04);
  border: 1px solid rgba(0, 0, 0, 0.06);
  outline: none;
  color: inherit;
  font-size: 14px;
  padding: 11px 12px;
  border-radius: 12px;
}

.search-submit {
  border: 1px solid color-mix(in srgb, var(--color-primary) 34%, transparent);
  border-radius: 12px;
  /* 阅读页专用写法: 底色走 --color-primary 的淡色(该变量在 .reader-view 上被
   * 覆盖成立即生效), 文字用 currentColor 跟随阅读主题的 fontColor。
   * 不能像应用页面那样混入 --color-text —— 那是应用主题的文字色, 与阅读主题
   * 无关; 两者错配时(浅色应用 + 暗夜阅读)对比度只剩 2.6:1。 */
  background: color-mix(in srgb, var(--color-primary) 14%, transparent);
  color: currentColor;
  font-size: 13px;
  padding: 11px 14px;
  cursor: pointer;
}

.search-submit:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-primary) 20%, transparent);
}

.search-submit:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.reader-search-results {
  max-height: min(48vh, 420px);
  /* 面板本身有 max-height: 让结果列表成为唯一的收缩项, 否则矮视口下它会顶破面板。 */
  min-height: 0;
  flex: 1 1 auto;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.search-result-item {
  border: 1px solid rgba(0,0,0,0.06);
  background: rgba(0,0,0,0.02);
  color: inherit;
  border-radius: 12px;
  padding: 10px 12px;
  text-align: left;
  cursor: pointer;
}

.search-result-item.active {
  border-color: var(--color-primary);
  background: rgba(201, 127, 58, 0.08);
}

.search-result-title {
  font-size: 13px;
  font-weight: 700;
  margin-bottom: 4px;
}

.search-result-snippet {
  font-size: 12px;
  line-height: 1.55;
  opacity: 0.78;
  word-break: break-all;
}

.search-empty {
  text-align: center;
  opacity: 0.55;
  padding: 28px 12px;
  font-size: 13px;
}

.search-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  opacity: 0.8;
}

.search-actions button {
  background: rgba(0,0,0,0.05);
  border: none;
  width: 24px;
  height: 24px;
  border-radius: 4px;
  cursor: pointer;
  color: inherit;
}

.close-search {
  border: none;
  background: transparent;
  color: inherit;
  font-size: 20px;
  line-height: 1;
  opacity: 0.65;
  cursor: pointer;
}

@media (max-width: 768px) {
  .reader-search-panel {
    top: auto;
    bottom: calc(80px + var(--safe-area-bottom));
    right: 16px;
    left: 16px;
    width: auto;
  }
}
</style>
