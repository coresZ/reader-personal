<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="visible" class="group-prompt-overlay" @click.self="$emit('close')">
        <div class="group-prompt">
          <h3 class="prompt-title">{{ title }}</h3>
          <p class="prompt-message">{{ message }}</p>
          <input
            ref="inputRef"
            v-model="value"
            class="prompt-input"
            type="text"
            :placeholder="placeholder"
            list="source-group-suggestions"
            @keydown.enter.prevent="confirm"
            @keydown.esc.prevent="$emit('close')"
          />
          <datalist id="source-group-suggestions">
            <option v-for="group in suggestions" :key="group" :value="group" />
          </datalist>
          <div v-if="suggestions.length" class="prompt-suggestions">
            <button
              v-for="group in suggestions"
              :key="group"
              class="suggestion-chip"
              :class="{ active: selectedNames.includes(group) }"
              type="button"
              @click="toggleSuggestion(group)"
            >
              {{ group }}
            </button>
          </div>
          <p class="prompt-hint">多个分组用逗号分隔，如：玄幻,都市</p>
          <div class="prompt-actions">
            <button class="prompt-btn" type="button" @click="$emit('close')">取消</button>
            <button class="prompt-btn primary" type="button" :disabled="!hasInput" @click="confirm">
              {{ confirmText }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'

const props = withDefaults(
  defineProps<{
    visible: boolean
    title: string
    message?: string
    placeholder?: string
    confirmText?: string
    suggestions?: string[]
  }>(),
  {
    message: '',
    placeholder: '分组名',
    confirmText: '确定',
    suggestions: () => [],
  },
)

const emit = defineEmits<{
  confirm: [value: string]
  close: []
}>()

const value = ref('')
const inputRef = ref<HTMLInputElement | null>(null)

const selectedNames = computed(() =>
  value.value
    .split(/[,;；、]/)
    .map((name) => name.trim())
    .filter(Boolean),
)

const hasInput = computed(() => selectedNames.value.length > 0)

watch(
  () => props.visible,
  async (visible) => {
    if (visible) {
      value.value = ''
      await nextTick()
      inputRef.value?.focus()
    }
  },
)

function toggleSuggestion(group: string) {
  const names = new Set(selectedNames.value)
  if (names.has(group)) {
    names.delete(group)
  } else {
    names.add(group)
  }
  value.value = Array.from(names).join(',')
}

function confirm() {
  if (!hasInput.value) return
  emit('confirm', value.value)
}
</script>

<style scoped>
.group-prompt-overlay {
  position: fixed;
  inset: 0;
  /* 与 ConfirmDialog 同级: 盖在书源管理弹窗(z-modal)之上, 不与 ContextMenu 竞争。 */
  z-index: calc(var(--z-modal) + 10);
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(2px);
  padding: 24px;
}

.group-prompt {
  width: min(380px, 100%);
  background: var(--color-bg-elevated);
  border-radius: var(--radius-lg);
  padding: 20px;
  box-shadow: var(--shadow-xl);
}

.prompt-title {
  margin: 0 0 8px;
  font-size: 15px;
  font-weight: 700;
}

.prompt-message {
  margin: 0 0 14px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--color-text-secondary);
  word-break: break-all;
}

.prompt-input {
  width: 100%;
  min-height: 38px;
  padding: 0 12px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-bg);
  color: inherit;
  font-size: 13px;
}

.prompt-input:focus {
  outline: none;
  border-color: var(--color-primary);
}

.prompt-suggestions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 12px;
}

.suggestion-chip {
  min-height: 28px;
  padding: 0 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-full);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 12px;
  transition: all var(--duration-fast);
}

.suggestion-chip:hover {
  background: var(--color-bg-hover);
}

.suggestion-chip.active {
  background: var(--color-primary-soft-bg);
  border-color: var(--color-primary-soft-border);
  color: var(--color-primary-soft-text);
}

.prompt-hint {
  margin: 10px 0 0;
  font-size: 12px;
  color: var(--color-text-tertiary);
}

.prompt-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 18px;
}

.prompt-btn {
  min-height: 34px;
  padding: 0 16px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 13px;
  cursor: pointer;
}

.prompt-btn.primary {
  background: var(--color-primary-soft-bg);
  border-color: var(--color-primary-soft-border);
  color: var(--color-primary-soft-text);
}

.prompt-btn:disabled {
  cursor: not-allowed;
  opacity: 0.52;
}

.prompt-btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
}

.prompt-btn.primary:hover:not(:disabled) {
  background: var(--color-primary-soft-bg-hover);
  border-color: var(--color-primary);
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.15s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
