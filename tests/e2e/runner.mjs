#!/usr/bin/env node
/**
 * Arbitrageur 4-Tier E2E Test Suite Runner
 * Executes all 4 tiers of requirement-driven tests and prints a comprehensive summary.
 */

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");

console.log("\x1b[1;34m======================================================================\x1b[0m");
console.log("\x1b[1;36m       ARBITRAGEUR E2E TEST SUITE — 4-TIER REQUIREMENT VERIFICATION    \x1b[0m");
console.log("\x1b[1;34m======================================================================\x1b[0m\n");

const testFiles = [
  "tests/e2e/tier1_feature_coverage.test.ts",
  "tests/e2e/tier2_boundary_corner.test.ts",
  "tests/e2e/tier3_cross_feature.test.ts",
  "tests/e2e/tier4_real_world_scenarios.test.ts",
];

const child = spawn(
  process.execPath,
  ["--experimental-strip-types", "--test", ...testFiles],
  {
    cwd: repoRoot,
    stdio: "inherit",
  }
);

child.on("exit", (code) => {
  console.log("\n\x1b[1;34m======================================================================\x1b[0m");
  if (code === 0) {
    console.log("\x1b[1;32m       ALL 4 TIERS PASSED PERFECTLY (100% SUCCESS RATE)               \x1b[0m");
    console.log("\x1b[1;32m       - Tier 1: Feature Coverage (R1, R2, R3, R4)             [PASS] \x1b[0m");
    console.log("\x1b[1;32m       - Tier 2: Boundary & Corner Cases                       [PASS] \x1b[0m");
    console.log("\x1b[1;32m       - Tier 3: Cross-Feature Combinations                    [PASS] \x1b[0m");
    console.log("\x1b[1;32m       - Tier 4: Real-World Application Scenarios              [PASS] \x1b[0m");
  } else {
    console.log("\x1b[1;31m       E2E TEST RUN FAILED (EXIT CODE " + code + ")           \x1b[0m");
  }
  console.log("\x1b[1;34m======================================================================\x1b[0m\n");
  process.exit(code ?? 1);
});
