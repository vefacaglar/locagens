import fs from "node:fs";
import path from "node:path";

/**
 * Shared path/argument/output guards for the workspace tool layer. Every file
 * access must go through resolveInside so a tool call can never escape the
 * run's project directory.
 */

/** Resolves a workspace-relative path to an absolute one, refusing to escape. */
export function resolveInside(baseDir: string, relativePath: string): string {
  if (relativePath.includes("\0")) throw new Error("Access denied: path contains a null byte.");
  const canonicalBase = fs.realpathSync.native(baseDir);
  const absolutePath = path.resolve(canonicalBase, relativePath);
  const isInside = absolutePath === canonicalBase || absolutePath.startsWith(canonicalBase + path.sep);
  if (!isInside) {
    throw new Error(`Access denied: path '${relativePath}' is outside of the workspace directory.`);
  }
  return absolutePath;
}

function ensureRealTargetInside(baseDir: string, absolutePath: string, relativePath: string): string {
  const canonicalBase = fs.realpathSync.native(baseDir);
  const realTarget = fs.realpathSync.native(absolutePath);
  if (realTarget !== canonicalBase && !realTarget.startsWith(canonicalBase + path.sep)) {
    throw new Error(`Access denied: path '${relativePath}' resolves outside of the workspace directory.`);
  }
  return realTarget;
}

/** Resolves an existing read target and verifies the final symlink destination. */
export function resolveInsideForRead(baseDir: string, relativePath: string): string {
  const absolutePath = resolveInside(baseDir, relativePath);
  if (!fs.existsSync(absolutePath)) return absolutePath;
  return ensureRealTargetInside(baseDir, absolutePath, relativePath);
}

/**
 * Resolves a mutation target and refuses every symlink in its existing path
 * chain. This prevents an in-workspace link from redirecting a write/delete to
 * another part of the machine.
 */
export function resolveInsideForMutation(baseDir: string, relativePath: string): string {
  const canonicalBase = fs.realpathSync.native(baseDir);
  const absolutePath = resolveInside(canonicalBase, relativePath);
  const rel = path.relative(canonicalBase, absolutePath);
  let cursor = canonicalBase;
  for (const segment of rel.split(path.sep).filter(Boolean)) {
    cursor = path.join(cursor, segment);
    if (!fs.existsSync(cursor)) break;
    if (fs.lstatSync(cursor).isSymbolicLink()) {
      throw new Error(`Access denied: mutation path '${relativePath}' contains a symbolic link.`);
    }
  }
  return absolutePath;
}

/** Reads a file's current content, or null if it is missing/unreadable. */
export function readExistingFile(absolutePath: string): string | null {
  try {
    if (fs.existsSync(absolutePath) && fs.statSync(absolutePath).isFile()) {
      return fs.readFileSync(absolutePath, "utf-8");
    }
  } catch {
    /* ignore unreadable files */
  }
  return null;
}

/** Ensures a required path argument is present, returning it. */
export function requirePath(value: unknown, name = "path"): string {
  if (value === undefined || value === null) {
    throw new Error(`Missing parameter: ${name}`);
  }
  return String(value);
}

/** Caps command output so a noisy build doesn't blow up the context window. */
export function truncateOutput(output: string): string {
  const MAX = 20_000;
  if (output.length <= MAX) return output;
  return output.slice(0, MAX) + `\n... [output truncated, ${output.length - MAX} more characters]`;
}
