import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'

// The highlighting worker uses the same grammar module as the main renderer.
// Bundle its imports so the Rust snapshot serves one complete Worker entry.
export default defineConfig({
  base: './',
  publicDir: false,
  build: {
    outDir: fileURLToPath(new URL('./dist-migration', import.meta.url)),
    emptyOutDir: false,
    lib: {
      entry: fileURLToPath(new URL('./src/workers/syntax-worker.js', import.meta.url)),
      formats: ['es'],
      fileName: () => 'syntax-worker.js',
    },
  },
})
