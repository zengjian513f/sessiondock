import { fileURLToPath } from 'node:url'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

export default defineConfig({
  base: './',
  plugins: [vue()],
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    outDir: '../legacy-web/framework',
    emptyOutDir: true,
    lib: {
      entry: fileURLToPath(new URL('./src/legacy/settings.ts', import.meta.url)),
      name: 'SessionDockSettings',
      formats: ['iife'],
      fileName: () => 'settings.js',
    },
  },
})
