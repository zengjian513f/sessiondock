import { copyFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import vue from '@vitejs/plugin-vue'
import { build, defineConfig } from 'vite'

const webRoot = fileURLToPath(new URL('.', import.meta.url))
const outputRoot = resolve(webRoot, 'dist-pages')
const pages = ['grid', 'records', 'file'] as const
// Build the independent ESM entries with
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
              lib: { entry: resolve(webRoot, `src/pages/${page}/entry.ts`), formats: ['es'], fileName: () => `${page}.js` },
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
    lib: { entry: resolve(webRoot, 'src/pages/grid/entry.ts'), formats: ['es'], fileName: () => 'grid.js' },
  },
})
