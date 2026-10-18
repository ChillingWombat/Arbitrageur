// Component render tests live under src/components/*.test.ts but import .tsx
// modules — `node --experimental-strip-types` can't transform JSX, so this
// runner bundles them through esbuild (already a Vite dependency) and
// executes each bundle with node:test. No-op when no component tests exist.
import { build } from "esbuild";
import { globSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, basename } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const tests = globSync("src/components/*.test.ts", { cwd: root });
if (tests.length === 0) process.exit(0);

const outdir = join(root, "node_modules/.cache/component-tests");
mkdirSync(outdir, { recursive: true });

await build({
  entryPoints: tests.map((t) => join(root, t)),
  bundle: true,
  format: "cjs",
  platform: "node",
  outdir,
  outExtension: { ".js": ".cjs" },
  logLevel: "silent",
});

let failed = false;
for (const t of tests) {
  const name = basename(t, ".test.ts") + ".test.cjs";
  try {
    execFileSync(process.execPath, [join(outdir, name)], { stdio: "inherit" });
  } catch {
    failed = true;
  }
}
process.exit(failed ? 1 : 0);
