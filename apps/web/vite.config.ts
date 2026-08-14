import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

// The backend port lives in settings.json (the same file the Settings screen
// edits). Read it here so the dev frontend connects to the configured port
// without a hardcoded value. Mirrors AppSettingsStore on the backend.
function resolveSettingsPath(): string {
  if (process.env.LOCAGENS_SETTINGS_PATH) return process.env.LOCAGENS_SETTINGS_PATH
  const appDirName = 'Locagens'
  if (process.platform === 'darwin') {
    return path.join(os.homedir(), 'Library', 'Application Support', appDirName, 'settings.json')
  }
  if (process.platform === 'win32') {
    return path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), appDirName, 'settings.json')
  }
  return path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'locagens', 'settings.json')
}

function resolveBackendPort(): number {
  try {
    const parsed = JSON.parse(fs.readFileSync(resolveSettingsPath(), 'utf-8'))
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
