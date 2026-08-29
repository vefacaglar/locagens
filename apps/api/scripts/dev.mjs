import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const childEnvironment = {
  ...process.env,
  LOCAGENS_API_TOKEN: process.env.LOCAGENS_API_TOKEN || "locagens-development-token-not-for-production-0000000000000000",
  LOCAGENS_DB_PATH: process.env.LOCAGENS_DB_PATH || "../../.locagens-dev/locagens.db"
};

// Keep the token in the child environment, never in a shell command or argv.
const tsxCli = fileURLToPath(import.meta.resolve("tsx/cli"));
const child = spawn(process.execPath, [tsxCli, "watch", "src/server.ts"], {
  env: childEnvironment,
  stdio: "inherit"
});

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  process.exit(code ?? 1);
});
