import { copyFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import vue from '@vitejs/plugin-vue'
import { build, defineConfig } from 'vite'

const webRoot = fileURLToPath(new URL('.', import.meta.url))
const outputRoot = resolve(webRoot, 'dist-pages')
const pages = ['grid', 'records', 'file'] as const
// Rollup IIFE output has one entry. Build the remaining classic scripts with
// the same production Vue compiler, then publish all four HTML entry pages.
export default defineConfig({
  base: './',
  publicDir: false,
  plugins: [
    vue({ template: { compilerOptions: { whitespace: 'preserve' } } }),
    {
      name: 'standalone-vue-pages',
      async closeBundle() {
        for (const page of pages.slice(1)) {
          await build({
            configFile: false,
            base: './',
            publicDir: false,
            plugins: [vue({ template: { compilerOptions: { whitespace: 'preserve' } } })],
            define: { 'process.env.NODE_ENV': JSON.stringify('production') },
            build: {
              outDir: outputRoot,
              emptyOutDir: false,
              lib: { entry: resolve(webRoot, `src/pages/${page}/entry.ts`), name: `SessionDock_${page}`, formats: ['iife'], fileName: () => `${page}.js` },
            },
          })
        }
        for (const page of [...pages, 'files']) {
          copyFileSync(resolve(webRoot, `migration/${page}.html`), resolve(outputRoot, `${page}.html`))
        }
      },
    },
  ],
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    outDir: outputRoot,
    emptyOutDir: true,
    lib: { entry: resolve(webRoot, 'src/pages/grid/entry.ts'), name: 'SessionDock_grid', formats: ['iife'], fileName: () => 'grid.js' },
  },
})
