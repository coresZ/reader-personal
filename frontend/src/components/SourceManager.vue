<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="modelValue" class="modal-overlay" @click="close"></div>
    </Transition>

    <Transition name="scale">
      <div v-if="modelValue" class="modal-container" @click.self="close">
        <div class="source-modal">
          <SourceManagerHeader
            :total="sourceStats.total"
            :enabled="sourceStats.enabled"
            :filtered="sourceStats.filtered"
            :selected="selectedFilteredSources.length"
            :loading="loading"
            :testing="testingSources"
            :stopping="sourceTestStopRequested && testingSources"
            :test-completed="sourceTestProgress.completed"
            :test-total="sourceTestProgress.total"
            :invalid-count="invalidSources.length"
            @refresh="loadSources"
            @import-local="triggerFileImport"
            @open-subscriptions="subscriptionPanelVisible = true"
            @export="exportSources"
            @test-sources="testSources()"
            @cancel-test="cancelSourceTest"
            @delete-invalid="removeInvalidSources"
            @create="createSource"
            @close="close"
          />

          <input
            ref="fileInputRef"
            type="file"
            accept=".json,.txt"
            class="hidden-input"
            @change="handleFileImport"
          />

          <SourceFilterBar
            :filter-text="filterText"
            :filter-group="filterGroup"
            :groups="groupList"
            :all-selected="allFilteredSelected"
            :partially-selected="partiallyFilteredSelected"
            :selected-count="selectedFilteredSources.length"
            :has-sources="filteredSources.length > 0"
            @update:filter-text="filterText = $event"
            @update:filter-group="filterGroup = $event"
            @toggle-current-selection="toggleFilteredSelection"
            @delete-selection="removeSelectedSources"
            @more-selection="handleSelectionMore"
          />

          <div class="content-grid">
            <SourceList
              class="source-list-slot"
              :sources="filteredSources"
              :loading="loading"
              :selected-urls="selectedSourceUrls"
              :active-url="editingSource?.bookSourceUrl"
              :empty-title="sources.length ? '没有匹配的书源' : '暂无书源'"
              :empty-description="sources.length ? '调整搜索关键词或分组筛选后再试' : '可以本地导入、网络导入或手动新增'"
              @edit="editSource"
              @toggle-enabled="toggleSource"
              @toggle-selection="toggleSourceSelection"
              @delete="removeSource"
              @pin="pinSource"
              @unpin="unpinSource"
              @contextmenu="handleSourceContextMenu"
            />

            <SourceEditorPanel
              class="editor-slot"
              :source="editingSource"
              :editor-text="editorText"
              :can-login="canLoginSource"
              :login-loading="sourceLoginLoading"
              :reset-signal="resetSignal"
              @update:editor-text="editorText = $event"
              @format="formatEditor"
              @save="saveEditor"
              @login="handleSourceLogin"
              @create="createSource"
              @import-local="triggerFileImport"
              @more="handleEditorMore"
            />
          </div>

          <footer class="modal-footer">
            <span class="count-info">显示 {{ filteredSources.length }} / {{ sources.length }}</span>
          </footer>
        </div>
      </div>
    </Transition>

    <SourceSubscriptionPanel
      v-model:remote-url="remoteUrl"
      :visible="subscriptionPanelVisible"
      :subscriptions="subscriptions"
      :syncing="Boolean(syncingSubscriptionUrl)"
      :syncing-url="syncingSubscriptionUrl"
      @sync="importRemoteSource"
      @save="saveSubscription"
      @sync-subscription="syncSubscription"
      @remove-subscription="removeSubscription"
      @close="subscriptionPanelVisible = false"
    />

    <SourceLoginDialog v-model="loginPreviewVisible" :source="loginSource" />

    <SourceGroupPrompt
      :visible="groupPromptVisible"
      :title="groupPromptMode === 'add' ? '添加到分组' : '从分组移除'"
      :message="groupPromptMessage"
      :confirm-text="groupPromptMode === 'add' ? '添加' : '移除'"
      :suggestions="groupList"
      @confirm="handleGroupPromptConfirm"
      @close="closeGroupPrompt"
    />
  </Teleport>
</template>

<script setup lang="ts">
import { ref, computed, onBeforeUnmount, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  getBookSources,
  deleteBookSource,
  deleteBookSources,
  deleteInvalidBookSources,
  saveBookSource,
  saveBookSources,
  testBookSources,
  cancelBookSourceTest,
  readRemoteSourceFile,
  readSourceFile,
  exportBookSources,
  pinBookSource,
  unpinBookSource,
} from '../api/source'
import { useAppStore } from '../stores/app'
import { isTauriRuntime } from '../api/transport'
import { useSourceStore } from '../stores/source'
import type { BookSource } from '../types'
import {
  addBookSourceGroups,
  filterBookSources,
  getBookSourceGroups,
  getBookSourceStats,
  getVisibleSelection,
  isEditorTextUnchanged,
  moveBookSourcesToEdge,
  removeBookSourceGroups,
  toBookSourceDeletePayload,
} from '../utils/sourceSelection'
import { chunkBookSourceUrls, mergeBookSourceTestResponses } from '../utils/sourceTesting'
import SourceEditorPanel from './source-manager/SourceEditorPanel.vue'
import SourceLoginDialog from './source-manager/SourceLoginDialog.vue'
import SourceFilterBar from './source-manager/SourceFilterBar.vue'
import SourceList from './source-manager/SourceList.vue'
import { showContextMenu, type MenuItem } from '../composables/useContextMenu'
import SourceManagerHeader from './source-manager/SourceManagerHeader.vue'
import SourceSubscriptionPanel from './source-manager/SourceSubscriptionPanel.vue'
import SourceGroupPrompt from './source-manager/SourceGroupPrompt.vue'

type SourceSubscription = {
  url: string
  lastSyncedAt?: number
}

const SUBSCRIPTION_KEY = 'reader-source-subscriptions'

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

const appStore = useAppStore()
const sourceStore = useSourceStore()

const sources = ref<BookSource[]>([])
const loading = ref(false)
const testingSources = ref(false)
const sourceTestProgress = ref({ completed: 0, total: 0 })
const sourceTestTaskId = ref('')
const sourceTestStopRequested = ref(false)
let stopSourceTestProgressListener: UnlistenFn | null = null
const filterText = ref('')
const filterGroup = ref('')
const fileInputRef = ref<HTMLInputElement | null>(null)
const selectedSourceUrls = ref(new Set<string>())

const subscriptionPanelVisible = ref(false)
const remoteUrl = ref('')
const subscriptions = ref<SourceSubscription[]>(loadSubscriptions())
/** 正在同步的订阅链接(空串表示没有进行中的同步); 表单同步与列表同步共用。 */
const syncingSubscriptionUrl = ref('')

const editingSource = ref<BookSource | null>(null)
const editorText = ref(JSON.stringify(createEmptySource(), null, 2))
/**
 * 最近一次载入编辑器的原文(editSource/createSource 写入)。
 * 用来判断"用户是否改过编辑器内容": 不能拿 editingSource 现序列化来比 ——
 * toggleSource 会就地改那个对象, 比出来必然是"没改过", 于是把用户的编辑丢掉。
 */
const editorBaseline = ref(editorText.value)
const resetSignal = ref(0)
const sourceLoginLoading = ref(false)
const loginPreviewVisible = ref(false)
const loginSource = ref<BookSource | null>(null)

/** "更多"菜单里的分组操作输入弹窗(参考 legado 的 suggestion 输入框)。 */
const groupPromptVisible = ref(false)
const groupPromptMode = ref<'add' | 'remove'>('add')
/** 分组弹窗的目标(空数组表示用当前多选集合; 单源菜单传入那一个源)。 */
const groupPromptTargets = ref<BookSource[]>([])
/** 打开弹窗时快照的多选计数: 关闭动画期间 targets 已清空, 依赖它保持文案稳定。 */
const groupPromptFallbackCount = ref(0)

/** 分组弹窗说明文案: 单源时点名书源, 多选时说数量。
 *
 *  目标在打开弹窗时快照到 `groupPromptTargets`(单源菜单传 1 个), 为 0 表示
 *  "用当前多选集合"。这里不用 `selectedFilteredSources.length` 兜底计数:
 *  ① 弹窗关闭动画期间 `closeGroupPrompt()` 已清空 targets, 若现算计数会闪成
 *  「0 个」; ② 多元素 explicitTargets 时现算计数会与实际目标不符。
 *  因此计数也取自同一份快照, 在打开时一并算好。 */
const groupPromptMessage = computed(() => {
  const verb = groupPromptMode.value === 'add' ? '添加' : '移除'
  const targets = groupPromptTargets.value
  if (targets.length === 1) {
    return `将对「${targets[0].bookSourceName}」${verb}以下分组`
  }
  const count = groupPromptTargets.value.length || groupPromptFallbackCount.value
  return `将对选中的 ${count} 个书源${verb}以下分组`
})

const groupList = computed(() => getBookSourceGroups(sources.value))

watch(groupList, (groups) => {
  if (filterGroup.value && !groups.includes(filterGroup.value)) {
    filterGroup.value = ''
  }
})

const filteredSources = computed(() =>
  filterBookSources(sources.value, filterText.value, filterGroup.value)
)

const sourceStats = computed(() => getBookSourceStats(sources.value, filteredSources.value))

const selectedFilteredSources = computed(() =>
  getVisibleSelection(filteredSources.value, selectedSourceUrls.value, (source) => source.bookSourceUrl)
)

const invalidSources = computed(() =>
  sources.value.filter((source) => hasSourceGroup(source, '失效'))
)

const allFilteredSelected = computed(() =>
  filteredSources.value.length > 0 && selectedFilteredSources.value.length === filteredSources.value.length
)

const partiallyFilteredSelected = computed(() =>
  selectedFilteredSources.value.length > 0 && !allFilteredSelected.value
)

const canLoginSource = computed(() => {
  try {
    const parsed = JSON.parse(editorText.value) as BookSource
    return Boolean(parsed.bookSourceUrl?.trim() && parsed.bookSourceName?.trim())
  } catch {
    return false
  }
})

function createEmptySource(): BookSource {
  return {
    bookSourceName: '新增书源',
    bookSourceUrl: '',
    enabled: true,
  }
}

function loadSubscriptions(): SourceSubscription[] {
  try {
    const raw = localStorage.getItem(SUBSCRIPTION_KEY)
    return raw ? JSON.parse(raw) : []
  } catch {
    return []
  }
}

function persistSubscriptions() {
  localStorage.setItem(SUBSCRIPTION_KEY, JSON.stringify(subscriptions.value))
}

async function loadSources() {
  loading.value = true
  try {
    sources.value = await getBookSources()
    pruneSelection()
    syncEditingSource()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message, 'error')
  } finally {
    loading.value = false
    // Keep the shared source store in sync so the search page's group/source
    // dropdowns reflect add/delete/edit done here.
    sourceStore.fetchSources(true).catch(() => undefined)
  }
}

/** 编辑器内容与最新数据分叉时的统一提醒(每次会话只提示一次, 不打断连续操作)。
 *
 *  定义在 `syncEditingSource`/`toggleSource` 之前: 这两处会在用户操作时调用它,
 *  而模块级 `let` 存在 TDZ, 声明顺序靠后会留下隐患。 */
let editorStaleWarned = false
function warnEditorStale() {
  if (editorStaleWarned) return
  editorStaleWarned = true
  appStore.showToast('编辑框内是打开时的旧内容，直接保存会覆盖刚做的改动；可重新打开该源', 'warning')
}

/**
 * 列表刷新后让编辑面板跟上最新数据。
 *
 * 菜单操作(启用/停用、发现开关、置顶/沉底等)会改动正在编辑的那个源:
 * 置顶/沉底走 `loadSources()` 重建数组(toggleSource 则是 `Object.assign`
 * 就地改对象), 两种路径都会让面板停在旧内容上 —— 概览显示旧值, 用户随后
 * 点「保存」还会把刚做的改动覆盖回去。
 *
 * 判据必须用**打开编辑时记下的文本**(editorBaseline), 不能拿当前对象重新
 * 序列化: `toggleSource` 就地改的正是 editingSource 指向的那个对象, 重新
 * 序列化会得到"已更新"的结果, 于是把用户的编辑误判成没改过而直接丢弃。
 *
 * 比较按解析后的语义做, 这样「格式化」(只改缩进) 后仍算无改动。
 */
function syncEditingSource() {
  const current = editingSource.value
  if (!current) return
  const latest = sources.value.find((item) => item.bookSourceUrl === current.bookSourceUrl)
  if (!latest) return
  if (isEditorUnchanged()) {
    editSource(latest)
    return
  }
  // 编辑器有未保存修改: 内容一律不覆盖。但若磁盘上的记录在这期间已经变了
  //（置顶/沉底/发现开关等菜单操作），用户随后点「保存」会把它整份写回、
  // 静默回退那次操作 —— 这种情况必须提示, 而不是默默放过。
  // 判据用「最新记录 vs 打开时的快照」的语义比较: 没变就不提示, 避免普通刷新噪声。
  if (!isEditorTextUnchanged(JSON.stringify(latest, null, 2), editorBaseline.value)) {
    warnEditorStale()
  }
}

/** 编辑器内容是否仍是载入时那一份(忽略缩进等格式差异)。 */
function isEditorUnchanged() {
  return isEditorTextUnchanged(editorText.value, editorBaseline.value)
}

async function toggleSource(source: BookSource) {
  const next = { ...source, enabled: source.enabled === false ? true : false }
  try {
    await saveBookSource(next)
    Object.assign(source, next)
    await sourceStore.fetchSources(true)
    // 就地改的是 sources 里的同一个对象, 不会经过 loadSources, 得在这里同步
    // 编辑面板: 否则编辑器文本还带着旧的 enabled, 用户随后点「保存」会把这次
    // 停用/启用静默回退。
    if (editingSource.value?.bookSourceUrl === source.bookSourceUrl) {
      if (isEditorUnchanged()) {
        editorText.value = JSON.stringify(source, null, 2)
        editorBaseline.value = editorText.value
      } else {
        // 编辑器有未保存修改: 不能覆盖用户内容, 但也不能让这次菜单操作被
        // 随后的「保存」静默回退 —— 必须让用户知道编辑器里的是旧快照。
        warnEditorStale()
      }
    }
    appStore.showToast('书源状态已更新', 'success')
  } catch (e: unknown) {
    appStore.showToast((e as Error).message, 'error')
  }
}

async function pinSource(source: BookSource) {
  try {
    await pinBookSource(source.bookSourceUrl)
    appStore.showToast(`已置顶「${source.bookSourceName}」`, 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message, 'error')
  }
}

async function unpinSource(source: BookSource) {
  try {
    await unpinBookSource(source.bookSourceUrl)
    appStore.showToast(`已取消置顶「${source.bookSourceName}」`, 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message, 'error')
  }
}

async function removeSource(source: BookSource) {
  const ok = await appStore.confirmDialog(`确定删除书源 "${source.bookSourceName}"？`, { title: '删除书源', danger: true })
  if (!ok) return
  try {
    await deleteBookSource(source.bookSourceUrl)
    sources.value = sources.value.filter((s) => s.bookSourceUrl !== source.bookSourceUrl)
    selectedSourceUrls.value = new Set(
      Array.from(selectedSourceUrls.value).filter((url) => url !== source.bookSourceUrl)
    )
    if (editingSource.value?.bookSourceUrl === source.bookSourceUrl) {
      createSource()
    }
    appStore.showToast('已删除', 'success')
    await sourceStore.fetchSources(true).catch(() => undefined)
  } catch (e: unknown) {
    appStore.showToast((e as Error).message, 'error')
  }
}

function handleSourceContextMenu({ source, event }: { source: BookSource; event: MouseEvent }) {
  showContextMenu(event, buildSourceMenuItems(source), source)
}

/** 编辑面板的「更多」: 对当前正在编辑的书源弹出与右键一致的操作菜单。 */
function handleEditorMore(event: MouseEvent) {
  if (!editingSource.value) return
  showContextMenu(event, buildSourceMenuItems(editingSource.value, { includeEdit: false }), editingSource.value)
}

/**
 * 单个书源的完整操作菜单(列表右键与编辑面板「更多」共用一套)。
 * 参考 legado 单源菜单 sourceMenuActions: 置顶/置底、启用/停用、启用发现、
 * 测试、导出、分组、删除。
 *
 * includeEdit 为 false 时省略「编辑」项: 编辑面板里本就停在编辑态, 那一项
 * 只会把编辑器内容重载回磁盘版本, 静默丢掉未保存的修改。
 */
function buildSourceMenuItems(source: BookSource, opts: { includeEdit?: boolean } = {}): MenuItem[] {
  const pinned = typeof source.customOrder === 'number' && source.customOrder < 0
  const enabled = source.enabled !== false
  const exploreEnabled = source.enabledExplore === true
  const items: MenuItem[] = []
  if (opts.includeEdit !== false) {
    items.push({ label: '编辑', action: () => editSource(source) })
    items.push({ divider: true })
  }
  items.push(
    { label: enabled ? '停用' : '启用', action: () => toggleSource(source) },
    {
      label: exploreEnabled ? '停用发现' : '启用发现',
      action: () => setSourceExploreEnabled(source, !exploreEnabled),
    },
    { divider: true },
    {
      label: pinned ? '取消置顶' : '置顶',
      action: () => (pinned ? unpinSource(source) : pinSource(source)),
    },
    { label: '沉底', action: () => sinkSourceToBottom(source) },
    { divider: true },
    { label: '添加到分组…', action: () => openGroupPrompt('add', [source]) },
    { label: '从分组移除…', action: () => openGroupPrompt('remove', [source]) },
    { divider: true },
    { label: '导出此书源', action: () => exportSourcesList([source]) },
    {
      label: '测试此书源',
      action: () => testSources([source]),
      // 用 getter 而非快照值: 菜单项存于 reactive 的 `state.items`, 模板渲染时
      // 读取该属性即会追踪 `testingSources` —— 后台测试跑完后菜单里的禁用态
      // 会立即解除, 不必关掉菜单再打开。
      get disabled() { return testingSources.value },
    },
    { divider: true },
    { label: '删除', danger: true, action: () => removeSource(source) },
  )
  return items
}

async function removeSelectedSources() {
  // Snapshot the visible selection before opening the asynchronous confirm
  // dialog so the deletion target cannot drift with later filter changes.
  const targets = [...selectedFilteredSources.value]
  if (!targets.length) {
    appStore.showToast('请先选择要删除的书源', 'warning')
    return
  }
  const ok = await appStore.confirmDialog(`确定删除选中的 ${targets.length} 个书源？`, { title: '删除书源(批量)', danger: true })
  if (!ok) return
  try {
    await deleteBookSources(toBookSourceDeletePayload(targets))
    const targetUrls = new Set(targets.map((source) => source.bookSourceUrl))
    sources.value = sources.value.filter((source) => !targetUrls.has(source.bookSourceUrl))
    selectedSourceUrls.value = new Set(
      Array.from(selectedSourceUrls.value).filter((url) => !targetUrls.has(url))
    )
    if (editingSource.value && targetUrls.has(editingSource.value.bookSourceUrl)) {
      createSource()
    }
    appStore.showToast(`已删除 ${targets.length} 个书源`, 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '批量删除失败', 'error')
  }
}

/**
 * "更多"菜单(参考 legado book_source_sel 多选操作菜单):
 * 启用/停用、分组增删、发现开关、置顶/沉底、导出/测试所选。
 * 分享与校验间隔两项为 Android 特有语义, 桌面端由导出替代、间隔无数据模型, 不做。
 */
function handleSelectionMore(event: MouseEvent) {
  const count = selectedFilteredSources.value.length
  if (!count) {
    appStore.showToast('请先选择要操作的书源', 'warning')
    return
  }
  showContextMenu(event, [
    { label: `启用选中（${count}）`, action: () => setSelectedEnabled(true) },
    { label: `停用选中（${count}）`, action: () => setSelectedEnabled(false) },
    { divider: true },
    { label: '添加到分组…', action: () => openGroupPrompt('add') },
    { label: '从分组移除…', action: () => openGroupPrompt('remove') },
    { divider: true },
    { label: '启用发现', action: () => setSelectedExploreEnabled(true) },
    { label: '停用发现', action: () => setSelectedExploreEnabled(false) },
    { divider: true },
    { label: '置顶所选', action: () => moveSelectedSources('top') },
    { label: '沉底所选', action: () => moveSelectedSources('bottom') },
    { divider: true },
    { label: '导出所选', action: () => exportSelectedSources() },
    {
      label: '测试所选',
      action: () => testSources(),
      disabled: testingSources.value,
    },
  ], null, { width: 240 })
}

/** 批量操作通用流程: 快照选中项 → 逐个变形 → 一次批量保存 → 刷新。 */
async function applySelectedSources(
  update: (source: BookSource) => BookSource,
  successMessage: (count: number) => string,
  explicitTargets?: BookSource[],
) {
  // 与 removeSelectedSources 相同的快照原则: 异步等待期间筛选可能变化。
  // explicitTargets 用于单源菜单等"目标已确定"的入口。
  const targets = explicitTargets?.length ? [...explicitTargets] : [...selectedFilteredSources.value]
  if (!targets.length) {
    appStore.showToast('请先选择要操作的书源', 'warning')
    return
  }
  try {
    await saveBookSources(targets.map(update))
    appStore.showToast(successMessage(targets.length), 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '批量操作失败', 'error')
  }
}

function setSelectedEnabled(enabled: boolean) {
  return applySelectedSources(
    (source) => ({ ...source, enabled }),
    (count) => (enabled ? `已启用 ${count} 个书源` : `已停用 ${count} 个书源`),
  )
}

function setSelectedExploreEnabled(enabled: boolean) {
  return applySelectedSources(
    (source) => ({ ...source, enabledExplore: enabled }),
    (count) => (enabled ? `已启用 ${count} 个书源的发现` : `已停用 ${count} 个书源的发现`),
  )
}

async function moveSelectedSources(edge: 'top' | 'bottom') {
  const targets = [...selectedFilteredSources.value]
  if (!targets.length) {
    appStore.showToast('请先选择要操作的书源', 'warning')
    return
  }
  try {
    const targetUrls = new Set(targets.map((source) => source.bookSourceUrl))
    const updated = moveBookSourcesToEdge(sources.value, targetUrls, edge)
    // 选中项的 customOrder 必被改写(top 恒为负/bottom 恒为正, 必异于任何原值),
    // 因此按选中集合过滤出的就是需要写回的全部变更。
    const changed = updated.filter((source) => targetUrls.has(source.bookSourceUrl))
    await saveBookSources(changed)
    appStore.showToast(
      edge === 'top' ? `已置顶 ${targets.length} 个书源` : `已沉底 ${targets.length} 个书源`,
      'success',
    )
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '批量操作失败', 'error')
  }
}

/** 单源沉底: 复用批量的排序计算, 但提示语按单源措辞。置顶走后端 pinSource(数值等价)。 */
async function sinkSourceToBottom(source: BookSource) {
  const targetUrls = new Set([source.bookSourceUrl])
  try {
    const updated = moveBookSourcesToEdge(sources.value, targetUrls, 'bottom')
    const changed = updated.filter((item) => targetUrls.has(item.bookSourceUrl))
    await saveBookSources(changed)
    appStore.showToast(`已沉底「${source.bookSourceName}」`, 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '操作失败', 'error')
  }
}

function setSourceExploreEnabled(source: BookSource, enabled: boolean) {
  return applySelectedSources(
    (item) => ({ ...item, enabledExplore: enabled }),
    () => (enabled ? `已启用「${source.bookSourceName}」的发现` : `已停用「${source.bookSourceName}」的发现`),
    [source],
  )
}

function openGroupPrompt(mode: 'add' | 'remove', explicitTargets?: BookSource[]) {
  const count = explicitTargets?.length || selectedFilteredSources.value.length
  if (!count) {
    appStore.showToast('请先选择要操作的书源', 'warning')
    return
  }
  groupPromptTargets.value = explicitTargets ? [...explicitTargets] : []
  // 与 targets 同批快照: 关闭动画期间 targets 会被清空, 计数不能依赖现算。
  groupPromptFallbackCount.value = selectedFilteredSources.value.length
  groupPromptMode.value = mode
  groupPromptVisible.value = true
}

function closeGroupPrompt() {
  groupPromptVisible.value = false
  groupPromptTargets.value = []
}

async function handleGroupPromptConfirm(groupsInput: string) {
  const mode = groupPromptMode.value
  const targets = groupPromptTargets.value
  closeGroupPrompt()
  await applySelectedSources(
    (source) => (mode === 'add' ? addBookSourceGroups(source, groupsInput) : removeBookSourceGroups(source, groupsInput)),
    (count) => (mode === 'add' ? `已将 ${count} 个书源加入分组` : `已将 ${count} 个书源移出分组`),
    targets.length ? targets : undefined,
  )
}

async function testSources(explicitTargets?: BookSource[]) {
  const targets = explicitTargets?.length
    ? explicitTargets
    : (selectedFilteredSources.value.length ? selectedFilteredSources.value : sources.value)
  if (!targets.length) {
    appStore.showToast('没有可测试的书源', 'warning')
    return
  }
  // 单源入口与批量入口的措辞分开: 前者点名书源, 后者说数量。
  const single = explicitTargets?.length === 1 ? explicitTargets[0] : null
  const scopeText = single
    ? `「${single.bookSourceName}」这 1`
    : (selectedFilteredSources.value.length ? `选中的 ${targets.length}` : `全部 ${targets.length}`)
  const ok = await appStore.confirmDialog(`将测试${scopeText} 个书源。测试会请求外部站点，耗时可能较长，是否继续？`, { title: '测试书源' })
  if (!ok) return

  testingSources.value = true
  sourceTestStopRequested.value = false
  sourceTestTaskId.value = `source-test-${Date.now()}-${Math.random().toString(36).slice(2)}`
  sourceTestProgress.value = { completed: 0, total: targets.length }
  try {
    let completedBeforeBatch = 0
    stopSourceTestProgressListener = await listen<{
      taskId: string
      total: number
      completed: number
      cancelled: boolean
    }>('book-source-test-progress', ({ payload }) => {
      if (payload.taskId !== sourceTestTaskId.value) return
      sourceTestProgress.value.completed = Math.min(
        sourceTestProgress.value.total,
        completedBeforeBatch + payload.completed,
      )
    })
    const batches = chunkBookSourceUrls(targets.map((source) => source.bookSourceUrl))
    const responses = []
    for (const batch of batches) {
      if (sourceTestStopRequested.value) break
      responses.push(
        await testBookSources({
          bookSourceUrls: batch,
          markInvalid: true,
          // 与后端 clamp 后的上限一致(8): 设更高只会被截断, 反而误导。
          concurrent: 8,
          taskId: sourceTestTaskId.value,
        })
      )
      completedBeforeBatch += responses.at(-1)?.total || 0
      sourceTestProgress.value.completed = Math.min(targets.length, completedBeforeBatch)
      if (responses.at(-1)?.cancelled) break
    }
    const result = mergeBookSourceTestResponses(responses)
    await loadSources()
    if (result.invalid > 0 && !result.cancelled) {
      filterGroup.value = '失效'
    }
    if (sourceTestStopRequested.value || result.cancelled) {
      // 如实区分三种数: 已完成(有效/失效)与未检测的。不能把未完成的算成"失效"。
      //
      // 未检测数必须用 `targets.length - valid - invalid`:
      // 后端 `valid + invalid` 恰好等于"真正完成检测的源数"(被取消的不计入),
      // 所以这个差集天然覆盖「从未派发」「派发了但结果被丢弃」「已返回已取消」
      // 三种情况。不能用 `cancelledCount`(只含最后一种) 或 `total`(含已取消条目),
      // 否则 100 个源只测 5 个就中止时会显示成"2 个未检测"。
      const undone = Math.max(0, targets.length - result.valid - result.invalid)
      const parts = [`测试已中止`, `有效 ${result.valid}`, `失效 ${result.invalid}`]
      if (undone > 0) parts.push(`${undone} 个未检测`)
      appStore.showToast(parts.join('，'), 'warning')
    } else {
      appStore.showToast(
        `测试完成：有效 ${result.valid} 个，失效 ${result.invalid} 个，更新分组 ${result.markedInvalid} 个`,
        result.invalid > 0 ? 'warning' : 'success'
      )
    }
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '书源测试失败', 'error')
  } finally {
    stopSourceTestProgressListener?.()
    stopSourceTestProgressListener = null
    sourceTestTaskId.value = ''
    testingSources.value = false
  }
}

async function cancelSourceTest() {
  if (!testingSources.value || sourceTestStopRequested.value) return
  sourceTestStopRequested.value = true
  const taskId = sourceTestTaskId.value
  if (taskId) {
    // 不吞掉失败: 取消命令没送到会让后端继续检测, 必须留下线索而不是静默。
    await cancelBookSourceTest(taskId).catch((err) => {
      console.warn('[source-test] 取消请求失败', err)
    })
  }
}

onBeforeUnmount(() => {
  stopSourceTestProgressListener?.()
  if (sourceTestTaskId.value) {
    cancelBookSourceTest(sourceTestTaskId.value).catch(() => undefined)
  }
})

async function removeInvalidSources() {
  if (!invalidSources.value.length) {
    appStore.showToast('没有失效书源', 'warning')
    return
  }
  const ok = await appStore.confirmDialog(`确定删除 ${invalidSources.value.length} 个失效书源？此操作不可撤销。`, { title: '删除失效书源', danger: true })
  if (!ok) return
  try {
    const result = await deleteInvalidBookSources()
    const invalidUrls = new Set(invalidSources.value.map((source) => source.bookSourceUrl))
    sources.value = sources.value.filter((source) => !invalidUrls.has(source.bookSourceUrl))
    selectedSourceUrls.value.clear()
    if (editingSource.value && invalidUrls.has(editingSource.value.bookSourceUrl)) {
      createSource()
    }
    appStore.showToast(`已删除 ${result.deleted} 个失效书源`, 'success')
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '删除失效书源失败', 'error')
  }
}

function toggleSourceSelection(source: BookSource) {
  const next = new Set(selectedSourceUrls.value)
  if (next.has(source.bookSourceUrl)) {
    next.delete(source.bookSourceUrl)
  } else {
    next.add(source.bookSourceUrl)
  }
  selectedSourceUrls.value = next
}

function toggleFilteredSelection() {
  const next = new Set(selectedSourceUrls.value)
  if (allFilteredSelected.value) {
    filteredSources.value.forEach((source) => next.delete(source.bookSourceUrl))
  } else {
    filteredSources.value.forEach((source) => next.add(source.bookSourceUrl))
  }
  selectedSourceUrls.value = next
}

function pruneSelection() {
  const availableUrls = new Set(sources.value.map((source) => source.bookSourceUrl))
  selectedSourceUrls.value = new Set(
    Array.from(selectedSourceUrls.value).filter((url) => availableUrls.has(url))
  )
}

function hasSourceGroup(source: BookSource, groupName: string) {
  return (source.bookSourceGroup || '')
    .split(/[,;；、]/)
    .map((group) => group.trim())
    .filter(Boolean)
    .includes(groupName)
}

function createSource() {
  editingSource.value = null
  editorText.value = JSON.stringify(createEmptySource(), null, 2)
  editorBaseline.value = editorText.value
  resetSignal.value += 1
}

function editSource(source: BookSource) {
  editingSource.value = source
  editorText.value = JSON.stringify(source, null, 2)
  editorBaseline.value = editorText.value
}

function formatEditor() {
  try {
    const parsed = JSON.parse(editorText.value)
    editorText.value = JSON.stringify(parsed, null, 2)
  } catch {
    appStore.showToast('JSON 格式错误，无法格式化', 'error')
  }
}

async function saveEditor() {
  try {
    const raw = JSON.parse(editorText.value)
    const parsed = (Array.isArray(raw) ? raw : [raw]) as BookSource[]
    if (!parsed.length) {
      throw new Error('书源内容为空')
    }
    for (const source of parsed) {
      if (!source.bookSourceName?.trim()) {
        throw new Error('书源名称不能为空')
      }
      if (!source.bookSourceUrl?.trim()) {
        throw new Error('书源链接不能为空')
      }
    }
    await saveBookSources(parsed)
    appStore.showToast(`保存书源成功（${parsed.length} 个）`, 'success')
    await loadSources()
    const latest = sources.value.find((item) => item.bookSourceUrl === parsed[0].bookSourceUrl)
    if (latest) {
      editSource(latest)
    }
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '保存失败', 'error')
  }
}

async function handleSourceLogin() {
  try {
    const parsed = JSON.parse(editorText.value) as BookSource
    if (!parsed.bookSourceUrl?.trim() || !parsed.bookSourceName?.trim()) {
      throw new Error('请填写书源名称和地址')
    }
    sourceLoginLoading.value = true
    await saveBookSource(parsed)
    await loadSources()
    if (!props.modelValue) return
    loginSource.value = parsed
    loginPreviewVisible.value = true
  } catch (error: unknown) {
    appStore.showToast(error instanceof Error ? error.message : '打开登录失败', 'error')
  } finally {
    sourceLoginLoading.value = false
  }
}

function triggerFileImport() {
  fileInputRef.value?.click()
}

async function handleFileImport(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    const imported = await readSourceFile(file)
    if (!imported.length) {
      throw new Error('文件中没有可导入的书源')
    }
    await saveBookSources(imported)
    appStore.showToast(`成功导入 ${imported.length} 个书源`, 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '导入失败', 'error')
  } finally {
    input.value = ''
  }
}

async function importRemoteSource() {
  if (!remoteUrl.value) {
    appStore.showToast('请输入远程书源链接', 'warning')
    return
  }
  // 同步期间锁定入口: 转圈只是反馈, 还得防住重复点击造成的并发拉取与重复入库。
  if (syncingSubscriptionUrl.value) return
  const targetUrl = remoteUrl.value
  syncingSubscriptionUrl.value = targetUrl
  try {
    const raw = await readRemoteSourceFile(targetUrl)
    const parsed = raw.flatMap((item) => {
      try {
        const value = JSON.parse(item)
        return Array.isArray(value) ? value : [value]
      } catch {
        return []
      }
    }) as BookSource[]
    if (!parsed.length) {
      throw new Error('远程书源文件为空或格式错误')
    }
    await saveBookSources(parsed)
    touchSubscription(targetUrl)
    appStore.showToast(`成功导入 ${parsed.length} 个书源`, 'success')
    await loadSources()
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '网络导入失败', 'error')
  } finally {
    syncingSubscriptionUrl.value = ''
  }
}

function saveSubscription() {
  if (!remoteUrl.value) {
    appStore.showToast('请输入远程书源链接', 'warning')
    return
  }
  if (!subscriptions.value.find((item) => item.url === remoteUrl.value)) {
    subscriptions.value.unshift({ url: remoteUrl.value })
    persistSubscriptions()
    appStore.showToast('导入链接已保存', 'success')
  }
}

async function syncSubscription(url: string) {
  remoteUrl.value = url
  await importRemoteSource()
}

function removeSubscription(url: string) {
  subscriptions.value = subscriptions.value.filter((item) => item.url !== url)
  persistSubscriptions()
}

function touchSubscription(url: string) {
  const existing = subscriptions.value.find((item) => item.url === url)
  if (existing) {
    existing.lastSyncedAt = Date.now()
  } else {
    subscriptions.value.unshift({ url, lastSyncedAt: Date.now() })
  }
  persistSubscriptions()
}

async function exportSourcesList(list: BookSource[]) {
  if (isTauriRuntime()) {
    try {
      const result = await exportBookSources(list)
      if (result.cancelled) return
      appStore.showToast('书源已导出', 'success')
      return
    } catch (error) {
      appStore.showToast((error as Error).message || '导出失败', 'error')
      return
    }
  }
  const payload = JSON.stringify(list, null, 2)
  const blob = new Blob([payload], { type: 'application/json;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `reader-book-sources-${formatDateForFile()}.json`
  anchor.click()
  URL.revokeObjectURL(url)
}

function exportSources() {
  return exportSourcesList(sources.value)
}

function exportSelectedSources() {
  const targets = [...selectedFilteredSources.value]
  if (!targets.length) {
    appStore.showToast('请先选择要导出的书源', 'warning')
    return
  }
  return exportSourcesList(targets)
}

function formatDateForFile() {
  const date = new Date()
  const pad = (v: number) => `${v}`.padStart(2, '0')
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`
}

function close() {
  emit('update:modelValue', false)
}

watch(() => props.modelValue, (v) => {
  if (v) {
    if (sources.value.length === 0) {
      loadSources()
    }
    if (!editingSource.value && !editorText.value.trim()) {
      createSource()
    }
  }
}, { immediate: true })
</script>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  z-index: var(--z-overlay);
  backdrop-filter: blur(4px);
}

.modal-container {
  position: fixed;
  inset: 0;
  z-index: var(--z-modal);
  display: flex;
  align-items: center;
  justify-content: center;
  padding:
    calc(var(--space-6) + var(--safe-area-top))
    calc(var(--space-6) + var(--safe-area-right))
    calc(var(--space-6) + var(--safe-area-bottom))
    calc(var(--space-6) + var(--safe-area-left));
}

.source-modal {
  width: min(1180px, 100%);
  height: min(780px, calc(var(--app-height, 100dvh) - var(--safe-area-top) - var(--safe-area-bottom) - 32px));
  max-height: min(88vh, calc(var(--app-height, 100dvh) - var(--safe-area-top) - var(--safe-area-bottom) - 32px));
  background: var(--color-bg-elevated);
  border-radius: var(--radius-xl);
  display: flex;
  flex-direction: column;
  box-shadow: var(--shadow-xl);
  overflow: hidden;
}

.hidden-input {
  display: none;
}

.content-grid {
  display: grid;
  grid-template-columns: minmax(320px, 42%) minmax(420px, 58%);
  gap: 14px;
  min-height: 0;
  flex: 1;
  padding: 14px 24px 0;
  overflow: hidden;
}

.source-list-slot,
.editor-slot {
  min-height: 0;
}

.modal-footer {
  min-height: 44px;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 12px;
  padding: 8px 24px 14px;
  flex-shrink: 0;
}

.count-info {
  font-size: 12px;
  color: var(--color-text-tertiary);
}

.mini-btn,
.icon-btn {
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: transparent;
  transition: all var(--duration-fast);
}

.mini-btn {
  min-height: 32px;
  padding: 0 10px;
  font-size: 12px;
  white-space: nowrap;
}

.mini-btn.danger {
  color: var(--color-danger);
  border-color: rgba(245, 34, 45, 0.2);
}

.icon-btn {
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--color-text-secondary);
}

.icon-btn svg {
  width: 18px;
  height: 18px;
}

.mini-btn:hover,
.icon-btn:hover {
  background: var(--color-bg-hover);
}

@media (max-width: 900px) {
  .modal-container {
    align-items: stretch;
    padding: 8px;
  }

  .source-modal {
    width: 100%;
    height: calc(var(--app-height, 100dvh) - 16px);
    max-height: calc(var(--app-height, 100dvh) - 16px);
    border-radius: var(--radius-xl);
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .content-grid {
    display: flex;
    flex-direction: column;
    overflow: visible;
    flex: none;
    padding-left: 16px;
    padding-right: 16px;
  }

  .editor-slot {
    order: -1;
    min-height: 360px;
    max-height: min(58vh, 560px);
  }

  .source-list-slot {
    max-height: min(46vh, 460px);
  }

  .modal-footer {
    padding: 10px 16px 14px;
    background: var(--color-bg-elevated);
    border-top: 1px solid var(--color-border-light);
  }
}

@media (max-width: 420px) {
  .modal-container {
    padding:
      calc(8px + var(--safe-area-top))
      calc(8px + var(--safe-area-right))
      calc(8px + var(--safe-area-bottom))
      calc(8px + var(--safe-area-left));
  }

  .source-modal {
    border-radius: var(--radius-lg);
  }
}
</style>
