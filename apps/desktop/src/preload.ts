import { contextBridge, ipcRenderer } from "electron";

// The main process resolves the backend port and passes the base URL as a
// launch argument. Expose it as the global the web client reads
// (api/client.ts -> resolveApiBase), and a tiny bridge for restarting the
// backend after the port is changed in Settings.
const arg = process.argv.find((a) => a.startsWith("--locagens-api-base="));
if (arg) {
  contextBridge.exposeInMainWorld("__LOCAGENS_API_BASE__", arg.slice("--locagens-api-base=".length));
}

contextBridge.exposeInMainWorld("__LOCAGENS_DESKTOP__", {
  restartBackend: () => ipcRenderer.invoke("locagens:restart-backend"),
  toggleMaximize: () => ipcRenderer.invoke("locagens:toggle-maximize"),
  apiRequest: (input: unknown) => ipcRenderer.invoke("locagens:api-request", input),
  subscribeRunEvents: (input: unknown) => ipcRenderer.invoke("locagens:subscribe-run-events", input),
  unsubscribeRunEvents: (subscriptionId: string) => ipcRenderer.invoke("locagens:unsubscribe-run-events", subscriptionId),
  onRunEvent: (listener: (event: unknown) => void) => {
    const handler = (_event: Electron.IpcRendererEvent, payload: unknown) => listener(payload);
    ipcRenderer.on("locagens:run-event", handler);
    return () => ipcRenderer.removeListener("locagens:run-event", handler);
  },
});
