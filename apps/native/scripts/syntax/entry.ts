// The source highlighter (shiki 4 with its Oniguruma engine, the github-dark-default theme
// and fourteen grammars), bundled into one script a shell runs in its JavaScript engine
// (core/assets/generated/syntax.js, written by scripts/syntax/bundle.sh and handed out by the
// core, `highlight.rs`). Which grammar a file takes, the theme, the size past which a file
// stays plain and the reading of the answer are the core's; the script only colours.
// It defines `northroomHighlight(code, language, theme)`: a promise, settled once the grammars
// have compiled, of a highlight's number. `northroomHighlightStep(number)` then colours the
// next stretch of lines, for about `STEP_MS`, and returns null while lines remain, else the JSON
// answer: the theme's colour and, per line, each token's text, colour and style. A shell runs
// one step at a time between frames, so a long file never holds its window still.
// `northroomHighlightDrop(number)` forgets a highlight no longer wanted.
import "./atob";
import onigBase64 from "./onig";
import { createHighlighterCore } from "shiki/core";
import { createOnigurumaEngine } from "shiki/engine/oniguruma";
import theme from "shiki/themes/github-dark-default.mjs";
import javascript from "shiki/langs/javascript.mjs";
import typescript from "shiki/langs/typescript.mjs";
import json from "shiki/langs/json.mjs";
import bash from "shiki/langs/bash.mjs";
import css from "shiki/langs/css.mjs";
import html from "shiki/langs/html.mjs";
import python from "shiki/langs/python.mjs";
import rust from "shiki/langs/rust.mjs";
import go from "shiki/langs/go.mjs";
import yaml from "shiki/langs/yaml.mjs";
import markdown from "shiki/langs/markdown.mjs";
import sql from "shiki/langs/sql.mjs";
import tsx from "shiki/langs/tsx.mjs";
import jsx from "shiki/langs/jsx.mjs";

const highlighter = createHighlighterCore({
  themes: [theme],
  langs: [
    javascript,
    typescript,
    json,
    bash,
    css,
    html,
    python,
    rust,
    go,
    yaml,
    markdown,
    sql,
    tsx,
    jsx,
  ],
  engine: createOnigurumaEngine(Uint8Array.from(atob(onigBase64), (letter) => letter.charCodeAt(0))),
});

/** About how long one step colours for, in milliseconds. */
const STEP_MS = 4;
/** Lines coloured at once within a step: a step ends between batches. */
const BATCH_LINES = 2;

type Made = Awaited<typeof highlighter>;
type Highlight = {
  made: Made;
  lines: string[];
  at: number;
  language: string;
  theme: string;
  fg: string;
  state?: ReturnType<Made["getLastGrammarState"]>;
  coloured: [string, string, number][][];
};

const highlights = new Map<number, Highlight>();
let nextHighlight = 0;

/** Colours `highlight`'s next batch of lines, carrying the grammar's state across batches. */
function colourBatch(highlight: Highlight) {
  const batch = highlight.lines.slice(highlight.at, highlight.at + BATCH_LINES).join("\n");
  const result = highlight.made.codeToTokens(batch, {
    lang: highlight.language,
    theme: highlight.theme,
    grammarState: highlight.state,
  });
  highlight.state = result.grammarState;
  highlight.fg = result.fg ?? highlight.fg;
  for (const line of result.tokens) {
    highlight.coloured.push(
      line.map((token) => [token.content, token.color ?? "", token.fontStyle ?? 0]),
    );
  }
  highlight.at += BATCH_LINES;
}

Object.assign(globalThis, {
  northroomHighlight: (code: string, language: string, themeName: string): Promise<number> =>
    highlighter.then((made) => {
      const number = nextHighlight++;
      highlights.set(number, {
        made,
        lines: code.split(/\r?\n/),
        at: 0,
        language,
        theme: themeName,
        fg: "",
        coloured: [],
      });
      return number;
    }),
  northroomHighlightStep: (number: number): string | null => {
    const highlight = highlights.get(number);
    if (highlight === undefined) {
      return "";
    }
    const started = Date.now();
    do {
      colourBatch(highlight);
    } while (highlight.at < highlight.lines.length && Date.now() - started < STEP_MS);
    if (highlight.at < highlight.lines.length) {
      return null;
    }
    highlights.delete(number);
    return JSON.stringify({ fg: highlight.fg, lines: highlight.coloured });
  },
  northroomHighlightDrop: (number: number) => {
    highlights.delete(number);
  },
});
