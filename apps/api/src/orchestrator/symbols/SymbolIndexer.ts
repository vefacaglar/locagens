import fs from "node:fs";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { CodeSymbol, SymbolKind } from "./types.js";
import { SymbolExtractor } from "./SymbolExtractor.js";

const execFileAsync = promisify(execFile);

interface CachedFileSymbols {
  mtime: number;
  symbols: CodeSymbol[];
}

const SUPPORTED_EXTENSIONS = new Set([
  "ts",
  "js",
  "tsx",
  "jsx",
  "vue",
  "py",
  "go",
  "rs",
  "mjs",
  "cjs",
  "json"
]);

export class SymbolIndexer {
  private cache = new Map<string, Map<string, CachedFileSymbols>>();

  /**
   * Searches symbols across the project matching the query and optional kind filter.
   */
  public async search(
    projectPath: string,
    query: string,
    kind?: SymbolKind,
    limit = 60
  ): Promise<CodeSymbol[]> {
    const allSymbols = await this.indexProject(projectPath);
    const q = query.trim().toLowerCase();

    return allSymbols
      .filter((sym) => {
        if (kind && sym.kind !== kind) return false;
        if (!q) return true;
        return (
          sym.name.toLowerCase().includes(q) ||
          sym.filePath.toLowerCase().includes(q) ||
          sym.preview.toLowerCase().includes(q)
        );
      })
      .slice(0, limit);
  }

  /**
   * Indexes or updates the symbol index for a project directory.
   */
  public async indexProject(projectPath: string): Promise<CodeSymbol[]> {
    let projectCache = this.cache.get(projectPath);
    if (!projectCache) {
      projectCache = new Map();
      this.cache.set(projectPath, projectCache);
    }

    const fileList = await this.listFiles(projectPath);
    const allSymbols: CodeSymbol[] = [];

    for (const relPath of fileList) {
      const ext = relPath.split(".").pop()?.toLowerCase() || "";
      if (!SUPPORTED_EXTENSIONS.has(ext)) continue;

      const fullPath = path.join(projectPath, relPath);
      try {
        const stat = fs.statSync(fullPath);
        const cached = projectCache.get(relPath);

        if (cached && cached.mtime === stat.mtimeMs) {
          allSymbols.push(...cached.symbols);
        } else {
          const content = fs.readFileSync(fullPath, "utf-8");
          const extracted = SymbolExtractor.extract(relPath, content);
          projectCache.set(relPath, {
            mtime: stat.mtimeMs,
            symbols: extracted
          });
          allSymbols.push(...extracted);
        }
      } catch {
        /* skip unreadable files */
      }
    }

    return allSymbols;
  }

  private async listFiles(projectPath: string): Promise<string[]> {
    try {
      const { stdout } = await execFileAsync("git", ["ls-files", "-co", "--exclude-standard"], {
        cwd: projectPath,
        encoding: "utf-8",
        maxBuffer: 8 * 1024 * 1024
      });
      return stdout
        .split("\n")
        .map((f) => f.trim())
        .filter(Boolean);
    } catch {
      // Fallback: directory traversal
      const files: string[] = [];
      const walk = (dir: string, base = "", depth = 0) => {
        if (depth > 6 || files.length > 500) return;
        const entries = fs.readdirSync(dir, { withFileTypes: true });
        for (const ent of entries) {
          if (
            ent.name.startsWith(".") ||
            ent.name === "node_modules" ||
            ent.name === "dist" ||
            ent.name === "build"
          ) {
            continue;
          }
          const rel = base ? `${base}/${ent.name}` : ent.name;
          if (ent.isDirectory()) {
            walk(path.join(dir, ent.name), rel, depth + 1);
          } else if (ent.isFile()) {
            files.push(rel);
          }
        }
      };
      try {
        walk(projectPath, "");
      } catch {
        /* ignore */
      }
      return files;
    }
  }

  public clearCache(projectPath?: string): void {
    if (projectPath) {
      this.cache.delete(projectPath);
    } else {
      this.cache.clear();
    }
  }
}
