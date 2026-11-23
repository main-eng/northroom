import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { test } from "bun:test";

const script = readFileSync(
  new URL("../apps/native/core/assets/generated/syntax.js", import.meta.url),
  "utf8",
);

test("the shipped highlighter colors source without an engine-provided base64 decoder", async () => {
  const engine = { WebAssembly, TextDecoder, TextEncoder };
  runInNewContext(script, engine);
  const source = "let answer = 42;\n// a color";
  const answer: { lines: [string, string, number][][] } = JSON.parse(
    await runInNewContext(
      `northroomHighlight(${JSON.stringify(source)}, "rust", "github-dark-default")`,
      engine,
    ),
  );
  assert.equal(answer.lines.map((line) => line.map(([text]) => text).join("")).join("\n"), source);
  assert.ok(new Set(answer.lines.flat().map(([, color]) => color)).size > 1);
});

test("the highlighter keeps the JavaScript engine's existing base64 decoder", async () => {
  const engine = { WebAssembly, TextDecoder, TextEncoder, atob };
  runInNewContext(script, engine);
  await runInNewContext('northroomHighlight("{}", "json", "github-dark-default")', engine);
  assert.equal(engine.atob, atob);
});
