import { cpSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

const webRoot = fileURLToPath(new URL('.', import.meta.url))
const legacyRoot = resolve(webRoot, '../legacy-web')
const compatRoot = resolve(webRoot, 'src/compat')
const outputRoot = resolve(webRoot, 'dist-migration')

export default defineConfig({
  base: './',
  publicDir: false,
  plugins: [
    vue({ template: { compilerOptions: { whitespace: 'preserve' } } }),
    {
      name: 'migration-static-assets',
      writeBundle() {
        // Reuse raw assets and standalone pages. The production app and settings
        // bundle are excluded: only the migration-owned sources supply those.
        for (const name of readdirSync(legacyRoot)) {
          if (name === 'index.html' || name === 'framework' || name.endsWith('.js')) continue
          cpSync(resolve(legacyRoot, name), resolve(outputRoot, name), { recursive: true })
        }
        for (const name of readdirSync(compatRoot)) {
          if (name.endsWith('.js')) cpSync(resolve(compatRoot, name), resolve(outputRoot, name))
        }
        cpSync(resolve(webRoot, 'migration/index.html'), resolve(outputRoot, 'index.html'))
      },
    },
  ],
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    outDir: outputRoot,
    emptyOutDir: true,
    lib: {
      entry: resolve(webRoot, 'src/migration/main.ts'),
      name: 'SessionDockMigration',
      formats: ['iife'],
      fileName: () => 'migration-shell.js',
    },
  },
})
