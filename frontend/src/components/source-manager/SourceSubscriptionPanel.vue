<template>
  <Transition name="fade">
    <div v-if="visible" class="subscription-backdrop" @click.self="$emit('close')">
      <section class="subscription-panel">
        <header class="subscription-header">
          <div>
            <h3>网络导入</h3>
            <p>填一个书源合集链接即可导入；保存后可以随时一键重新导入更新。</p>
          </div>
          <button class="icon-btn" type="button" title="关闭" @click="$emit('close')">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </button>
        </header>

        <div class="remote-form">
          <input
            :value="remoteUrl"
            type="text"
            placeholder="输入远程书源合集链接"
            @input="$emit('update:remoteUrl', ($event.target as HTMLInputElement).value)"
          />
          <button class="action-btn primary" type="button" :disabled="syncing" @click="$emit('sync')">
            <span v-if="syncing" class="spinner" aria-hidden="true"></span>
            {{ syncing ? '导入中…' : '立即导入' }}
          </button>
          <button class="action-btn" type="button" @click="$emit('save')">保存链接</button>
        </div>

        <div v-if="subscriptions.length" class="subscription-list">
          <article v-for="item in subscriptions" :key="item.url" class="subscription-item">
            <div class="subscription-main">
              <span class="subscription-url">{{ item.url }}</span>
              <span v-if="item.lastSyncedAt" class="subscription-time">上次导入 {{ formatTime(item.lastSyncedAt) }}</span>
            </div>
            <div class="subscription-actions">
              <button
                class="mini-btn"
                type="button"
                :disabled="syncing"
                @click="$emit('sync-subscription', item.url)"
              >
                <span v-if="syncingUrl === item.url" class="spinner" aria-hidden="true"></span>
                {{ syncingUrl === item.url ? '导入中' : '重新导入' }}
              </button>
              <button class="mini-btn danger" type="button" :disabled="syncing" @click="$emit('remove-subscription', item.url)">删除</button>
            </div>
          </article>
        </div>
        <div v-else class="subscription-empty">还没有保存的网络导入记录。</div>
      </section>
    </div>
  </Transition>
</template>

<script setup lang="ts">
export type SourceSubscription = {
  url: string
  lastSyncedAt?: number
}

defineProps<{
  visible: boolean
  remoteUrl: string
  subscriptions: SourceSubscription[]
  /** 有任意同步在进行中(表单同步或某条订阅)。 */
  syncing?: boolean
  /** 正在同步的订阅链接, 用于只在那一条上转圈。 */
  syncingUrl?: string
}>()

defineEmits<{
  'update:remoteUrl': [value: string]
  sync: []
  save: []
  'sync-subscription': [url: string]
  'remove-subscription': [url: string]
  close: []
}>()

function formatTime(ts: number) {
  return new Date(ts).toLocaleString()
}
</script>

<style scoped>
.subscription-backdrop {
  position: fixed;
  inset: 0;
  z-index: calc(var(--z-modal) + 1);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.28);
}

.subscription-panel {
  width: min(720px, 100%);
  max-height: min(78vh, 720px);
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 20px;
  border: 1px solid var(--color-border-light);
  border-radius: var(--radius-xl);
  background: var(--color-bg-elevated);
  box-shadow: var(--shadow-xl);
  overflow: auto;
}

.subscription-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.subscription-header h3 {
  margin: 0;
  font-size: 17px;
}

.subscription-header p {
  margin-top: 6px;
  color: var(--color-text-tertiary);
  font-size: 13px;
}

.remote-form {
  display: flex;
  gap: 10px;
}

.remote-form input {
  flex: 1;
  min-width: 0;
  min-height: 40px;
  padding: 0 12px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-bg);
}

.subscription-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.subscription-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px;
  border: 1px solid var(--color-border-light);
  border-radius: var(--radius-lg);
  background: var(--color-bg);
}

.subscription-main {
  min-width: 0;
  display: flex;
  flex-direction: column;
}

.subscription-url {
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.subscription-time,
.subscription-empty {
  margin-top: 4px;
  color: var(--color-text-tertiary);
  font-size: 12px;
}

.subscription-actions {
  display: flex;
  gap: 8px;
}

.action-btn,
.mini-btn,
.icon-btn {
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: transparent;
  transition: all var(--duration-fast);
}

.action-btn {
  min-height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 0 12px;
  font-size: 13px;
  /* 与 SourceManagerHeader 同一原因: 不写就会被 global.css 里那份不限作用域的
   * `.action-btn` 的 600 命中, 按钮凭空变粗。 */
  font-weight: 400;
  white-space: nowrap;
}

.action-btn.primary {
  background: var(--color-primary-soft-bg);
  border-color: var(--color-primary-soft-border);
  color: var(--color-primary-soft-text);
}

.mini-btn {
  min-height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  padding: 0 10px;
  font-size: 12px;
}

.mini-btn.danger {
  color: var(--color-danger);
  border-color: rgba(245, 34, 45, 0.2);
}

/* 同步中的转圈: 与 .spinning 图标同一手法(旋转 animation), 尺寸随按钮字号走。 */
.spinner {
  flex-shrink: 0;
  width: 12px;
  height: 12px;
  border: 2px solid color-mix(in srgb, currentColor 30%, transparent);
  border-top-color: currentColor;
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.action-btn:disabled,
.mini-btn:disabled,
.icon-btn:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.icon-btn {
  width: 34px;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.icon-btn svg {
  width: 16px;
  height: 16px;
}

/* :not(:disabled) 让同步中的按钮不再有 hover 反馈; 逐条标注而非事后覆盖,
 * 避免主按钮的淡橙底被同特异性的兜底规则抹掉。 */
.action-btn:hover:not(.primary):not(:disabled),
.mini-btn:hover:not(:disabled),
.icon-btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
}

/* 同 SourceManagerHeader: 不给主按钮单独兜底, 它的淡橙底会被上面那条抹掉。 */
.action-btn.primary:hover:not(:disabled) {
  background: var(--color-primary-soft-bg-hover);
  border-color: var(--color-primary);
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity var(--duration-normal) var(--ease-out);
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

@media (max-width: 640px) {
  .subscription-backdrop {
    align-items: stretch;
    padding: 12px;
  }

  .remote-form,
  .subscription-item {
    flex-direction: column;
    align-items: stretch;
  }
}
</style>
