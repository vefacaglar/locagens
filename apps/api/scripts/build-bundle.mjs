import { build } from "esbuild";

await build({
  entryPoints: ["src/server.ts"],
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node22",
  outfile: "dist/server.bundle.cjs",
  banner: {
    js: "const LOCAGENS_BUNDLE_IMPORT_META_URL = require('node:url').pathToFileURL(__filename).href;"
  },
  define: {
    "import.meta.url": "LOCAGENS_BUNDLE_IMPORT_META_URL"
  }
});
