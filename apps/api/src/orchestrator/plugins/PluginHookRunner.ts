import { spawn } from "node:child_process";
import path from "node:path";
import type {
  PluginManifest,
  SessionStartHookContext,
  SessionStartResult,
  PreToolUseHookContext,
  PreToolUseResult,
  PostToolUseHookContext,
  PostToolUseResult,
  PreCompactHookContext,
  PreCompactResult
} from "./types.js";

/** Max length of tool output before context-mode sandboxing / summarization kicks in. */
const MAX_RAW_OUTPUT_LENGTH = 16_000;

export class PluginHookRunner {
  /**
   * Executes session start hooks across all enabled plugins.
   * Collects additional system prompt supplements and instructions.
   */
  async runSessionStart(
    ctx: SessionStartHookContext,
    activePlugins: PluginManifest[]
  ): Promise<SessionStartResult> {
    const promptSupplements: string[] = [];

    for (const plugin of activePlugins) {
      if (plugin.systemPrompt?.trim()) {
        promptSupplements.push(`[Plugin: ${plugin.name}]\n${plugin.systemPrompt.trim()}`);
      }

      if (plugin.hooks?.onSessionStart) {
        try {
          const result = await this.executeHookCommand(
            plugin.hooks.onSessionStart,
            ctx,
            plugin.dir
          );
          if (result?.systemPromptSupplement) {
            promptSupplements.push(result.systemPromptSupplement);
          }
        } catch (err) {
          console.error(`[PluginHookRunner] Error in onSessionStart hook for ${plugin.id}:`, err);
        }
      }
    }

    return {
      systemPromptSupplement: promptSupplements.length > 0 ? promptSupplements.join("\n\n") : undefined
    };
  }

  /**
   * Executes pre-tool-use hooks before a tool call runs.
   * Can validate, sandbox, or modify tool arguments.
   */
  async runPreToolUse(
    ctx: PreToolUseHookContext,
    activePlugins: PluginManifest[]
  ): Promise<PreToolUseResult> {
    let currentArgs = { ...ctx.args };

    for (const plugin of activePlugins) {
      if (!plugin.hooks?.preToolUse) continue;

      try {
        const result = await this.executeHookCommand(
          plugin.hooks.preToolUse,
          { ...ctx, args: currentArgs },
          plugin.dir
        );

        if (result?.proceed === false) {
          return {
            proceed: false,
            error: result.error || `Tool call blocked by plugin ${plugin.name}.`
          };
        }

        if (result?.handledResult !== undefined) {
          return {
            proceed: false,
            handledResult: result.handledResult
          };
        }

        if (result?.modifiedArgs) {
          currentArgs = result.modifiedArgs;
        }
      } catch (err) {
        console.error(`[PluginHookRunner] Error in preToolUse hook for ${plugin.id}:`, err);
      }
    }

    return {
      proceed: true,
      modifiedArgs: currentArgs
    };
  }

  /**
   * Executes post-tool-use hooks after a tool call returns.
   * Sandboxes and condenses large outputs (e.g. context-mode token optimization).
   */
  async runPostToolUse(
    ctx: PostToolUseHookContext,
    activePlugins: PluginManifest[]
  ): Promise<PostToolUseResult> {
    let currentResult = ctx.rawResult;

    for (const plugin of activePlugins) {
      // 1. Internal context-mode handler
      if (plugin.hooks?.postToolUse === "internal:context-mode-sandbox" || plugin.id === "context-mode") {
        currentResult = this.applyContextModeSandboxing(ctx.toolName, ctx.args, currentResult);
      }

      // 2. Custom external hook command if defined
      if (plugin.hooks?.postToolUse && !plugin.hooks.postToolUse.startsWith("internal:")) {
        try {
          const result = await this.executeHookCommand(
            plugin.hooks.postToolUse,
            { ...ctx, rawResult: currentResult },
            plugin.dir
          );
          if (result?.result) {
            currentResult = result.result;
          }
        } catch (err) {
          console.error(`[PluginHookRunner] Error in postToolUse hook for ${plugin.id}:`, err);
        }
      }
    }

    return {
      result: currentResult
    };
  }

  /**
   * Built-in context-mode output sandboxing & token reduction:
   * Trims massive outputs, preserves key error lines and summary stats.
   */
  private applyContextModeSandboxing(toolName: string, args: Record<string, any>, result: string): string {
    if (!result || result.length <= MAX_RAW_OUTPUT_LENGTH) {
      return result;
    }

    const lines = result.split("\n");
    const totalLines = lines.length;

    if (totalLines > 300) {
      const head = lines.slice(0, 100).join("\n");
      const tail = lines.slice(-100).join("\n");
      const omitted = totalLines - 200;

      return (
        `${head}\n\n` +
        `--- [Context-Mode: Sandboxed ${omitted} middle lines (${(result.length / 1024).toFixed(1)} KB output preserved in session)] ---\n\n` +
        `${tail}`
      );
    }

    return result;
  }

  /**
   * Executes pre-compact hooks before context window compaction.
   */
  async runPreCompact(
    ctx: PreCompactHookContext,
    activePlugins: PluginManifest[]
  ): Promise<PreCompactResult> {
    const summaries: string[] = [];

    for (const plugin of activePlugins) {
      if (!plugin.hooks?.preCompact) continue;

      try {
        const result = await this.executeHookCommand(
          plugin.hooks.preCompact,
          ctx,
          plugin.dir
        );
        if (result?.summarySupplement) {
          summaries.push(result.summarySupplement);
        }
      } catch (err) {
        console.error(`[PluginHookRunner] Error in preCompact hook for ${plugin.id}:`, err);
      }
    }

    return {
      summarySupplement: summaries.length > 0 ? summaries.join("\n") : undefined
    };
  }

  /**
   * Helper to execute a hook script/command with JSON in/out via stdio.
   */
  private executeHookCommand(command: string, payload: any, cwd?: string): Promise<any> {
    return new Promise((resolve) => {
      // If hook is an internal identifier, resolve immediately
      if (command.startsWith("internal:")) {
        return resolve({});
      }

      const child = spawn(command, {
        shell: true,
        cwd: cwd || process.cwd(),
        env: {
          ...process.env,
          LOCAGENS_PLUGIN_HOOK_PAYLOAD: JSON.stringify(payload)
        }
      });

      let stdout = "";
      let stderr = "";

      child.stdout.on("data", (data) => {
        stdout += data.toString();
      });

      child.stderr.on("data", (data) => {
        stderr += data.toString();
      });

      child.on("close", (code) => {
        if (code !== 0) {
          console.warn(`[PluginHookRunner] Hook command exited with code ${code}: ${stderr}`);
          return resolve({});
        }

        try {
          const parsed = JSON.parse(stdout.trim());
          resolve(parsed);
        } catch {
          resolve({ output: stdout.trim() });
        }
      });

      child.on("error", (err) => {
        console.error(`[PluginHookRunner] Failed to spawn hook command "${command}":`, err);
        resolve({});
      });

      // Send payload via stdin
      try {
        child.stdin.write(JSON.stringify(payload));
        child.stdin.end();
      } catch {
        // Stdin write error ignored
      }
    });
  }
}
