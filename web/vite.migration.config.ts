import { cpSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import vue from '@vitejs/plugin-vue'
import { build, defineConfig } from 'vite'

const webRoot = fileURLToPath(new URL('.', import.meta.url))
const legacyRoot = resolve(webRoot, '../legacy-web')
const outputRoot = resolve(webRoot, 'dist-migration')

export default defineConfig({
  base: './',
  publicDir: false,
  plugins: [
    vue({ template: { compilerOptions: { whitespace: 'preserve' } } }),
    {
      name: 'migration-static-assets',
      async writeBundle() {
        // Reuse raw assets and standalone pages. The production app and settings
        // bundle are excluded: only the migration-owned sources supply those.
        for (const name of readdirSync(legacyRoot)) {
          if (name === 'index.html' || name === 'framework' || name.endsWith('.js')) continue
          cpSync(resolve(legacyRoot, name), resolve(outputRoot, name), { recursive: true })
        }
        cpSync(resolve(webRoot, 'src/workers/regex-worker.js'), resolve(outputRoot, 'regex-worker.js'))
        await build({ configFile: resolve(webRoot, 'vite.workers.config.ts') })
        cpSync(resolve(webRoot, 'migration/index.html'), resolve(outputRoot, 'index.html'))
        await build({ configFile: resolve(webRoot, 'vite.pages.config.ts') })
        for (const name of readdirSync(resolve(webRoot, 'dist-pages'))) {
          cpSync(resolve(webRoot, 'dist-pages', name), resolve(outputRoot, name))
        }
      },
    },
  ],
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    outDir: outputRoot,
    emptyOutDir: true,
    lib: {
      entry: resolve(webRoot, 'src/migration/main.ts'),
      formats: ['es'],
      fileName: () => 'migration-shell.js',
    },
  },
})
