import { readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function collectTests(directory) {
  return readdirSync(directory, { withFileTypes: true })
    .flatMap((entry) => {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) return collectTests(entryPath);
      return entry.isFile() && entry.name.endsWith(".test.ts") ? [entryPath] : [];
    });
}

const tests = collectTests(path.join(packageRoot, "src")).sort();
if (tests.length === 0) {
  console.error("No API tests found.");
  process.exit(1);
}

const result = spawnSync(process.execPath, ["--import", "tsx", "--test", ...tests], {
  cwd: packageRoot,
  env: { ...process.env, NODE_ENV: "test" },
  stdio: "inherit"
});

if (result.error) throw result.error;
process.exit(result.status ?? 1);
