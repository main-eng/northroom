import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";

const GIT_FILE_LIST_MAX_BUFFER_BYTES = 16 * 1024 * 1024;
const FILES_PER_PROCESS = 400;

const sourceExtensions = new Set([
  ".astro",
  ".css",
  ".html",
  ".js",
  ".json",
  ".jsonc",
  ".mjs",
  ".md",
  ".ts",
  ".yaml",
  ".yml",
]);

/** Packaged skill bundles, which the repo doesn't author. */
const unownedSourceTrees = [".agents/"];
// Build output, generated files and vendored upstream code. Prettier must not rewrite a vendor
// file even when it is listed by name.
const ignoredSources = [
  ".agents/**",
  ".claude/**",
  ".codex/**",
  "**/node_modules/**",
  "**/build/**/*",
  "**/dist/**",
  "**/coverage/**",
  "**/.astro/**",
  "**/generated/**",
  "**/public/**",
  "**/vendor/**",
  "**/*.d.ts",
].map((pattern) => new Bun.Glob(pattern));

const args = new Set(Bun.argv.slice(2));
const allFiles = args.has("--all");
const write = args.has("--write");

function git(...gitArgs: string[]): string[] {
  const output = execFileSync("git", gitArgs, {
    encoding: "utf8",
    maxBuffer: GIT_FILE_LIST_MAX_BUFFER_BYTES,
  });
  return output
    .split("\n")
    .map((file) => file.trim())
    .filter(Boolean);
}

/** Every tracked and untracked file with `--all`, otherwise the files the branch or tree changed. */
function listCandidateFiles(): string[] {
  if (allFiles) {
    return git("ls-files", "--cached", "--others", "--exclude-standard").filter((file) =>
      existsSync(file),
    );
  }

  const explicitBaseSha = process.env.QUALITY_BASE_SHA?.trim();
  const baseRef = process.env.GITHUB_BASE_REF;
  const hasExplicitBase =
    explicitBaseSha !== undefined &&
    /^[0-9a-f]{40}$/u.test(explicitBaseSha) &&
    !/^0+$/u.test(explicitBaseSha);
  const files = hasExplicitBase
    ? git("diff", "--name-only", "--diff-filter=ACMR", `${explicitBaseSha}...HEAD`)
    : baseRef
      ? git("diff", "--name-only", "--diff-filter=ACMR", `origin/${baseRef}...HEAD`)
      : [
          ...git("diff", "--name-only", "--diff-filter=ACMR", "HEAD"),
          ...git("diff", "--cached", "--name-only", "--diff-filter=ACMR"),
          ...git("ls-files", "--others", "--exclude-standard"),
        ];
  return [...new Set(files)];
}

function run(command: string, commandArgs: string[]): void {
  execFileSync(command, commandArgs, { stdio: "inherit" });
}

function runInBatches(files: readonly string[], runBatch: (batch: string[]) => void): void {
  for (let index = 0; index < files.length; index += FILES_PER_PROCESS) {
    runBatch(files.slice(index, index + FILES_PER_PROCESS));
  }
}

const files = listCandidateFiles().filter(
  (file) =>
    sourceExtensions.has(file.slice(file.lastIndexOf("."))) &&
    !unownedSourceTrees.some((tree) => file.startsWith(tree)) &&
    !ignoredSources.some((pattern) => pattern.match(file)),
);
if (files.length === 0) {
  console.log("Quality gate: no changed source files to check.");
  process.exit(0);
}

console.log(`Quality gate: checking ${files.length} source file(s).`);
// Prettier needs prettier-plugin-astro for `.astro`, and the root doesn't install it.
const formatFiles = files.filter((file) => !file.endsWith(".astro"));
runInBatches(formatFiles, (batch) => run("prettier", [write ? "--write" : "--check", ...batch]));
