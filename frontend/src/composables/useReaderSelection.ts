import { ref } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import type { useReaderStore } from '../stores/reader'
import type { useAppStore } from '../stores/app'
import { saveReplaceRule } from '../api/replaceRule'
import { saveBook } from '../api/bookshelf'

type ReaderStore = ReturnType<typeof useReaderStore>
type AppStore = ReturnType<typeof useAppStore>
type ReplaceRuleScope = 'book' | 'source' | 'global'

interface SelectionReplaceOptions {
  replacement?: string
  isRegex?: boolean
}

export function useReaderSelection(
  store: ReaderStore,
  appStore: AppStore,
  config: ComputedRef<{ selectAction: 'popup' | 'ignore' }>,
  scrollContainerRef: Ref<HTMLElement | undefined>,
) {
  const isTouchDevice = typeof window !== 'undefined'
    && ('ontouchstart' in window || navigator.maxTouchPoints > 0)
  const isAndroid = typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent)
  const selectionMenu = ref({
    visible: false,
    text: '',
    top: 0,
    left: 0,
  })
  const activeSelectionText = ref('')
  const suppressSelectionCloseUntil = ref(0)
  let selectionMenuUpdateTimer: number | null = null

  function hideSelectionMenu() {
    selectionMenu.value.visible = false
  }

  function scheduleSelectionMenuUpdate(delay = 220) {
    if (selectionMenuUpdateTimer) {
      clearTimeout(selectionMenuUpdateTimer)
    }
    selectionMenuUpdateTimer = window.setTimeout(() => {
      updateSelectionMenu()
    }, delay)
  }

  function handleMouseUpSelection() {
    scheduleSelectionMenuUpdate(120)
  }

  function handleTouchEndSelection() {
    scheduleSelectionMenuUpdate(260)
  }

  function handleSelectionChange() {
    scheduleSelectionMenuUpdate(isAndroid ? 320 : 220)
  }

  function updateSelectionMenu() {
    if (config.value.selectAction !== 'popup') {
      hideSelectionMenu()
      return
    }

    const selection = window.getSelection?.()
    const text = selection?.toString().trim() || ''
    if (!selection || selection.rangeCount === 0 || !text || selection.isCollapsed) {
      hideSelectionMenu()
      return
    }
    if (isTouchDevice && text.length < (isAndroid ? 2 : 4)) {
      hideSelectionMenu()
      return
    }

    const container = scrollContainerRef.value
    const range = selection.getRangeAt(0)
    const commonAncestor = range.commonAncestorContainer
    const targetNode = commonAncestor.nodeType === Node.TEXT_NODE ? commonAncestor.parentElement : commonAncestor as HTMLElement | null
    if (!container || !targetNode || !container.contains(targetNode)) {
      hideSelectionMenu()
      return
    }

    const rect = range.getBoundingClientRect()
    if (!rect.width && !rect.height) {
      hideSelectionMenu()
      return
    }
    suppressSelectionCloseUntil.value = Date.now() + 250
    activeSelectionText.value = text
    selectionMenu.value = {
      visible: true,
      text: text.length > 48 ? `${text.slice(0, 48)}...` : text,
      top: isTouchDevice
        ? Math.min(window.innerHeight - 76, Math.max(16 + Math.max(0, parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--safe-area-top')) || 0), rect.bottom + 12))
        : Math.max(16 + Math.max(0, parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--safe-area-top')) || 0), rect.top - 56),
      left: Math.min(window.innerWidth - 240, Math.max(16, rect.left + rect.width / 2 - 110)),
    }
  }

  async function addSelectionBookmark(explicitText = '') {
    const selection = window.getSelection?.()
    const text = explicitText || activeSelectionText.value || selection?.toString().trim() || ''
    if (!text) return
    try {
      const pos = scrollContainerRef.value?.scrollTop || 0
      await store.addBookmark(pos, text)
      selection?.removeAllRanges()
      activeSelectionText.value = ''
      hideSelectionMenu()
      appStore.showToast('已加入书签', 'success')
    } catch {
      appStore.showToast('加入书签失败', 'error')
    }
  }

  async function addSelectionReplaceRule(
    mode: ReplaceRuleScope,
    explicitText = '',
    options: SelectionReplaceOptions = {},
  ) {
    const selection = window.getSelection?.()
    const text = explicitText || activeSelectionText.value || selection?.toString().trim() || ''
    if (!text || !store.book) return false

    try {
      const scope = mode === 'source'
        ? `source:${store.book.origin}`
        : mode === 'global'
          ? '*'
          : `book:${store.book.bookUrl}`
      const scopeLabel = mode === 'source'
        ? '书源替换'
        : mode === 'global'
          ? '全部替换'
          : '本书替换'
      const baseName = `${scopeLabel} ${text.slice(0, 20)}`
      const samePatternRule = store.replaceRules.find(rule => (
        rule.pattern === text && (rule.scope || '') === scope
      ))
      const ruleName = samePatternRule?.name || baseName
      const nextOrder = samePatternRule?.order
        ?? Math.max(0, ...store.replaceRules.map(rule => rule.order)) + 1
      await saveReplaceRule({
        id: samePatternRule?.id ?? 0,
        name: ruleName,
        pattern: text,
        replacement: options.replacement ?? '',
        scope,
        isEnabled: true,
        isRegex: options.isRegex ?? false,
        order: nextOrder,
      })
      const book = store.book
      if (book.readConfig?.useReplaceRule === false) {
        // 必须提交**整本** book: 后端对同一本书是整条覆盖(`list[i] = book.clone()`),
        // 只传 5 个字段会把用户自定义封面(`customCoverUrl`)等未兜底的字段清空。
        // 详见 `BookService::save_book` 的合并列表。
        await saveBook({
          ...book,
          readConfig: {
            ...book.readConfig,
            useReplaceRule: true,
          },
        })
        book.readConfig = {
          ...book.readConfig,
          useReplaceRule: true,
        }
      }
      await store.fetchReplaceRules()
      selection?.removeAllRanges()
      activeSelectionText.value = ''
      hideSelectionMenu()
      appStore.showToast(
        mode === 'source'
          ? '已加入书源替换规则'
          : mode === 'global'
            ? '已加入全局替换规则'
            : '已加入本书替换规则',
        'success',
      )
      return true
    } catch {
      appStore.showToast('加入替换规则失败', 'error')
      return false
    }
  }

  function clearSelectionState() {
    activeSelectionText.value = ''
    hideSelectionMenu()
    window.getSelection?.()?.removeAllRanges()
  }

  function disposeSelection() {
    if (selectionMenuUpdateTimer) {
      clearTimeout(selectionMenuUpdateTimer)
      selectionMenuUpdateTimer = null
    }
  }

  return {
    selectionMenu,
    suppressSelectionCloseUntil,
    hideSelectionMenu,
    scheduleSelectionMenuUpdate,
    handleMouseUpSelection,
    handleTouchEndSelection,
    handleSelectionChange,
    updateSelectionMenu,
    addSelectionBookmark,
    addSelectionReplaceRule,
    clearSelectionState,
    disposeSelection,
  }
}
