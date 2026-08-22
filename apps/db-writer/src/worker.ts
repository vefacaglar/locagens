import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { DatabaseSync } from "node:sqlite";
import { handleLine, initDB } from "./core.js";

function fatal(message: string): never {
  console.error(message);
  process.exit(1);
}

async function main(): Promise<void> {
  const dbPath = process.env.LOCAGENS_DB_PATH;
  if (!dbPath) {
    fatal("LOCAGENS_DB_PATH is required");
  }
  fs.mkdirSync(path.dirname(dbPath), { recursive: true });

  let db: DatabaseSync;
  try {
    db = new DatabaseSync(dbPath);
    initDB(db);
  } catch (err) {
    fatal(err instanceof Error ? err.message : String(err));
  }

  process.on("exit", () => {
    try {
      db.close();
    } catch {
      // Already closed
    }
  });

  const rl = readline.createInterface({ input: process.stdin });
  for await (const line of rl) {
    const res = handleLine(db, line);
    if (res) {
      process.stdout.write(JSON.stringify(res) + "\n");
    }
  }
}

main().catch((err) => {
  fatal(err instanceof Error ? err.message : String(err));
});
