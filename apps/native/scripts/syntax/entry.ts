// The source highlighter (shiki 4 with its Oniguruma engine, the github-dark-default theme
// and fourteen grammars), bundled into one script a shell runs in its JavaScript engine
// (core/assets/generated/syntax.js, written by scripts/syntax/bundle.sh and handed out by the
// core, `highlight.rs`). Which grammar a file takes, the theme, the size past which a file
// stays plain and the reading of the answer are the core's; the script only colours.
// It defines `northroomHighlight(code, language, theme)`: a promise, settled once the grammars
// have compiled, of JSON with the theme's colour and, per line, each token's text, colour and
// style.
import "./atob";
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
  engine: createOnigurumaEngine(import("shiki/wasm")),
});

Object.assign(globalThis, {
  northroomHighlight: (code: string, language: string, themeName: string): Promise<string> =>
    highlighter.then((made) => {
      const result = made.codeToTokens(code, { lang: language, theme: themeName });
      return JSON.stringify({
        fg: result.fg,
        lines: result.tokens.map((line) =>
          line.map((token) => [token.content, token.color ?? "", token.fontStyle ?? 0]),
        ),
      });
    }),
});
