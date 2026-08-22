import { build } from "esbuild";

await build({
  entryPoints: ["src/worker.ts"],
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node22",
  outfile: "dist/db-writer.cjs"
});
