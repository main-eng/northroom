import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "bun:test";

const QUALITY_SCRIPT = fileURLToPath(new URL("./check-quality.ts", import.meta.url));

interface ToolCall {
  readonly command: string;
  readonly args: readonly string[];
}

/** Runs the real gate with fake `git` and `prettier`, and returns what each was asked. */
function runQualityGate(changedFiles: readonly string[], flags: readonly string[]): ToolCall[] {
  const directory = mkdtempSync(join(tmpdir(), "northroom-quality-"));
  try {
    const binaries = join(directory, "bin");
    mkdirSync(binaries);
    const record = join(directory, "calls.log");
    writeFileSync(record, "");
    writeFileSync(
      join(binaries, "git"),
      `#!/bin/sh\nprintf '%s\\n' ${changedFiles.map((file) => `'${file}'`).join(" ")}\n`,
      { mode: 0o755 },
    );
    for (const command of ["prettier"]) {
      writeFileSync(
        join(binaries, command),
        `#!/bin/sh\nprintf '%s' '${command}' >> '${record}'\nfor argument in "$@"; do printf '\\t%s' "$argument" >> '${record}'; done\nprintf '\\n' >> '${record}'\n`,
        { mode: 0o755 },
      );
    }
    for (const file of changedFiles) {
      mkdirSync(join(directory, file, ".."), { recursive: true });
      writeFileSync(join(directory, file), "");
    }
    const result = spawnSync("bun", [QUALITY_SCRIPT, ...flags], {
      cwd: directory,
      env: { ...process.env, PATH: `${binaries}:${process.env.PATH}`, GITHUB_BASE_REF: "" },
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    return readFileSync(record, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [command, ...args] = line.split("\t");
        return { command: command!, args };
      });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("the gate checks the format of source files it owns", () => {
  const calls = runQualityGate(
    ["src/page.astro", "src/site.ts", "README.md", "image.png", ".agents/skills/rule.ts"],
    ["--all"],
  );
  assert.deepEqual(calls, [{ command: "prettier", args: ["--check", "src/site.ts", "README.md"] }]);
});

test("--write formats in place", () => {
  assert.deepEqual(runQualityGate(["a.ts"], ["--all", "--write"]), [
    { command: "prettier", args: ["--write", "a.ts"] },
  ]);
});

test("upstream vendor files are left alone", () => {
  const calls = runQualityGate(["src/page.ts", "assets/vendor/library.js"], ["--all"]);
  assert.deepEqual(calls, [{ command: "prettier", args: ["--check", "src/page.ts"] }]);
});
