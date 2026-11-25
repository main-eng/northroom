//! Syntax highlighting for files shown as source, using shiki 4 with its Oniguruma engine, the
//! github-dark-default theme and fourteen grammars, whatever the app's theme.
//!
//! shiki is a JavaScript TextMate-grammar engine, so a shell runs it in the platform's
//! JavaScript engine. It evaluates [`SCRIPT`] once, then evaluates [`call`]'s expression per
//! file, steps the highlight it settles with a few milliseconds at a time between frames, and
//! passes the answer to [`runs`]. Grammar choice, the theme, the size limit
//! and how the result is read all live here. `scripts/syntax/bundle.sh` generates the script
//! (`core/assets/generated/syntax.js`).

use serde::Deserialize;

/// The highlighter. Evaluating it defines `northroomHighlight(code, language, theme)`, which
/// returns a promise, settled once its grammars compile, of a highlight's number;
/// `northroomHighlightStep(number)` colours the next few milliseconds of lines and returns null
/// until the last, then the answer for [`runs`]; `northroomHighlightDrop(number)` forgets one. It needs no engine globals and
/// brings its own `atob` where the engine lacks one.
pub const SCRIPT: &str = include_str!("../assets/generated/syntax.js");

/// The only theme the script carries.
const THEME: &str = "github-dark-default";

/// Files longer than this stay plain. Counted in UTF-16 units, as JavaScript string length is.
const LIMIT_UTF16_UNITS: usize = 100_000;

const LANGUAGES: [&str; 14] = [
    "javascript",
    "typescript",
    "json",
    "bash",
    "css",
    "html",
    "python",
    "rust",
    "go",
    "yaml",
    "markdown",
    "sql",
    "tsx",
    "jsx",
];

/// Aliases for grammar names.
const ALIASES: [(&str, &str); 15] = [
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("ts", "typescript"),
    ("mts", "typescript"),
    ("sh", "bash"),
    ("shell", "bash"),
    ("zsh", "bash"),
    ("console", "bash"),
    ("py", "python"),
    ("rs", "rust"),
    ("yml", "yaml"),
    ("md", "markdown"),
    ("jsonc", "json"),
    ("golang", "go"),
];

/// The grammar for `extension` (`list::name_extension`), ignoring case and surrounding spaces.
/// `None` when the script has no grammar for it.
pub fn language(extension: &str) -> Option<&'static str> {
    let lower = extension.trim().to_lowercase();
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map_or(lower.as_str(), |(_, name)| name);
    LANGUAGES.iter().copied().find(|language| *language == name)
}

/// The JavaScript expression that highlights `code` from a file with `extension`. It evaluates
/// to a promise of the highlight's number, which the shell steps to the answer for [`runs`]. `None` when the file stays plain: no grammar, or
/// more than 100,000 characters.
pub fn call(code: &str, extension: &str) -> Option<String> {
    let language = language(extension)?;
    if code.encode_utf16().count() > LIMIT_UTF16_UNITS {
        return None;
    }
    let code = serde_json::to_string(code).expect("a string is written as JSON");
    Some(format!(
        "northroomHighlight({code}, \"{language}\", \"{THEME}\")"
    ))
}

/// An sRGB colour with straight alpha, channels 0–255.
pub type Rgba = [u8; 4];

/// A run of highlighted text. `color` is `None` where the theme gives none, and the shell uses
/// its foreground.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub color: Option<Rgba>,
    pub italic: bool,
    pub bold: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// The script's answer: the theme's text colour and each line's tokens as `[text, colour,
/// style]`. Colour is `#rrggbb` or `#rrggbbaa`, empty for none; style is shiki's bits.
#[derive(Deserialize)]
struct Answer {
    fg: String,
    lines: Vec<Vec<(String, String, u32)>>,
}

/// shiki font style bits.
const ITALIC: u32 = 1;
const BOLD: u32 = 2;
const UNDERLINE: u32 = 4;
const STRIKETHROUGH: u32 = 8;

/// Converts the script's `answer` into runs: each token with its colour (the theme's text
/// colour if none) and style, joined by plain `\n` (so `\r\n` becomes `\n`). `None` when
/// `answer` isn't the script's JSON.
pub fn runs(answer: &str) -> Option<Vec<Run>> {
    let answer: Answer = serde_json::from_str(answer).ok()?;
    let foreground = rgba(&answer.fg);
    let mut runs = Vec::new();
    for (index, line) in answer.lines.iter().enumerate() {
        if index > 0 {
            runs.push(Run {
                text: "\n".to_owned(),
                color: None,
                italic: false,
                bold: false,
                underline: false,
                strikethrough: false,
            });
        }
        runs.extend(line.iter().map(|(text, color, style)| Run {
            text: text.clone(),
            color: rgba(color).or(foreground),
            italic: style & ITALIC != 0,
            bold: style & BOLD != 0,
            underline: style & UNDERLINE != 0,
            strikethrough: style & STRIKETHROUGH != 0,
        }));
    }
    Some(runs)
}

/// Parses `#rrggbb` or `#rrggbbaa`, with the `#` optional and hex digits in either case.
fn rgba(hex: &str) -> Option<Rgba> {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    if !(digits.len() == 6 || digits.len() == 8)
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if digits.len() == 8 {
            channel(6)?
        } else {
            u8::MAX
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The script's answers for each sample, recorded in JavaScriptCore, as
    /// `{ extension, code, answer }`. `answer` is null where the file stayed plain.
    const FIXTURES: &str = include_str!("highlight_fixtures.json");

    #[derive(Deserialize)]
    struct Fixture {
        extension: String,
        code: String,
        answer: Option<String>,
    }

    fn fixtures() -> Vec<Fixture> {
        serde_json::from_str(FIXTURES).expect("fixtures")
    }

    fn fixture(extension: &str) -> Fixture {
        fixtures()
            .into_iter()
            .find(|fixture| fixture.extension == extension)
            .expect("a fixture for the extension")
    }

    fn plain_run(text: &str, color: Rgba) -> Run {
        Run {
            text: text.to_owned(),
            color: Some(color),
            italic: false,
            bold: false,
            underline: false,
            strikethrough: false,
        }
    }

    #[test]
    fn a_file_takes_the_grammar_its_extension_names() {
        assert_eq!(language("rs"), Some("rust"));
        assert_eq!(language("ts"), Some("typescript"));
        assert_eq!(language("tsx"), Some("tsx"));
        assert_eq!(language("yml"), Some("yaml"));
        assert_eq!(language("golang"), Some("go"));
        assert_eq!(language("jsonc"), Some("json"));
        assert_eq!(language(" TS "), Some("typescript"));
        assert_eq!(language("Markdown"), Some("markdown"));
        for language_name in LANGUAGES {
            assert_eq!(language(language_name), Some(language_name));
        }
    }

    #[test]
    fn a_kind_with_no_grammar_stays_plain() {
        assert_eq!(language("txt"), None);
        assert_eq!(language("makefile"), None);
        assert_eq!(language(""), None);
        assert_eq!(call("Just words.\n", "txt"), None);
        // Every sample recorded as plain stays plain in the core.
        for fixture in fixtures() {
            assert_eq!(
                call(&fixture.code, &fixture.extension).is_none(),
                fixture.answer.is_none(),
                "{}",
                fixture.extension
            );
        }
    }

    #[test]
    fn the_call_names_the_grammar_and_theme_and_carries_the_code_as_a_string() {
        assert_eq!(
            call("let a = \"b\";\n\tc\u{1}", "TS").as_deref(),
            Some("northroomHighlight(\"let a = \\\"b\\\";\\n\\tc\\u0001\", \"typescript\", \"github-dark-default\")")
        );
        assert_eq!(
            call("", "rs").as_deref(),
            Some("northroomHighlight(\"\", \"rust\", \"github-dark-default\")")
        );
    }

    #[test]
    fn a_file_past_100000_characters_stays_plain() {
        assert!(call(&"a".repeat(100_000), "ts").is_some());
        assert!(call(&"a".repeat(100_001), "ts").is_none());
        // Counted in UTF-16 units: a character outside the BMP counts as two.
        assert!(call(&"😀".repeat(50_000), "ts").is_some());
        assert!(call(&format!("{}a", "😀".repeat(50_000)), "ts").is_none());
        // Three bytes in UTF-8 but one unit.
        assert!(call(&"é".repeat(100_000), "ts").is_some());
    }

    #[test]
    fn every_fixture_reads_back_to_the_text_shiki_cut() {
        let fixtures = fixtures();
        assert!(fixtures.len() >= 10);
        for fixture in fixtures {
            let Some(answer) = fixture.answer else {
                continue;
            };
            let runs = runs(&answer).expect("shiki's answer");
            let text: String = runs.iter().map(|run| run.text.as_str()).collect();
            // shiki splits lines at `\r\n` or `\n`; runs join them with `\n`.
            assert_eq!(
                text,
                fixture.code.replace("\r\n", "\n"),
                "{}",
                fixture.extension
            );
            assert!(
                runs.iter()
                    .all(|run| run.text == "\n" || run.color.is_some()),
                "{}",
                fixture.extension
            );
        }
    }

    #[test]
    fn rust_carries_shikis_colours() {
        let runs = runs(&fixture("rs").answer.expect("coloured")).expect("shiki's answer");
        let keyword = [0xFF, 0x7B, 0x72, 0xFF];
        let comment = [0x8B, 0x94, 0x9E, 0xFF];
        assert_eq!(runs[0], plain_run("use", keyword));
        assert!(runs.contains(&plain_run("/// Greets by name.", comment)));
        assert!(runs.contains(&plain_run("\"hello {name}\"", [0xA5, 0xD6, 0xFF, 0xFF])));
        assert!(runs.contains(&plain_run("greet", [0xD2, 0xA8, 0xFF, 0xFF])));
    }

    #[test]
    fn a_line_break_is_its_own_uncoloured_run() {
        let runs = runs(&fixture("py").answer.expect("coloured")).expect("shiki's answer");
        let breaks: Vec<&Run> = runs.iter().filter(|run| run.text.contains('\n')).collect();
        assert_eq!(breaks.len(), 6);
        assert!(breaks
            .iter()
            .all(|run| run.text == "\n" && run.color.is_none() && !run.bold && !run.italic));
    }

    #[test]
    fn markdown_carries_shikis_styles() {
        let runs = runs(&fixture("md").answer.expect("coloured")).expect("shiki's answer");
        let find = |text: &str| {
            runs.iter()
                .find(|run| run.text == text)
                .expect(text)
                .clone()
        };
        let heading = find("# Heading");
        assert_eq!(heading.color, Some([0x79, 0xC0, 0xFF, 0xFF]));
        assert!(heading.bold && !heading.italic && !heading.underline);
        assert!(find("**bold**").bold);
        let italic = find("_italic_");
        assert!(italic.italic && !italic.bold);
        assert!(find("*both*").italic);
        let link = find("https://example.com");
        assert!(link.underline && !link.bold && !link.strikethrough);
        assert_eq!(link.color, Some([0xE6, 0xED, 0xF3, 0xFF]));
    }

    #[test]
    fn an_empty_file_is_one_empty_line() {
        let empty = fixtures()
            .into_iter()
            .find(|fixture| fixture.code.is_empty())
            .expect("an empty file");
        assert_eq!(
            runs(&empty.answer.expect("coloured")).expect("shiki's answer"),
            vec![]
        );
    }

    #[test]
    fn a_token_without_a_colour_takes_the_themes_and_style_bits_combine() {
        let runs = runs(
            r##"{"fg":"#e6edf3","lines":[[["a","",3],["b","#11223344",12],["c","nope",0]],[]]}"##,
        )
        .expect("an answer");
        assert_eq!(
            runs,
            vec![
                Run {
                    text: "a".into(),
                    color: Some([0xE6, 0xED, 0xF3, 0xFF]),
                    italic: true,
                    bold: true,
                    underline: false,
                    strikethrough: false
                },
                Run {
                    text: "b".into(),
                    color: Some([0x11, 0x22, 0x33, 0x44]),
                    italic: false,
                    bold: false,
                    underline: true,
                    strikethrough: true
                },
                Run {
                    text: "c".into(),
                    color: Some([0xE6, 0xED, 0xF3, 0xFF]),
                    italic: false,
                    bold: false,
                    underline: false,
                    strikethrough: false
                },
                Run {
                    text: "\n".into(),
                    color: None,
                    italic: false,
                    bold: false,
                    underline: false,
                    strikethrough: false
                },
            ]
        );
        // A theme with no text colour leaves such a token to the shell's foreground.
        assert_eq!(
            runs_color(r#"{"fg":"","lines":[[["a","",0]]]}"#),
            vec![None]
        );
    }

    fn runs_color(answer: &str) -> Vec<Option<Rgba>> {
        runs(answer)
            .expect("an answer")
            .into_iter()
            .map(|run| run.color)
            .collect()
    }

    #[test]
    fn an_answer_that_is_not_the_scripts_is_refused() {
        assert_eq!(runs(""), None);
        assert_eq!(runs("null"), None);
        assert_eq!(runs(r##"{"fg":"#fff"}"##), None);
        assert_eq!(runs(r##"{"fg":"#ffffff","lines":[[["a"]]]}"##), None);
    }

    #[test]
    fn colours_are_six_or_eight_hex_digits() {
        assert_eq!(rgba("#E6EDF3"), Some([0xE6, 0xED, 0xF3, 0xFF]));
        assert_eq!(rgba("e6edf3"), Some([0xE6, 0xED, 0xF3, 0xFF]));
        assert_eq!(rgba("#e6edf380"), Some([0xE6, 0xED, 0xF3, 0x80]));
        assert_eq!(rgba("#fff"), None);
        assert_eq!(rgba("#e6edf3f"), None);
        assert_eq!(rgba("#+6edf3"), None);
        assert_eq!(rgba("#éédf3"), None);
        assert_eq!(rgba(""), None);
    }
}
