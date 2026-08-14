import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import {
  SandboxManager,
  installWindowsSandboxAsync,
  type SandboxRuntimeConfig
} from "@anthropic-ai/sandbox-runtime";
import type { SecurityStatus } from "@locagens/shared";
import { truncateOutput } from "../orchestrator/workspace/pathGuards.js";

const MAX_BUFFER = 4 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS = 120_000;
const SECRET_ENV_NAME = /(TOKEN|SECRET|PASSWORD|PASSWD|API_?KEY|AUTH|COOKIE|CREDENTIAL|LOCAGENS_API)/i;
const DANGEROUS_ENV_NAME = /^(NODE_OPTIONS|BASH_ENV|ENV|ZDOTDIR|GIT_SSH_COMMAND|LD_PRELOAD|DYLD_.*)$/i;
const SAFE_ENV_NAMES = new Set([
  "PATH", "HOME", "SHELL", "USER", "LOGNAME", "LANG", "TERM", "COLORTERM",
  "TMPDIR", "TEMP", "TMP", "JAVA_HOME", "GOPATH", "GOROOT", "CARGO_HOME",
  "RUSTUP_HOME", "SDKROOT", "DEVELOPER_DIR", "SYSTEMROOT", "WINDIR", "COMSPEC",
  "PATHEXT", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE"
]);

export interface SandboxCommandResult {
  success: boolean;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  error?: string;
}

export interface CommandSandbox {
  status(): Promise<SecurityStatus["sandbox"]>;
  installWindows(): Promise<SecurityStatus["sandbox"]>;
  run(cwd: string, command: string, networkDomains: unknown, timeoutMs?: number): Promise<SandboxCommandResult>;
}

function domainIsSafe(domain: string): boolean {
  if (!/^(\*\.)?[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::\d{1,5})?$/i.test(domain)) return false;
  const host = domain.replace(/^\*\./, "").split(":", 1)[0].toLowerCase();
  return host !== "localhost" && !host.endsWith(".localhost") && !/^\d+(?:\.\d+){3}$/.test(host) && !host.includes(":");
}

export function normalizeNetworkDomains(value: unknown): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error("network_domains must be an array of host names.");
  const normalized = [...new Set(value.map(item => String(item).trim().toLowerCase()).filter(Boolean))].sort();
  if (normalized.length > 20) throw new Error("At most 20 network domains may be requested per command.");
  if (normalized.some(domain => !domainIsSafe(domain))) {
    throw new Error("network_domains contains an invalid, local, or IP-literal host.");
  }
  return normalized;
}

function sanitizedEnvironment(runtimeEnv: NodeJS.ProcessEnv, sandboxTemp: string): NodeJS.ProcessEnv {
  const safe: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(runtimeEnv)) {
    if (value === undefined || SECRET_ENV_NAME.test(key) || DANGEROUS_ENV_NAME.test(key)) continue;
    const runtimeAdded = process.env[key] !== value;
    if (runtimeAdded || SAFE_ENV_NAMES.has(key) || key.startsWith("LC_") || key.startsWith("GIT_CONFIG_")) {
      safe[key] = value;
    }
  }
  safe.TMPDIR = sandboxTemp;
  safe.TMP = sandboxTemp;
  safe.TEMP = sandboxTemp;
  return safe;
}

function sandboxConfig(cwd: string, tempDir: string, networkDomains: string[]): SandboxRuntimeConfig {
  const runtimeDir = process.env.LOCAGENS_SANDBOX_RUNTIME_DIR;
  const architecture = process.arch === "arm64" ? "arm64" : "x64";
  return {
    network: {
      allowedDomains: networkDomains,
      deniedDomains: [],
      strictAllowlist: true,
      allowLocalBinding: false,
      allowUnixSockets: []
    },
    filesystem: {
      denyRead: [os.homedir(), os.tmpdir()],
      allowRead: [cwd, tempDir],
      allowWrite: [cwd, tempDir],
      denyWrite: [
        path.join(cwd, ".env"),
        path.join(cwd, ".git", "config"),
        path.join(cwd, ".git", "hooks")
      ],
      allowGitConfig: false
    },
    credentials: {
      envVars: [{ name: "LOCAGENS_API_TOKEN", mode: "deny" }]
    },
    git: { safeDirectories: [cwd] },
    ...(runtimeDir ? {
      seccomp: { applyPath: path.join(runtimeDir, "seccomp", architecture, "apply-seccomp") },
      windows: { srtWin: { path: path.join(runtimeDir, "srt-win", architecture, "srt-win.exe") } }
    } : {}),
    allowAppleEvents: false,
    enableWeakerNestedSandbox: false
  };
}

export class AnthropicSandboxRuntimeAdapter implements CommandSandbox {
  private queue: Promise<void> = Promise.resolve();

  async status(): Promise<SecurityStatus["sandbox"]> {
    if (!SandboxManager.isSupportedPlatform()) {
      return { platform: process.platform, status: "unavailable", errors: [`Unsupported platform: ${process.platform}`], warnings: [], canInstall: false };
    }
    try {
      const check = await SandboxManager.checkDependenciesAsync();
      const setupRequired = process.platform === "win32" && check.errors.length > 0;
      return {
        platform: process.platform,
        status: check.errors.length === 0 ? "ready" : (setupRequired ? "setup_required" : "unavailable"),
        errors: check.errors,
        warnings: check.warnings,
        canInstall: setupRequired
      };
    } catch (error: any) {
      return { platform: process.platform, status: "unavailable", errors: [error?.message || "Sandbox dependency check failed."], warnings: [], canInstall: false };
    }
  }

  async installWindows(): Promise<SecurityStatus["sandbox"]> {
    if (process.platform !== "win32") throw new Error("Windows sandbox setup is only available on Windows.");
    await installWindowsSandboxAsync();
    return this.status();
  }

  async run(cwdInput: string, commandInput: string, domainsInput: unknown, timeoutMs = DEFAULT_TIMEOUT_MS): Promise<SandboxCommandResult> {
    const previous = this.queue;
    let release!: () => void;
    this.queue = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try {
      return await this.runExclusive(cwdInput, commandInput, domainsInput, timeoutMs);
    } finally {
      release();
    }
  }

  private async runExclusive(cwdInput: string, commandInput: string, domainsInput: unknown, timeoutMs: number): Promise<SandboxCommandResult> {
    const cwd = fs.realpathSync.native(cwdInput);
    const command = commandInput.trim();
    if (!command) return { success: false, exitCode: null, stdout: "", stderr: "", error: "Missing parameter: command" };
    const networkDomains = normalizeNetworkDomains(domainsInput);
    const status = await this.status();
    if (status.status !== "ready") {
      return { success: false, exitCode: null, stdout: "", stderr: "", error: `Sandbox is not ready: ${status.errors.join("; ") || status.status}` };
    }

    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-sandbox-"));
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), timeoutMs);
    const commandId = `locagens-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    try {
      await SandboxManager.initialize(sandboxConfig(cwd, tempDir, networkDomains), undefined, false);
      const wrapped = await SandboxManager.wrapWithSandboxArgv(command, undefined, undefined, controller.signal, cwd, { commandId, commandText: command });
      const env = sanitizedEnvironment(wrapped.env, tempDir);
      return await new Promise<SandboxCommandResult>((resolve) => {
        const child = spawn(wrapped.argv[0], wrapped.argv.slice(1), { cwd, shell: false, env, stdio: ["ignore", "pipe", "pipe"] });
        const stdout: Buffer[] = [];
        const stderr: Buffer[] = [];
        let stdoutBytes = 0;
        let stderrBytes = 0;
        const collect = (target: Buffer[], chunk: Buffer, current: number) => {
          if (current < MAX_BUFFER) target.push(Buffer.from(chunk.subarray(0, MAX_BUFFER - current)));
        };
        child.stdout.on("data", (chunk: Buffer) => { collect(stdout, chunk, stdoutBytes); stdoutBytes += chunk.length; });
        child.stderr.on("data", (chunk: Buffer) => { collect(stderr, chunk, stderrBytes); stderrBytes += chunk.length; });
        const abort = () => child.kill("SIGTERM");
        controller.signal.addEventListener("abort", abort, { once: true });
        child.on("error", error => resolve({ success: false, exitCode: null, stdout: "", stderr: "", error: error.message }));
        child.on("close", (code, signal) => {
          controller.signal.removeEventListener("abort", abort);
          const rawStderr = Buffer.concat(stderr).toString("utf-8");
          const annotated = SandboxManager.annotateStderrWithSandboxFailures(commandId, rawStderr);
          resolve({
            success: code === 0 && !signal,
            exitCode: code,
            stdout: truncateOutput(Buffer.concat(stdout).toString("utf-8")),
            stderr: truncateOutput(annotated),
            error: controller.signal.aborted ? `Command timed out after ${timeoutMs}ms.` : undefined
          });
        });
      });
    } catch (error: any) {
      return { success: false, exitCode: null, stdout: "", stderr: "", error: error?.message || "Sandboxed command failed." };
    } finally {
      clearTimeout(timeout);
      SandboxManager.cleanupAfterCommand();
      await SandboxManager.reset().catch(() => undefined);
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  }
}

export const commandSandbox: CommandSandbox = new AnthropicSandboxRuntimeAdapter();
