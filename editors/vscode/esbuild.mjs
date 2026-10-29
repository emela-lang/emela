// Bundles the extension and its dependencies into out/extension.js, so the package does not
// carry node_modules. Type checking is left to `tsc --noEmit` (`npm run check`).

import * as esbuild from "esbuild";

const production = process.argv.includes("--production");
const watch = process.argv.includes("--watch");

const context = await esbuild.context({
  entryPoints: ["src/extension.ts"],
  bundle: true,
  outfile: "out/extension.js",
  format: "cjs",
  platform: "node",
  target: "node18",
  // Provided by VSCode at run time.
  external: ["vscode"],
  minify: production,
  sourcemap: !production,
  logLevel: "warning",
});

if (watch) {
  await context.watch();
} else {
  await context.rebuild();
  await context.dispose();
}
