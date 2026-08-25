import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { dataDir, isDev, resolvePort, webIndex } from "./paths";
import { startBackend, stopBackend } from "./backend";

const DEV_URL = "http://localhost:5173";
const DEV_API_TOKEN = "locagens-development-token-not-for-production-0000000000000000";
const apiToken = isDev
  ? (process.env.LOCAGENS_API_TOKEN || DEV_API_TOKEN)
  : randomBytes(32).toString("base64url");

// Force the app name so app.getPath("userData") resolves to
// ~/Library/Application Support/Locagens (matching the backend's own default
// config dir) rather than the scoped package name "@locagens/desktop".
app.setName("Locagens");

let win: BrowserWindow | null = null;
const eventStreams = new Map<string, AbortController>();

function apiBase(port: number): string {
  return `http://127.0.0.1:${port}`;
}

/**
 * Ensures the backend is up for the current environment and returns its port.
 * In prod the main process owns the backend (fork + health-check). In dev the
 * backend runs standalone via `pnpm dev`, so we only resolve the port.
 */
async function ensureBackend(): Promise<number> {
  fs.mkdirSync(dataDir(), { recursive: true });
  const port = resolvePort();
  if (!isDev) {
    await startBackend(port, apiToken);
  }
  return port;
}

function createWindow(port: number): void {
  win = new BrowserWindow({
    width: 1280,
    height: 860,
    backgroundColor: "#1a1a1a",
    // Hide the native title bar; keep the macOS traffic lights inset over the
    // app's top-left. The window stays draggable via -webkit-app-region on the
    // app's own header bars (see the .is-desktop styles in the web app).
    titleBarStyle: "hiddenInset",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      additionalArguments: [`--locagens-api-base=${apiBase(port)}`],
    },
  });

  if (isDev) {
    win.loadURL(DEV_URL);
    win.webContents.openDevTools({ mode: "detach" });
  } else {
    win.loadFile(webIndex());
  }

  // In dev the Vite server may not be up yet (or starts after Electron). Retry
  // the load instead of leaving a blank window. Also surfaces real load errors.
  win.webContents.on("did-fail-load", (_e, code, desc, url) => {
    if (code === -3) return; // aborted (e.g. superseded by a new load)
    console.error(`[window] failed to load ${url}: ${code} ${desc}`);
    if (isDev) {
      setTimeout(() => {
        if (win && !win.isDestroyed()) win.loadURL(DEV_URL);
      }, 1000);
    }
  });

  // Open external links in the user's browser, not inside the app window.
  win.webContents.setWindowOpenHandler(({ url }) => {
    try {
      const protocol = new URL(url).protocol;
      if (protocol === "https:" || protocol === "http:") void shell.openExternal(url);
    } catch {
      // Malformed URLs stay blocked.
    }
    return { action: "deny" };
  });

  win.webContents.on("will-navigate", (event, url) => {
    let allowed = false;
    try {
      const target = new URL(url);
      if (isDev) {
        allowed = target.origin === DEV_URL;
      } else {
        const entry = new URL(pathToFileURL(webIndex()).href);
        allowed = target.protocol === "file:" && target.pathname === entry.pathname;
      }
    } catch {
      allowed = false;
    }
    if (!allowed) event.preventDefault();
  });

  win.on("closed", () => {
    win = null;
  });
}

// Double-clicking the app's draggable header toggles the macOS "zoom" (fill the
// screen's width/height, not native fullscreen). Deterministic regardless of the
// system's title-bar double-click preference.
ipcMain.handle("locagens:toggle-maximize", (event) => {
  const target = BrowserWindow.fromWebContents(event.sender);
  if (!target) return;
  if (target.isMaximized()) target.unmaximize();
  else target.maximize();
});

ipcMain.handle("locagens:select-directory", async (event) => {
  const target = BrowserWindow.fromWebContents(event.sender);
  if (!target) throw new Error("Folder picker is not attached to a Locagens window.");
  const result = await dialog.showOpenDialog(target, {
    title: "Select a project folder",
    properties: ["openDirectory", "createDirectory"]
  });
  if (result.canceled || result.filePaths.length === 0) return null;
  const selectedPath = result.filePaths[0];
  return { path: selectedPath, name: path.basename(selectedPath) || "Workspace" };
});

// Opens a local folder/file in the OS file manager. Path must be absolute and
// exist; the API only hands the renderer allowlisted skill (or similar) roots.
ipcMain.handle("locagens:open-path", async (_event, targetPath: unknown) => {
  if (typeof targetPath !== "string" || !targetPath.trim()) {
    throw new Error("Path is required.");
  }
  const resolved = path.resolve(targetPath.trim());
  if (!path.isAbsolute(resolved) || resolved.includes("\0")) {
    throw new Error("Invalid path.");
  }
  if (!fs.existsSync(resolved)) {
    throw new Error("Path does not exist.");
  }
  const err = await shell.openPath(resolved);
  if (err) throw new Error(err);
  return { ok: true as const };
});

function validateApiPath(value: unknown): string {
  const apiPath = String(value || "");
  if (!apiPath.startsWith("/api/") || /[\r\n\\]/.test(apiPath) || apiPath.includes("://")) {
    throw new Error("Invalid API path.");
  }
  const rawPathname = apiPath.split(/[?#]/, 1)[0];
  let decodedPathname: string;
  try {
    decodedPathname = decodeURIComponent(rawPathname);
  } catch {
    throw new Error("Invalid API path encoding.");
  }
  if (decodedPathname.split("/").some(segment => segment === "." || segment === ".." || segment.includes("\\"))) {
    throw new Error("Invalid API path traversal.");
  }
  const normalized = new URL(apiPath, "http://127.0.0.1");
  if (!normalized.pathname.startsWith("/api/")) throw new Error("Invalid API path.");
  return apiPath;
}

function authHeaders(extra?: Record<string, string>): Record<string, string> {
  return { ...extra, authorization: `Bearer ${apiToken}` };
}

ipcMain.handle("locagens:api-request", async (_event, input: {
  path?: string;
  method?: string;
  body?: unknown;
}) => {
  const port = resolvePort();
  const apiPath = validateApiPath(input?.path);
  const method = String(input?.method || "GET").toUpperCase();
  if (!["GET", "POST", "PUT", "DELETE"].includes(method)) throw new Error("Invalid API method.");
  const hasBody = input?.body !== undefined;
  const response = await fetch(`${apiBase(port)}${apiPath}`, {
    method,
    headers: authHeaders(hasBody ? { "content-type": "application/json" } : undefined),
    body: hasBody ? JSON.stringify(input.body) : undefined
  });
  return {
    status: response.status,
    contentType: response.headers.get("content-type") || "",
    body: await response.text()
  };
});

function emitRunStream(subscriptionId: string, payload: { type: "message" | "error" | "closed"; data?: string; error?: string }): void {
  if (win && !win.isDestroyed()) {
    win.webContents.send("locagens:run-event", { subscriptionId, ...payload });
  }
}

ipcMain.handle("locagens:subscribe-run-events", async (_event, input: { subscriptionId?: string; runId?: string }) => {
  const subscriptionId = String(input?.subscriptionId || "");
  const runId = String(input?.runId || "");
  if (!/^[A-Za-z0-9_-]{1,160}$/.test(subscriptionId) || !/^[A-Za-z0-9_-]{1,160}$/.test(runId)) {
    throw new Error("Invalid event subscription.");
  }
  eventStreams.get(subscriptionId)?.abort();
  const controller = new AbortController();
  eventStreams.set(subscriptionId, controller);
  const port = resolvePort();

  void (async () => {
    try {
      const response = await fetch(`${apiBase(port)}/api/runs/${encodeURIComponent(runId)}/events`, {
        headers: authHeaders({ accept: "text/event-stream" }),
        signal: controller.signal
      });
      if (!response.ok || !response.body) throw new Error(`Event stream failed with HTTP ${response.status}.`);
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      while (!controller.signal.aborted) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
        let boundary = buffer.indexOf("\n\n");
        while (boundary >= 0) {
          const block = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const data = block.split("\n")
            .filter(line => line.startsWith("data:"))
            .map(line => line.slice(5).trimStart())
            .join("\n");
          if (data) emitRunStream(subscriptionId, { type: "message", data });
          boundary = buffer.indexOf("\n\n");
        }
      }
      if (!controller.signal.aborted) emitRunStream(subscriptionId, { type: "closed" });
    } catch (error: any) {
      if (!controller.signal.aborted) emitRunStream(subscriptionId, { type: "error", error: error?.message || "Event stream failed." });
    } finally {
      if (eventStreams.get(subscriptionId) === controller) eventStreams.delete(subscriptionId);
    }
  })();
  return { success: true };
});

ipcMain.handle("locagens:unsubscribe-run-events", (_event, subscriptionId: string) => {
  eventStreams.get(subscriptionId)?.abort();
  eventStreams.delete(subscriptionId);
  return { success: true };
});

// Restart after the port changes in Settings. In prod, kill + re-fork the
// backend on the new port and recreate the window (so preload re-injects the
// new API base). In dev the backend is external, so we just recreate the
// window; the user restarts `pnpm dev` for the new port to bind.
ipcMain.handle("locagens:restart-backend", async () => {
  const port = resolvePort();
  if (!isDev) {
    await stopBackend();
    await startBackend(port, apiToken);
  }
  const old = win;
  createWindow(port);
  old?.close();
  return { port };
});

app.whenReady().then(async () => {
  const port = await ensureBackend();
  createWindow(port);

  app.on("activate", async () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      const port = await ensureBackend();
      createWindow(port);
    }
  });
});

app.on("window-all-closed", async () => {
  await stopBackend();
  if (process.platform !== "darwin") app.quit();
});

let isQuitting = false;

app.on("before-quit", (event) => {
  if (!isQuitting) {
    event.preventDefault();
    isQuitting = true;
    for (const controller of eventStreams.values()) controller.abort();
    eventStreams.clear();
    stopBackend().finally(() => {
      app.quit();
    });
  }
});
