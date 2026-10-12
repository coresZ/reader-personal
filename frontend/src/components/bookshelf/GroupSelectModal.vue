<template>
  <div v-if="modelValue" class="modal-overlay" @click="$emit('update:modelValue', false)">
    <div class="modal-content" @click.stop>
      <div class="modal-header">
        <h3>{{ (bookCount ?? 0) > 0 ? `调整分组（${bookCount} 本）` : '调整分组' }}</h3>
        <button class="close-btn" @click="$emit('update:modelValue', false)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12" /></svg>
        </button>
      </div>

      <div class="modal-body">
        <div class="group-list">
          <label
            v-for="group in shelfStore.groups"
            :key="group.groupId"
            class="group-item"
            :class="{ checked: checkedIds.has(group.groupId) }"
          >
            <input
              type="checkbox"
              class="group-checkbox"
              :checked="checkedIds.has(group.groupId)"
              @change="toggleCheck(group.groupId)"
            />
            <span class="group-name">{{ group.groupName }}</span>
          </label>
          <p v-if="!shelfStore.groups.length" class="empty-hint">还没有分组，先在下方新建一个吧</p>

          <div class="new-group">
            <input
              v-model="newGroupName"
              placeholder="新建分组名称..."
              class="group-input"
              @keyup.enter="handleCreate"
            />
            <button class="add-btn" @click="handleCreate" :disabled="!newGroupName.trim() || creating">
              新建并勾选
            </button>
          </div>
        </div>
      </div>

      <div class="modal-footer">
        <span class="footer-hint">{{ footerHint }}</span>
        <button class="confirm-btn" :disabled="!hasChanges || submitting" @click="handleConfirm">
          {{ submitting ? '保存中…' : '保存' }}
        </button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useBookshelfStore } from '../../stores/bookshelf'
import { useAppStore } from '../../stores/app'

const props = defineProps<{
  modelValue: boolean
  /** 本次调整的书; 为空数组表示批量(编辑模式多选)路径。 */
  books?: { group?: number }[]
  /** 批量路径的书数(用于标题展示)。 */
  bookCount?: number
  /** 落盘进行中(由调用方控制): 期间禁用保存按钮, 防重复提交。
   *  失败后调用方须复位, 否则用户无法在弹窗内重试。 */
  submitting?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  /** 确认时把「勾选状态的位掩码」交给调用方。 */
  confirm: [groupMask: number]
}>()

const shelfStore = useBookshelfStore()
const appStore = useAppStore()
const newGroupName = ref('')
const creating = ref(false)
/** 勾选集合: 打开时从书的现有归属初始化, 确认时整体落盘。 */
const checkedIds = ref<Set<number>>(new Set())
/** 打开时的快照, 用于「无变化禁用保存」与差量提示。 */
const initialMask = ref(0)

const isBatch = computed(() => (props.bookCount ?? 0) > 1)

/** 批量是整体覆盖语义(所有选中书的分组统一设为勾选项), 必须明示 ——
 *  否则「加入/移出」的措辞会被理解成逐本叠加, 静默抹掉书与书之间不同的归属。 */
const footerHint = computed(() => (
  isBatch.value
    ? `保存后 ${props.bookCount} 本书的分组将统一设为勾选项`
    : '勾选即加入分组，取消勾选即移出'
))

const hasChanges = computed(() => currentMask.value !== initialMask.value)

const currentMask = computed(() => {
  let mask = 0
  for (const id of checkedIds.value) mask |= id
  return mask
})

/** 弹窗每次打开都按目标书的位域重建勾选状态。
 *
 *  批量路径取**交集**（逐本 AND）—— 只预勾选所有选中书的共同归属, 差异项
 *  交给用户决定; 单本书时交集就是它自己的完整归属。
 *
 *  取交集而不是并集, 是因为保存是整体覆盖语义: 预勾选并集的话, 只要有一本
 *  书属于某分组就会勾上, 一保存就把它塞给了所有书。 */
watch(() => props.modelValue, (visible) => {
  if (!visible) return
  const targets = props.books ?? []
  let mask = -1 // 全 1 起步, 逐本 AND 出交集
  for (const book of targets) {
    mask &= book.group ?? 0
  }
  // 没有目标书(理论上不可达)时不给任何预勾选, 避免 -1 被当成「全选」。
  if (targets.length === 0) mask = 0
  initialMask.value = mask
  checkedIds.value = new Set(
    shelfStore.groups
      .filter((group) => group.groupId > 0 && (mask & group.groupId) !== 0)
      .map((group) => group.groupId),
  )
  newGroupName.value = ''
}, { immediate: true })

function toggleCheck(groupId: number) {
  const next = new Set(checkedIds.value)
  if (next.has(groupId)) next.delete(groupId)
  else next.add(groupId)
  checkedIds.value = next
}

async function handleCreate() {
  const name = newGroupName.value.trim()
  if (!name || creating.value) return
  creating.value = true
  try {
    const groupId = await shelfStore.saveGroup(name)
    if (groupId > 0) toggleCheck(groupId)
    newGroupName.value = ''
  } catch (e: unknown) {
    appStore.showToast((e as Error).message || '创建分组失败', 'error')
  } finally {
    creating.value = false
  }
}

function handleConfirm() {
  if (!hasChanges.value || props.submitting) return
  emit('confirm', currentMask.value)
}
</script>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding:
    calc(16px + var(--safe-area-top))
    calc(16px + var(--safe-area-right))
    calc(16px + var(--safe-area-bottom))
    calc(16px + var(--safe-area-left));
  z-index: 1000;
}

.modal-content {
  width: 100%;
  max-width: 360px;
  background: var(--color-bg-elevated);
  border-radius: var(--radius-2xl);
  box-shadow: var(--shadow-2xl);
  overflow: hidden;
  max-height: calc(var(--app-height, 100dvh) - var(--safe-area-top) - var(--safe-area-bottom) - 32px);
  display: flex;
  flex-direction: column;
}

.modal-header {
  padding: var(--space-4) var(--space-5);
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid var(--color-border-light);
}

.modal-header h3 { margin: 0; font-size: var(--text-lg); font-weight: 600; }

.close-btn {
  width: 32px; height: 32px;
  display: flex; align-items: center; justify-content: center;
  border-radius: var(--radius-md); color: var(--color-text-secondary);
  background: transparent; border: none; cursor: pointer;
}

.modal-body {
  padding: var(--space-2);
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
}

.group-list {
  display: flex; flex-direction: column; gap: 4px;
}

.group-item {
  display: flex; align-items: center; gap: var(--space-3);
  padding: var(--space-3) var(--space-4);
  border-radius: var(--radius-lg);
  cursor: pointer; text-align: left;
  transition: all 0.2s;
  user-select: none;
  -webkit-user-select: none;
}

.group-item:hover {
  background: var(--color-bg-hover);
}

.group-item.checked {
  background: var(--color-primary-soft-bg);
  color: var(--color-primary-soft-text);
}

.group-checkbox {
  width: 18px; height: 18px;
  accent-color: var(--color-primary);
  cursor: pointer;
  flex-shrink: 0;
}

.group-name { font-weight: 500; font-size: 15px; }

.empty-hint {
  margin: var(--space-3) var(--space-4);
  font-size: var(--text-sm);
  color: var(--color-text-tertiary);
}

.new-group {
  margin-top: var(--space-2);
  padding: var(--space-3);
  border-top: 1px solid var(--color-border-light);
  display: flex; gap: var(--space-2);
}

.group-input {
  flex: 1; padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-md); border: 1px solid var(--color-border);
  background: var(--color-bg-sunken); color: var(--color-text);
  font-size: var(--text-sm);
}

.add-btn {
  padding: 0 var(--space-3); border-radius: var(--radius-md);
  background: var(--color-primary-soft-bg); color: var(--color-primary-soft-text);
  border: 1px solid var(--color-primary-soft-border); font-size: var(--text-sm); font-weight: 500; cursor: pointer;
  white-space: nowrap;
}

.add-btn:disabled { opacity: 0.5; cursor: not-allowed; }

.add-btn:hover:not(:disabled) {
  background: var(--color-primary-soft-bg-hover);
  border-color: var(--color-primary);
}

.modal-footer {
  padding: var(--space-3) var(--space-5);
  border-top: 1px solid var(--color-border-light);
  display: flex; align-items: center; justify-content: space-between; gap: var(--space-3);
}

.footer-hint {
  font-size: var(--text-xs);
  color: var(--color-text-tertiary);
}

.confirm-btn {
  padding: var(--space-2) var(--space-4);
  border-radius: var(--radius-md);
  background: var(--color-primary);
  color: var(--color-primary-contrast, #fff);
  border: none; font-size: var(--text-sm); font-weight: 500; cursor: pointer;
}

.confirm-btn:disabled { opacity: 0.5; cursor: not-allowed; }

.confirm-btn:hover:not(:disabled) { filter: brightness(1.05); }
</style>
