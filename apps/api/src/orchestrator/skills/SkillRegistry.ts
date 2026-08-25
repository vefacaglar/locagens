import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { SkillSummary } from "@locagens/shared";
import { parseSkillMd } from "./parseSkillMd.js";
import type { DiscoveredSkill } from "./types.js";

const SKILL_FILE = "SKILL.md";
const PROJECT_SKILLS_SEGMENTS = [".locagens", "skills"] as const;

/**
 * Discovers Agent-Skills-compatible SKILL.md packs from the user skill root and
 * the active project's .locagens/skills directory. Project skills override user
 * skills with the same name. Paths stay inside allowlisted roots (symlink-safe).
 */
export class SkillRegistry {
  private readonly userRootOverride?: string;

  constructor(userRootOverride?: string) {
    this.userRootOverride = userRootOverride;
  }

  /** Absolute user-level skills directory (created on demand by ensureUserRoot). */
  userRoot(): string {
    if (this.userRootOverride) return path.resolve(this.userRootOverride);
    if (process.env.LOCAGENS_SKILLS_PATH) {
      return path.resolve(process.env.LOCAGENS_SKILLS_PATH);
    }
    const appDirName = "Locagens";
    if (process.platform === "darwin") {
      return path.join(os.homedir(), "Library", "Application Support", appDirName, "skills");
    }
    if (process.platform === "win32") {
      return path.join(process.env.APPDATA || path.join(os.homedir(), "AppData", "Roaming"), appDirName, "skills");
    }
    return path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "locagens", "skills");
  }

  /** Project skill root, or null when no project path is set. */
  projectRoot(projectPath?: string | null): string | null {
    if (!projectPath?.trim()) return null;
    try {
      const base = fs.realpathSync.native(projectPath.trim());
      return path.join(base, ...PROJECT_SKILLS_SEGMENTS);
    } catch {
      return null;
    }
  }

  /** Ensures the user skills directory exists; returns its absolute path. */
  ensureUserRoot(): string {
    const root = this.userRoot();
    fs.mkdirSync(root, { recursive: true });
    return fs.realpathSync.native(root);
  }

  /**
   * Ensures the project skills directory exists under a registered project.
   * Returns null when projectPath is missing or not a real directory.
   */
  ensureProjectRoot(projectPath: string): string | null {
    const root = this.projectRoot(projectPath);
    if (!root) return null;
    fs.mkdirSync(root, { recursive: true });
    try {
      return fs.realpathSync.native(root);
    } catch {
      return null;
    }
  }

  /**
   * Discovers skills for a run. User skills load first; project skills override
   * the same name. Invalid SKILL.md files are skipped.
   */
  discover(projectPath?: string | null): DiscoveredSkill[] {
    const byName = new Map<string, DiscoveredSkill>();

    for (const skill of this.scanRoot(this.userRoot(), "user")) {
      byName.set(skill.name, skill);
    }

    const projectRoot = this.projectRoot(projectPath);
    if (projectRoot) {
      for (const skill of this.scanRoot(projectRoot, "project")) {
        byName.set(skill.name, skill);
      }
    }

    return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  }

  /** Public catalog rows for Settings / prompt injection. */
  listSummaries(projectPath?: string | null): SkillSummary[] {
    return this.discover(projectPath).map(({ name, description, source, body }) => ({
      name,
      description,
      source,
      body
    }));
  }

  /**
   * Installs a SKILL.md into the user or project skills root. Parses frontmatter
   * for the folder name, writes under <root>/<name>/SKILL.md. Overwrites an
   * existing skill with the same name.
   */
  installSkillMd(
    target: "user" | "project",
    content: string,
    projectPath?: string | null
  ): SkillSummary {
    const raw = typeof content === "string" ? content : "";
    if (!raw.trim()) throw new Error("SKILL.md content is empty.");
    if (raw.length > 120_000) throw new Error("SKILL.md is too large (max ~120KB).");

    const parsed = parseSkillMd(raw);
    if (!parsed) {
      throw new Error(
        'Invalid SKILL.md. Need YAML frontmatter with name and description, e.g.\n---\nname: my-skill\ndescription: "..."\n---\n'
      );
    }

    const root =
      target === "user"
        ? this.ensureUserRoot()
        : (() => {
            if (!projectPath?.trim()) throw new Error("projectPath is required for project skills.");
            const dir = this.ensureProjectRoot(projectPath);
            if (!dir) throw new Error("Could not create project skills folder.");
            return dir;
          })();

    // Folder name follows the skill name (slug). Reject path segments.
    const folderName = parsed.name.includes("/") ? parsed.name.split("/").pop()! : parsed.name;
    if (!folderName || folderName.includes("..") || folderName.includes(path.sep)) {
      throw new Error("Invalid skill name.");
    }

    const skillDir = path.join(root, folderName);
    const skillFile = path.join(skillDir, SKILL_FILE);

    // Ensure the write stays inside the skills root (no symlink escape).
    const realRoot = fs.realpathSync.native(root);
    const resolvedDir = path.resolve(realRoot, folderName);
    if (resolvedDir !== realRoot && !resolvedDir.startsWith(realRoot + path.sep)) {
      throw new Error("Skill path escapes the skills directory.");
    }

    fs.mkdirSync(resolvedDir, { recursive: true });
    // Refuse if skillDir is a symlink pointing outside (mkdir may have followed).
    const realDir = fs.realpathSync.native(resolvedDir);
    if (realDir !== realRoot && !realDir.startsWith(realRoot + path.sep)) {
      throw new Error("Skill directory resolves outside the skills root.");
    }

    fs.writeFileSync(skillFile, raw.replace(/^\uFEFF/, ""), "utf-8");

    return {
      name: parsed.name,
      description: parsed.description,
      source: target,
      body: parsed.body
    };
  }

  /**
   * Deletes a skill folder under the user or project skills root.
   * Ensures the target path stays inside the allowlisted skills root.
   */
  deleteSkill(
    target: "user" | "project",
    name: string,
    projectPath?: string | null
  ): boolean {
    const rawName = typeof name === "string" ? name.trim().toLowerCase() : "";
    if (!rawName) throw new Error("Skill name is required.");
    if (
      !/^[a-z0-9][a-z0-9/_-]*$/.test(rawName) ||
      rawName.includes("..") ||
      rawName.startsWith("/") ||
      rawName.endsWith("/")
    ) {
      throw new Error("Invalid skill name.");
    }
    const folderName = rawName.includes("/") ? rawName.split("/").pop()! : rawName;
    if (!folderName) {
      throw new Error("Invalid skill name.");
    }

    const root =
      target === "user"
        ? this.userRoot()
        : (() => {
            if (!projectPath?.trim()) throw new Error("projectPath is required for project skills.");
            const dir = this.projectRoot(projectPath);
            if (!dir) throw new Error("Invalid project skills path.");
            return dir;
          })();

    if (!fs.existsSync(root)) return false;

    const realRoot = fs.realpathSync.native(root);
    const resolvedDir = path.resolve(realRoot, folderName);
    if (resolvedDir !== realRoot && !resolvedDir.startsWith(realRoot + path.sep)) {
      throw new Error("Skill path escapes the skills directory.");
    }

    if (!fs.existsSync(resolvedDir)) return false;

    const realDir = fs.realpathSync.native(resolvedDir);
    if (realDir !== realRoot && !realDir.startsWith(realRoot + path.sep)) {
      throw new Error("Skill directory resolves outside the skills root.");
    }

    fs.rmSync(resolvedDir, { recursive: true, force: true });
    return true;
  }

  private scanRoot(root: string, source: "user" | "project"): DiscoveredSkill[] {
    let entries: fs.Dirent[];
    try {
      if (!fs.existsSync(root) || !fs.statSync(root).isDirectory()) return [];
      const realRoot = fs.realpathSync.native(root);
      entries = fs.readdirSync(realRoot, { withFileTypes: true });
      return this.readEntries(realRoot, entries, source);
    } catch {
      return [];
    }
  }

  private readEntries(realRoot: string, entries: fs.Dirent[], source: "user" | "project"): DiscoveredSkill[] {
    const skills: DiscoveredSkill[] = [];
    for (const entry of entries) {
      if (!entry.isDirectory() && !entry.isSymbolicLink()) continue;
      if (entry.name.startsWith(".")) continue;

      const skillDir = path.join(realRoot, entry.name);
      let realDir: string;
      try {
        realDir = fs.realpathSync.native(skillDir);
      } catch {
        continue;
      }
      if (realDir !== realRoot && !realDir.startsWith(realRoot + path.sep)) continue;
      if (!fs.statSync(realDir).isDirectory()) continue;

      const skillFile = path.join(realDir, SKILL_FILE);
      let realFile: string;
      try {
        realFile = fs.realpathSync.native(skillFile);
      } catch {
        continue;
      }
      if (!realFile.startsWith(realDir + path.sep) && realFile !== path.join(realDir, SKILL_FILE)) {
        // File must live directly in the skill dir (no escape via symlink).
        if (path.dirname(realFile) !== realDir) continue;
      }
      if (!fs.statSync(realFile).isFile()) continue;

      let raw: string;
      try {
        raw = fs.readFileSync(realFile, "utf-8");
      } catch {
        continue;
      }

      const parsed = parseSkillMd(raw);
      if (!parsed) continue;

      skills.push({
        name: parsed.name,
        description: parsed.description,
        source,
        dir: realDir,
        body: parsed.body
      });
    }
    return skills;
  }
}

/** Look up a skill by name from a discovered list (case-insensitive). */
export function findSkill(skills: DiscoveredSkill[], name: string): DiscoveredSkill | undefined {
  const key = String(name || "").trim().toLowerCase();
  if (!key) return undefined;
  return skills.find(s => s.name === key);
}
