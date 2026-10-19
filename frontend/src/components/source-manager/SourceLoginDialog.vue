<template>
  <Transition name="scale">
    <div v-if="modelValue && source" class="source-login-overlay" @click.self="close" @keydown.esc.stop="close">
      <section class="source-login-dialog" role="dialog" aria-modal="true" :aria-labelledby="`${formId}-title`" :inert="!!browser">
        <header class="login-header">
          <div class="login-heading">
            <h3 :id="`${formId}-title`">{{ source.bookSourceName }} · 登录</h3>
            <p :title="previewUrl || source.bookSourceUrl">{{ previewUrl || source.bookSourceUrl }}</p>
          </div>
          <button class="close-button" type="button" aria-label="关闭登录窗口" @click="close">×</button>
        </header>
        <div class="login-body">
          <p v-if="loading" class="login-status" role="status">正在读取登录配置…</p>
          <p v-if="error" class="login-error" role="alert">{{ error }}</p>
          <div v-if="view?.mode === 'form'" class="login-form" :aria-busy="busy">
            <template v-for="(field, index) in view.fields" :key="`${field.name}-${index}`">
            <div v-if="field.style?.layout_wrapBefore" class="login-wrap" aria-hidden="true"></div>
            <div class="login-field" :style="fieldStyle(field)">
              <p v-if="field.type === 'button' && !field.action?.trim()" class="login-note">{{ field.viewName || field.name }}</p>
              <button v-else-if="field.type === 'button'" type="button" class="login-button" :disabled="busy || savingCookie" @click="runAction(`button:${index}`)">
                {{ field.viewName || field.name }}
              </button>
              <template v-else>
                <label :for="`${formId}-${index}`">{{ field.viewName || field.name }}</label>
                <select v-if="field.type === 'select'" :id="`${formId}-${index}`" v-model="values[field.name]" :disabled="busy" @change="field.action && runAction(`button:${index}`)">
                  <option v-for="option in field.chars?.filter((option) => option != null) || []" :key="String(option)" :value="String(option ?? '')">{{ option }}</option>
                </select>
                <input v-else-if="field.type === 'toggle'" :id="`${formId}-${index}`" type="checkbox" :checked="values[field.name] === String(field.chars?.[0] ?? 'true')" :disabled="busy" @change="toggle(field, index, $event)" />
                <input v-else :id="`${formId}-${index}`" v-model="values[field.name]" :type="field.type" :placeholder="field.placeholder || ''" :disabled="busy" :autocomplete="field.type === 'password' ? 'current-password' : 'off'" />
              </template>
            </div>
            </template>
          </div>
          <div v-if="messages.length" class="login-messages" role="status">
            <p v-for="(message, index) in messages" :key="index">{{ message }}</p>
          </div>          <section class="cookie-section" :aria-labelledby="`${formId}-cookie-label`">
            <label :id="`${formId}-cookie-label`" :for="`${formId}-cookie`">手动导入 Cookie</label>
            <p>粘贴在网站登录后获取的 Cookie，保存后此书源会使用新的登录状态。</p>
            <textarea :id="`${formId}-cookie`" v-model="cookieText" rows="3" spellcheck="false" autocomplete="off" placeholder="例如：session=…; user=…" :disabled="savingCookie || busy"></textarea>
            <div class="cookie-actions">
              <button class="login-button primary" type="button" :disabled="!cookieText.trim() || savingCookie || busy" @click="saveCookie(false)">{{ savingCookie ? '保存中…' : '导入 Cookie' }}</button>
              <button class="login-button" type="button" :disabled="savingCookie || busy" @click="saveCookie(true)">清除 Cookie</button>
            </div>
          </section>
          <iframe v-if="frameUrl" class="login-frame" :src="frameUrl" title="书源网页登录"></iframe>
        </div>
        <footer class="login-footer">
          <span v-if="busy" role="status">正在执行书源操作…</span>
          <button class="login-button" type="button" @click="close">关闭</button>
          <button v-if="view?.mode === 'form'" class="login-button primary" type="button" :disabled="busy || savingCookie || loading" @click="runAction('login')">确认登录</button>
        </footer>
      </section>
    </div>
  </Transition>
  <SourceBrowserPanel :request="browser" @close="browser = null" @login-data="updateLoginData" @refresh-login="refreshLogin" @toast="notify" />
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue'
import { loginBookSource, setBookSourceCookie, type SourceLoginField, type SourceLoginView } from '../../api/source'
import { readerOrigin } from '../../api/scheme'
import type { BookSource } from '../../types'
import SourceBrowserPanel from '../SourceBrowserPanel.vue'
import { closeSourcePanel, type ShowBrowserRequest } from '../../api/sourceClick'
import type { CSSProperties } from 'vue'
import { useAppStore } from '../../stores/app'

const props = defineProps<{ modelValue: boolean; source: BookSource | null }>()
const emit = defineEmits<{ 'update:modelValue': [value: boolean] }>()
const appStore = useAppStore()
const formId = useId()
const view = ref<SourceLoginView | null>(null)
const values = ref<Record<string, string>>({})
const loading = ref(false)
const busy = ref(false)
const savingCookie = ref(false)
const error = ref('')
const messages = ref<string[]>([])
const cookieText = ref('')
const browser = ref<ShowBrowserRequest | null>(null)
let generation = 0
onBeforeUnmount(() => { ++generation })

const previewUrl = computed(() => view.value?.openUrl || view.value?.url || '')
/** 书源脚本的 toast / 操作结果统一走全局弹窗：登录对话框下半部分常被
 *  预览 iframe 占满，写在对话框里的消息用户根本看不到。 */
function notify(message: string, type: 'success' | 'error' | 'warning' = 'success') {
  const text = String(message || '').trim()
  if (!text) return
  messages.value = [text]
  appStore.showToast(text, type)
}

const frameUrl = computed(() => {
  if (!/^https?:\/\//i.test(previewUrl.value)) return ''
  // These login pages depend on their original document.domain.
  if (/^https?:\/\/([\w-]+\.)?(qidian|yuewen)\.com(?:[/:]|$)/i.test(previewUrl.value)) return previewUrl.value
  // 登录页跑在 reader origin 的专属路径空间里, 且**完整保留上游路径**:
  //   http://reader.localhost/__login/<session>/user/me?app=yuedu&version=...
  //
  // 原因有两个, 缺一不可:
  // 1. 打包产出的单页应用把路径写死进代码 —— Vue Router 的 base 是 `/user/`,
  //    分片也可能是 `"/user/" + chunk`。文档路径一旦对不上, 这些绝对路径全部
  //    404, 应用永远挂载不起来(纯白页)。
  // 2. WebView2 在自定义协议下**不给子资源发 Cookie, Referer 也是空串**(实测),
  //    所以会话只能编码进路径, 让每个分片请求天然带着它 —— 见后端
  //    `source_login_fallback_route`。
  //
  // 效果等于在桌面端复刻 legado 手机端直接 `loadUrl(登录页)` 的行为。
  const session = view.value?.loginSession || ''
  try {
    const target = new URL(previewUrl.value)
    return `${readerOrigin}/__login/${encodeURIComponent(session)}${target.pathname}${target.search}${target.hash}`
  } catch {
    const params = new URLSearchParams({ loginSession: session, url: previewUrl.value })
    return `${readerOrigin}/bookSourceProxy?${params}`
  }
})

function fieldStyle(field: SourceLoginField): CSSProperties {
  const style = field.style || {}
  const numeric = (key: string, fallback: number) => typeof style[key] === 'number' && Number.isFinite(style[key]) ? Math.max(0, Number(style[key])) : fallback
  const requestedBasis = Number(style.layout_flexBasisPercent)
  const basis = Number.isFinite(requestedBasis) && requestedBasis > 0 ? Math.min(1, requestedBasis) : field.type === 'button' ? .5 : 1
  const alignment: Record<string, CSSProperties['alignSelf']> = { flex_start: 'flex-start', flex_end: 'flex-end', center: 'center', baseline: 'baseline', stretch: 'stretch' }
  return {
    flexBasis: `calc(${basis * 100}% - ${12 * (1 - basis)}px)`,
    flexGrow: numeric('layout_flexGrow', 0), flexShrink: numeric('layout_flexShrink', 1),
    alignSelf: alignment[String(style.layout_alignSelf)] || 'auto',
    '--field-align': style.layout_justifySelf === 'flex_start' ? 'left' : style.layout_justifySelf === 'flex_end' || style.layout_justifySelf === 'right' ? 'right' : 'center',
  }
}

function showBrowser(request: ShowBrowserRequest, current: number) {
  if (current === generation && props.modelValue) browser.value = request
  else if (request.panelId) void closeSourcePanel(request.panelId).catch(() => {})
}

function updateLoginData(data: Record<string, unknown> | null) {
  const updates = data || Object.fromEntries((view.value?.fields || []).filter((field) => field.type !== 'button').map((field) => [field.name, field.default ?? field.chars?.[0] ?? '']))
  if (data === null) values.value = {}
  for (const [key, value] of Object.entries(updates)) {
    const field = view.value?.fields.find((field) => field.name === key)
    if (field?.type === 'button') field.viewName = String(value ?? field.name)
    else values.value[key] = String(value ?? field?.default ?? '')
  }
}

async function refreshLogin() {
  if (!props.source || busy.value) return
  const current = generation
  try {
    const result = await loginBookSource(props.source.bookSourceUrl, undefined, { ...values.value }, (request) => showBrowser(request, current))
    if (current === generation) applyView(result)
  } catch (err) {
    if (current === generation) {
      const message = err instanceof Error ? err.message : '刷新登录表单失败'
      error.value = message
      appStore.showToast(message, 'error')
    }
  }
}

function applyView(result: SourceLoginView) {
  view.value = result
  values.value = { ...result.values }
  for (const message of result.messages || []) notify(message)
  if (result.showBrowser) browser.value = result.showBrowser
}

watch(() => [props.modelValue, props.source?.bookSourceUrl] as const, async ([visible]) => {
  const current = ++generation
  view.value = null
  values.value = {}
  error.value = ''
  messages.value = []
  cookieText.value = ''
  browser.value = null
  busy.value = false
  savingCookie.value = false
  loading.value = false
  if (!visible || !props.source) return
  loading.value = true
  try {
    const result = await loginBookSource(props.source.bookSourceUrl, undefined, undefined, (request) => showBrowser(request, current))
    if (current === generation) applyView(result)
  } catch (err) {
    if (current === generation) error.value = err instanceof Error ? err.message : '读取登录配置失败'
  } finally {
    if (current === generation) loading.value = false
  }
}, { immediate: true })

async function runAction(action: string) {
  if (!props.source || busy.value || savingCookie.value) return
  const current = generation
  busy.value = true
  error.value = ''
  messages.value = []
  try {
    const result = await loginBookSource(props.source.bookSourceUrl, action, { ...values.value }, (request) => showBrowser(request, current))
    if (current !== generation) return
    applyView(result)
    if (!messages.value.length && !result.showBrowser && !result.cancelled) notify(action === 'login' ? '登录操作已完成' : '操作已完成')
  } catch (err) {
    if (current === generation) {
      const message = err instanceof Error ? err.message : '登录操作失败'
      error.value = message
      appStore.showToast(message, 'error')
    }
  } finally {
    if (current === generation) busy.value = false
  }
}

function toggle(field: SourceLoginField, index: number, event: Event) {
  values.value[field.name] = String((event.target as HTMLInputElement).checked ? field.chars?.[0] ?? 'true' : field.chars?.[1] ?? 'false')
  if (field.action) void runAction(`button:${index}`)
}

async function saveCookie(clear: boolean) {
  if (!props.source || savingCookie.value || busy.value) return
  const current = generation
  savingCookie.value = true
  error.value = ''
  try {
    const result = await setBookSourceCookie(props.source.bookSourceUrl, clear ? '' : cookieText.value.trim())
    if (current !== generation) return
    if (!result.saved) throw new Error('保存 Cookie 失败')
    cookieText.value = ''
    notify(clear ? 'Cookie 已清除' : 'Cookie 已保存')
  } catch (err) {
    if (current === generation) {
      const message = err instanceof Error ? err.message : '保存 Cookie 失败'
      error.value = message
      appStore.showToast(message, 'error')
    }
  } finally {
    if (current === generation) savingCookie.value = false
  }
}

function close() { ++generation; browser.value = null; emit('update:modelValue', false) }
</script>

<style scoped>
.source-login-overlay { position: fixed; inset: 0; z-index: calc(var(--z-modal, 1000) + 2); display: flex; align-items: center; justify-content: center; padding: 20px; background: rgba(0, 0, 0, .35); }
.source-login-dialog { width: min(880px, 100%); max-height: calc(100dvh - 40px); height: min(86dvh, 880px); display: flex; flex-direction: column; overflow: hidden; background: var(--color-bg-elevated); color: var(--color-text); border-radius: var(--radius-xl, 16px); box-shadow: var(--shadow-xl); }
.login-header { flex: none; display: flex; align-items: center; gap: 16px; padding: 18px 22px; border-bottom: 1px solid var(--color-border-light); }
.login-heading { min-width: 0; flex: 1; }
.login-heading h3 { margin: 0; font-size: 17px; }
.login-heading p { margin: 6px 0 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--color-text-tertiary); }
.close-button { flex: none; border: 0; background: transparent; color: inherit; width: 32px; height: 32px; font-size: 25px; cursor: pointer; }
.login-body { flex: 1; min-height: 0; overflow-y: auto; padding: 20px 22px; display: flex; flex-direction: column; gap: 18px; }
.login-form { display: flex; flex-wrap: wrap; gap: 12px; flex: none; }
.login-wrap { flex-basis: 100%; height: 0; }
.login-field { min-width: 0; display: flex; flex-direction: column; gap: 7px; }
.login-field > .login-button { height: 100%; text-align: var(--field-align, center); overflow-wrap: anywhere; }
.login-note { margin: 4px 0; font-size: 13px; text-align: var(--field-align, center); color: var(--color-text-secondary); }
.login-field label, .cookie-section > label { font-size: 13px; font-weight: 600; }
.login-field input:not([type=checkbox]), .login-field select, .cookie-section textarea { box-sizing: border-box; width: 100%; border: 1px solid var(--color-border); border-radius: var(--radius-md, 8px); background: var(--color-bg-field, var(--color-bg)); color: var(--color-text); padding: 10px 12px; font: inherit; font-size: 13px; }
.login-field input[type=checkbox] { align-self: flex-start; }
.login-button { min-height: 36px; padding: 8px 14px; border: 1px solid var(--color-border); border-radius: var(--radius-md, 8px); color: var(--color-text); background: var(--color-bg); cursor: pointer; font-size: 13px; }
.login-button:hover:not(:disabled) { background: var(--color-bg-hover); }
.login-button.primary { background: var(--color-primary-soft-bg); color: var(--color-primary-soft-text); border-color: var(--color-primary-soft-border); }
.login-button:disabled { opacity: .5; cursor: default; }
.cookie-section { flex: none; display: flex; flex-direction: column; gap: 9px; padding-top: 16px; border-top: 1px solid var(--color-border-light); }
.cookie-section p, .login-status { margin: 0; font-size: 12px; color: var(--color-text-secondary); line-height: 1.6; }
.cookie-section textarea { resize: vertical; min-height: 78px; max-height: 220px; }
.cookie-actions, .login-footer { display: flex; align-items: center; gap: 10px; }
.login-footer { flex: none; justify-content: flex-end; padding: 14px 22px; border-top: 1px solid var(--color-border-light); }
.login-footer span { margin-right: auto; font-size: 12px; color: var(--color-text-secondary); }
.login-error { color: var(--color-danger, #b23b38); margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13px; flex: none; }
.login-messages { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13px; flex: none; }
.login-messages p { margin: 0 0 6px; }
.login-frame { flex: 1 0 320px; width: 100%; min-height: 240px; border: 1px solid var(--color-border-light); background: #fff; }
@media (max-width: 560px) { .source-login-overlay { padding: 8px; } .source-login-dialog { height: calc(100dvh - 16px); max-height: calc(100dvh - 16px); } .login-header, .login-body, .login-footer { padding-left: 14px; padding-right: 14px; } }
</style>
