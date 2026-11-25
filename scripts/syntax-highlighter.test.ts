import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { test } from "bun:test";

/** Colours `source` as a shell does: starts the highlight, then steps it until it answers. */
async function highlight(engine: object, source: string, language: string): Promise<string> {
  const number = await runInNewContext(
    `northroomHighlight(${JSON.stringify(source)}, ${JSON.stringify(language)}, "github-dark-default")`,
    engine,
  );
  for (;;) {
    const answer = runInNewContext(`northroomHighlightStep(${number})`, engine);
    if (answer !== null) return answer;
  }
}

const script = readFileSync(
  new URL("../apps/native/core/assets/generated/syntax.js", import.meta.url),
  "utf8",
);

test("the shipped highlighter colors source without an engine-provided base64 decoder", async () => {
  const engine = { WebAssembly, TextDecoder, TextEncoder };
  runInNewContext(script, engine);
  const source = "let answer = 42;\n// a color";
  const answer: { lines: [string, string, number][][] } = JSON.parse(
    await highlight(engine, source, "rust"),
  );
  assert.equal(answer.lines.map((line) => line.map(([text]) => text).join("")).join("\n"), source);
  assert.ok(new Set(answer.lines.flat().map(([, color]) => color)).size > 1);
});

test("a highlight coloured in steps is coloured as a whole file is", async () => {
  const engine = { WebAssembly, TextDecoder, TextEncoder };
  runInNewContext(script, engine);
  const source = "/* a comment\n   over lines */\nfn main() {\n    let text = \"two\nlines\";\n}";
  const answer: { lines: [string, string, number][][] } = JSON.parse(
    await highlight(engine, source, "rust"),
  );
  const comment = answer.lines[1][0][1];
  assert.equal(answer.lines[0][0][1], comment, "the comment's second line keeps its colour");
  assert.equal(answer.lines.length, 6);
});

test("the highlighter keeps the JavaScript engine's existing base64 decoder", async () => {
  const engine = { WebAssembly, TextDecoder, TextEncoder, atob };
  runInNewContext(script, engine);
  await highlight(engine, "{}", "json");
  assert.equal(engine.atob, atob);
});
