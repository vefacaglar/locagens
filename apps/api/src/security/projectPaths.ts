import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { IProjectRepository } from "../database/repositories.js";

export function canonicalProjectPath(input: string): string {
  const candidate = input.trim();
  if (!candidate || candidate.includes("\0")) throw new Error("Invalid project path.");
  let canonical: string;
  try {
    canonical = fs.realpathSync.native(candidate);
  } catch {
    throw new Error("Project directory does not exist or cannot be accessed.");
  }
  if (!fs.statSync(canonical).isDirectory()) throw new Error("Project path must be a directory.");
  const filesystemRoot = path.parse(canonical).root;
  const home = fs.realpathSync.native(os.homedir());
  if (canonical === filesystemRoot || canonical === home) {
    throw new Error("Choose a project folder, not the filesystem root or your entire home directory.");
  }
  return canonical;
}

export function requireRegisteredProject(projectRepo: IProjectRepository, input: string): string {
  const canonical = canonicalProjectPath(input);
  const exact = projectRepo.get(canonical);
  if (exact) return canonical;

  // Compatibility for pre-hardening databases that stored a symlinked or
  // non-normalized spelling of the same trusted directory.
  const legacyMatch = projectRepo.list().some((project) => {
    try {
      return canonicalProjectPath(project.path) === canonical;
    } catch {
      return false;
    }
  });
  if (!legacyMatch) throw new Error("Project is not registered in Locagens.");
  return canonical;
}

