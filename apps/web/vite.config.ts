import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import fs from 'node:fs'
import { defaultSettingsPath } from '@locagens/shared'

// The backend port lives in settings.json (the same file the Settings screen
// edits). Read it here so the dev frontend connects to the configured port
// without a hardcoded value. Mirrors AppSettingsStore on the backend.
function resolveBackendPort(): number {
  try {
    const parsed = JSON.parse(fs.readFileSync(defaultSettingsPath(), 'utf-8'))
    const port = Number(parsed?.port)
    if (Number.isInteger(port) && port >= 1 && port <= 65535) return port
  } catch {
    // fall through to default
  }
  return Number(process.env.PORT) || 4321
}

// https://vite.dev/config/
export default defineConfig(({ command }) => ({
  // The packaged Electron app loads index.html over file://, so built asset
  // paths must be relative ("./assets/...") rather than absolute ("/assets/...")
  // which would resolve to the filesystem root and 404 (blank window). The dev
  // server keeps the absolute base.
  base: command === "build" ? "./" : "/",
  plugins: [vue()],
  server: {
    // Fixed port so the Electron dev window can rely on it (no fallback to 5174).
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': {
        target: `http://127.0.0.1:${resolveBackendPort()}`,
        changeOrigin: false,
        configure(proxy) {
          proxy.on('proxyReq', (proxyReq) => {
            const token = process.env.LOCAGENS_API_TOKEN || 'locagens-development-token-not-for-production-0000000000000000'
            proxyReq.setHeader('authorization', `Bearer ${token}`)
          })
        }
      }
    },
  },
  define: {
    // Browser development uses the authenticated Vite proxy. Packaged Electron
    // injects its loopback base and transports requests through main-process IPC.
    'import.meta.env.VITE_API_BASE': JSON.stringify(''),
  },
}))
