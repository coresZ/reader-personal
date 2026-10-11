import { createApp, h, ref } from 'vue'
import SourceLoginDialog from '../../src/components/source-manager/SourceLoginDialog.vue'
import '../../src/styles/global.css'

const mode = new URLSearchParams(location.search).get('mode') || 'form'
const calls: unknown[] = []
let callbackId = 0
let pendingPanel: ((completed: boolean) => void) | null = null
Object.assign(window, {
  __loginCalls: calls,
  __fixtureCompletePanel() { pendingPanel?.(true); pendingPanel = null },
  __TAURI_INTERNALS__: {
    transformCallback() { return ++callbackId },
    unregisterCallback() {},
    async invoke(command: string, args: { req?: Record<string, any>; panelId?: string; onBrowser?: { onmessage: (request: unknown) => void } }) {
      const req = args.req || {}
      calls.push({ command, req, panelId: args.panelId })
      if (command === 'close_source_panel') {
        if (args.panelId === 'fixture-settings') { pendingPanel?.(false); pendingPanel = null }
        return
      }
      if (command === 'set_book_source_cookie') return { isSuccess: true, data: { saved: true } }
      if (mode === 'failure') throw new Error('登录配置加载失败')
      let cancelled = false
      if (mode === 'panels' && req.action === 'button:2') {
        const completion = new Promise<boolean>((resolve) => { pendingPanel = resolve })
        args.onBrowser?.onmessage({
          panelId: 'fixture-settings', url: 'https://fixture.test/settings', title: '线路设置',
          awaitResult: true, options: { heightPercentage: .8, expandedCornersRadius: 20, isDraggable: false, dismissOnTouchOutside: false },
        })
        cancelled = !await completion
      }
      const fields = mode === 'layout' ? [
        { name: '切换线路', type: 'button', action: 'open()', style: { layout_flexBasisPercent: 1, layout_flexGrow: 1 } },
        ...['书源设置', '段评设置'].map((name) => ({ name, type: 'button', action: 'open()', style: { layout_flexBasisPercent: .4, layout_flexGrow: 1 } })),
        ...['段评开关', '图片开关', '简介开关', '同步书架'].map((name) => ({ name, type: 'button', action: 'toggle()', style: { layout_flexBasisPercent: .25, layout_flexGrow: 1 } })),
        { name: '设置说明', type: 'button', action: '', style: { layout_flexBasisPercent: 1 } },
        ...Array.from({ length: 26 }, (_, index) => ({ name: '设置选项 ' + (index + 1), type: 'text' })),
      ] : [
        { name: '设备ID', type: 'text' },
        { name: '密码', type: 'password' },
        { name: '设置设备', type: 'button', action: 'configure()' },
      ]
      return {
        isSuccess: true,
        data: {
          mode: mode === 'long' ? 'web' : 'form', fields,
          values: req.action ? { ...req.values, ...(!cancelled && { 设备ID: '已配置' }) } : { 设备ID: '', 密码: '' },
          url: mode === 'long' ? 'https://example.invalid/login?state=' + 'a'.repeat(7400) : null,
          loginSession: 'fixture-session',
          messages: cancelled ? ['已取消'] : req.action ? ['设置完成'] : [],
          actionCompleted: Boolean(req.action) && !cancelled, cancelled,
        },
      }
    },
  },
})

createApp({
  setup() {
    const visible = ref(true)
    Object.assign(window, { __setLoginVisible: (value: boolean) => { visible.value = value } })
    return () => h(SourceLoginDialog, {
      modelValue: visible.value,
      'onUpdate:modelValue': (value: boolean) => { visible.value = value },
      source: { bookSourceName: '测试书源', bookSourceUrl: 'https://example.invalid', loginUi: '[]' },
    })
  },
}).mount('#app')
