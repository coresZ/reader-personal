import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'path'
import pkg from './package.json'

export default defineConfig({
  plugins: [vue()],
  define: {
    __APP_VERSION__: JSON.stringify(`v${pkg.version}`),
  },
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
    },
  },
  server: {
    port: 5173,
  },
  build: {
    outDir: 'dist',
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('/src/utils/chinese.js')) {
            return 'chinese-convert'
          }
          // pdf.js 只被本地 PDF 用到: 单独分块, 普通书籍不加载
          if (id.includes('node_modules/pdfjs-dist')) {
            return 'pdfjs'
          }
          if (id.includes('node_modules/vue') || id.includes('node_modules/pinia') || id.includes('node_modules/vue-router')) {
            return 'vendor-core'
          }
          if (id.includes('node_modules')) {
            return 'vendor-misc'
          }
        },
      },
    },
  },
})
