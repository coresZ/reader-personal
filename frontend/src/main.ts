import { createApp } from 'vue'
import { createPinia } from 'pinia'
import router from './router'
import App from './App.vue'
import './styles/global.css'
import { useAppStore } from './stores/app'
import { isWebUiMode } from './api/transport'
import { registerViewportSync } from './utils/viewport'

const app = createApp(App)
const pinia = createPinia()

// Web 阅读模式: 没有 Tauri 的 WebView 外壳, 也就没有自绘标题栏。
// 给 <html> 挂一个标记类, 让 variables.css 把 --titlebar-height 归零 ——
// 否则设置抽屉、模态框这些 `top: var(--titlebar-height)` 的浮层会空出 30px,
// 表现为面板和顶部之间多出一条缝。
if (isWebUiMode()) {
  document.documentElement.classList.add('web-mode')
}

app.use(pinia)
app.use(router)
registerViewportSync()
const appStore = useAppStore(pinia)
appStore.setOnlineStatus(navigator.onLine)
window.addEventListener('online', () => {
  appStore.setOnlineStatus(true)
  appStore.showToast('网络已恢复', 'success')
})
window.addEventListener('offline', () => {
  appStore.setOnlineStatus(false)
  appStore.showToast('已进入离线模式', 'warning')
})

app.mount('#app')
