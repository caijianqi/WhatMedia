import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Tauri 要求固定端口 + 禁用 host 检查；Web 构建走默认配置。
const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [vue()],
  css: {
    postcss: {
      plugins: [],
    },
  },
  clearScreen: false,
  // 缓存目录钉在项目内。默认会落到系统临时目录，而在受限的 Windows
  // 环境里往那里写会 EPERM —— 表现是测试文件随机少跑一个，很难查。
  cacheDir: 'node_modules/.vite',
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
    // 开发时前端跑在 1420、服务端跑在 8080，不代理的话 /api 会打到 vite 自己。
    // ws: true 是为了 HLS 之外的将来实时通道（扫描进度推送）。
    proxy: {
      '/api': {
        target: process.env.WHATMEDIA_SERVER ?? 'http://127.0.0.1:8080',
        changeOrigin: true,
      },
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    // 播放器要操作 DOM（<video> / <track>），所以用 jsdom 而不是 node。
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    globals: true,
  },
} as any)
