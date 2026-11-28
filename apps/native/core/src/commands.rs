//! The command palette (⌘K): searches the commands the current screen offers and ranks them
//! by `score`. The drive's commands come from `drive_commands`, and one for each running session
//! from `session_commands`.

use crate::strings::{self, key};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub title: String,
    /// Group heading: "Workspace" or "Sessions".
    pub group: String,
    /// Words that find the command besides its title.
    pub keywords: Vec<String>,
    /// Mac-style shortcut text (`menus::Shortcut::text` of the menu item), or empty.
    pub shortcut: String,
}

impl Command {
    fn new(id: &str, title: &str, group: &str, keywords: &[&str], shortcut: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            group: group.to_owned(),
            keywords: keywords.iter().map(|&keyword| keyword.to_owned()).collect(),
            shortcut: shortcut.to_owned(),
        }
    }

    /// The group heading in capitals, as displayed.
    pub fn heading(&self) -> String {
        self.group.to_uppercase()
    }
}

/// Commands offered while the drive is open: toggle hidden files (Finder's shortcut) and change
/// the drive folder. Titles are in `language`; keywords stay English.
pub fn drive_commands(language: &str) -> Vec<Command> {
    let group = strings::text(language, key::PALETTE_GROUP_WORKSPACE);
    vec![
        Command::new(
            "drive.toggle-hidden-files",
            strings::text(language, key::MENU_HIDDEN_FILES),
            group,
            &["dot", "dotfiles", "invisible"],
            "⌘⇧.",
        ),
        Command::new(
            "drive.change-folder",
            strings::text(language, key::MENU_CHANGE_DRIVE_FOLDER),
            group,
            &["drive", "folder", "workspace", "location", "move"],
            "",
        ),
        Command::new(
            "drive.start-terminal",
            strings::text(language, key::MENU_NEW_TERMINAL),
            group,
            &["terminal", "shell", "agent", "session", "start"],
            "⌘N",
        ),
    ]
}

/// One command for each session in `running`, in the sessions menu's order, under the
/// "Sessions" heading: titled by the session's task or terminal name and then its folder, and
/// found by its agent and terminal name too. Its id is its sessions menu row's
/// (`menus::session_entry_of`), and choosing it shows the session as that row does.
pub fn session_commands(running: &[crate::sessions::Running], language: &str) -> Vec<Command> {
    let group = strings::text(language, key::PALETTE_GROUP_SESSIONS);
    crate::sessions::grouped(running)
        .into_iter()
        .flat_map(|(_, listed)| listed)
        .map(|session| Command {
            id: crate::menus::session_entry_of(&session.id),
            title: format!(
                "{} · {}",
                crate::sessions::heading_of(session),
                session.folder
            ),
            group: group.to_owned(),
            keywords: [&session.agent, &session.name]
                .into_iter()
                .filter(|word| !word.is_empty())
                .cloned()
                .collect(),
            shortcut: String::new(),
        })
        .collect()
}

/// Score for a substring match. Every stronger kind of match scores higher.
const SUBSTRING_SCORE: u32 = 400;

/// Scores the needle's letters appearing in order. Fewer gaps score higher; 0 if any is missing.
fn subsequence_score(haystack: &[char], needle: &str) -> u32 {
    let mut position = 0;
    let mut gaps = 0usize;
    for wanted in needle.chars() {
        let Some(found) = haystack[position..]
            .iter()
            .position(|&letter| letter == wanted)
        else {
            return 0;
        };
        gaps += found;
        position += found + 1;
    }
    (40i64 - gaps as i64).max(1) as u32
}

/// Scores a text against a lowercased needle, best to worst: exact, prefix, word start,
/// substring, then in-order letters. Words start after a space, `-`, `_`, `:` or `/`.
pub fn score_text(text: &str, needle: &str) -> u32 {
    let value = text.to_lowercase();
    if value == needle {
        return 1000;
    }
    if value.starts_with(needle) {
        return 800;
    }
    if value
        .split(|letter: char| letter.is_whitespace() || "-_:/".contains(letter))
        .any(|word| word.starts_with(needle))
    {
        return 600;
    }
    if value.contains(needle) {
        return SUBSTRING_SCORE;
    }
    subsequence_score(&value.chars().collect::<Vec<_>>(), needle)
}

/// A command's score for `query`, from its title or else its keywords.
pub fn score(command: &Command, query: &str) -> u32 {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return 1;
    }
    let title = score_text(&command.title, &needle);
    if title >= SUBSTRING_SCORE {
        return title;
    }
    if command
        .keywords
        .iter()
        .any(|keyword| keyword.to_lowercase().starts_with(&needle))
    {
        return 300;
    }
    if command
        .keywords
        .iter()
        .any(|keyword| keyword.to_lowercase().contains(&needle))
    {
        return 200;
    }
    title
}

/// Matching commands, best first. Ties keep their original order.
pub fn rank(commands: &[Command], query: &str) -> Vec<Command> {
    let mut scored: Vec<(usize, u32, Command)> = commands
        .iter()
        .enumerate()
        .map(|(order, command)| (order, score(command, query), command.clone()))
        .filter(|(_, score, _)| *score > 0)
        .collect();
    scored.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    scored.into_iter().map(|(_, _, command)| command).collect()
}

/// Placeholder text for the empty search field.
pub fn placeholder(language: &str) -> &'static str {
    strings::text(language, key::PALETTE_PLACEHOLDER)
}

/// Key label shown at the end of the search row; it closes the palette.
pub const ESCAPE_KEY: &str = "Esc";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteKey {
    Down,
    Up,
    Return,
    Escape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAnswer {
    /// Redraw: the selection moved, or there was nothing to run.
    Redraw,
    /// Close the palette and run the command with this id.
    Run(String),
    Close,
}

/// A rectangle in points from the window's top-left corner, y downwards.
pub type Rect = crate::board::Frame;

/// An open palette: the commands offered when it opened, the matching ones, best first, and the
/// selected row, which ↑ and ↓ move with wraparound.
pub struct Palette {
    offered: Vec<Command>,
    rows: Vec<Command>,
    active: usize,
    panel: Rect,
    frames: Vec<Rect>,
}

impl Palette {
    /// Opens with every offered command listed and the first selected.
    pub fn open(offered: Vec<Command>) -> Self {
        let mut palette = Self {
            offered,
            rows: Vec::new(),
            active: 0,
            panel: Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            frames: Vec::new(),
        };
        palette.search("");
        palette
    }

    /// Ranks the offered commands for `query` and selects the best match.
    pub fn search(&mut self, query: &str) {
        self.rows = rank(&self.offered, query);
        self.active = 0;
    }

    pub fn rows(&self) -> &[Command] {
        &self.rows
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn key(&mut self, key: PaletteKey) -> PaletteAnswer {
        let count = self.rows.len();
        match key {
            PaletteKey::Down if count > 0 => self.active = (self.active + 1) % count,
            PaletteKey::Up if count > 0 => self.active = (self.active + count - 1) % count,
            PaletteKey::Down | PaletteKey::Up => self.active = 0,
            PaletteKey::Return => {
                return self
                    .rows
                    .get(self.active)
                    .map_or(PaletteAnswer::Redraw, |command| {
                        PaletteAnswer::Run(command.id.clone())
                    })
            }
            PaletteKey::Escape => return PaletteAnswer::Close,
        }
        PaletteAnswer::Redraw
    }

    /// Selects the row under the pointer. Returns whether the selection changed.
    pub fn point(&mut self, row: usize) -> bool {
        let changed = row < self.rows.len() && row != self.active;
        if changed {
            self.active = row;
        }
        changed
    }

    /// Lays out the palette in a `width` × `height` window. The panel is 448 pt wide (narrower
    /// in small windows) with its top 18% down. Below a 44 pt search row and a divider come
    /// 28 pt rows inset 4 pt, grouped under 14 pt headings with 4 pt above and below and 4 pt
    /// between groups. With no matches, a 64 pt band holds the message. The list scrolls past
    /// 320 pt.
    pub fn layout(&mut self, width: f64, height: f64) {
        let panel_width = PANEL_WIDTH.min(width - 32.0);
        let mut bottom = SEARCH_HEIGHT + 1.0 + 4.0;
        self.frames.clear();
        if self.rows.is_empty() {
            bottom += 24.0 + 16.0 + 24.0;
        } else {
            bottom = self.lay_out_rows(panel_width, bottom) + 4.0;
        }
        bottom += 4.0 + 2.0;
        self.panel = Rect {
            x: ((width - panel_width) / 2.0).round(),
            y: (height * 0.18).round(),
            width: panel_width,
            height: bottom.min(SEARCH_HEIGHT + 1.0 + LIST_MAX_HEIGHT + 2.0),
        };
        for frame in &mut self.frames {
            frame.x += self.panel.x + 1.0;
            frame.y += self.panel.y + 1.0;
        }
    }

    /// Frames the rows from `top` down, relative to the panel's top-left, leaving room for
    /// group headings. Returns the bottom of the last row.
    fn lay_out_rows(&mut self, panel_width: f64, top: f64) -> f64 {
        let mut bottom = top;
        let mut group = "";
        for row in &self.rows {
            if row.group != group {
                if !group.is_empty() {
                    bottom += 4.0;
                }
                bottom += 4.0 + HEADING_HEIGHT + 4.0;
                group = &row.group;
            }
            self.frames.push(Rect {
                x: 4.0,
                y: bottom,
                width: panel_width - 2.0 - 8.0,
                height: ROW_HEIGHT,
            });
            bottom += ROW_HEIGHT;
        }
        bottom
    }

    pub fn panel(&self) -> Rect {
        self.panel
    }

    /// The row's frame from the last layout, and whether a group heading sits above it.
    pub fn row_frame(&self, row: usize) -> Option<(Rect, bool)> {
        let frame = *self.frames.get(row)?;
        let heading = row == 0 || self.rows[row - 1].group != self.rows[row].group;
        Some((frame, heading))
    }
}

const PANEL_WIDTH: f64 = 448.0;
const SEARCH_HEIGHT: f64 = 44.0;
const HEADING_HEIGHT: f64 = 14.0;
const ROW_HEIGHT: f64 = 28.0;
const LIST_MAX_HEIGHT: f64 = 320.0;

// MARK: The palette's parts

use crate::board::card::Font;
use crate::dialog::layout::{Line, Measure};
use crate::geometry::Rect as Place;
use crate::theme::Token;

/// Search row, inside the 1 px border: a 16 pt magnifier 12 pt in and 14 pt down; the field
/// 8 pt after it, 12 pt down on a 20 px line, ending 8 pt before the "Esc" key; the key 12 pt
/// from the end, 13 pt down and 18 pt tall, its label 4 pt in and 2 pt down on a 14 px line.
const MAGNIFIER: f64 = 16.0;
const MAGNIFIER_IN: f64 = 12.0;
const MAGNIFIER_DOWN: f64 = 14.0;
const FIELD_DOWN: f64 = 12.0;
const FIELD_LINE: f64 = 20.0;
const FIELD_GAP: f64 = 8.0;
const KEY_END: f64 = 12.0;
const KEY_DOWN: f64 = 13.0;
const KEY_HEIGHT: f64 = 18.0;
const KEY_PADDING: f64 = 4.0;
const KEY_WORDS_DOWN: f64 = 2.0;
const KEY_LINE: f64 = 14.0;
const KEY_FONT: Font = Font::mono(10.0);
/// Key labels use the muted colour at 70%.
const KEY_OPACITY: f64 = 0.7;
/// Row title 8 pt in and 6 pt down on a 16 px line. The shortcut key sits 8 pt from the row's
/// end, 5 pt down, at least 12 pt after the title.
const ROW_FONT: Font = Font::sans(12.0, Font::REGULAR);
const ROW_WORDS_IN: f64 = 8.0;
const ROW_WORDS_DOWN: f64 = 6.0;
const ROW_LINE: f64 = 16.0;
const ROW_KEY_DOWN: f64 = 5.0;
const ROW_KEY_GAP: f64 = 12.0;
/// Group heading: 10 px medium capitals, 0.5 pt tracking, muted colour at 50%. Its line top is
/// 17 pt above the group's first row and its text 8 pt in.
const HEADING_FONT: Font = Font::sans(10.0, Font::MEDIUM);
const HEADING_TRACKING: f64 = 0.5;
const HEADING_OPACITY: f64 = 0.5;
const HEADING_ABOVE: f64 = 4.0 + HEADING_HEIGHT - 1.0;
/// "No matching commands", centred 28 pt below the divider on a 16 px line.
const NO_MATCH_DOWN: f64 = 4.0 + 24.0;

/// A key name drawn on a small background.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyCap {
    pub frame: Place,
    pub words: Line,
}

/// A group heading, positioned by the top-left of its line, with letter tracking.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub font: Font,
    pub color: Token,
    pub opacity: f64,
    pub tracking: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartsRow {
    pub id: String,
    /// Highlight area and hit area.
    pub frame: Place,
    pub lit: bool,
    pub heading: Option<Heading>,
    pub title: Line,
    pub key: Option<KeyCap>,
}

/// Frames for each part of the open palette, in window coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Parts {
    pub panel: Place,
    /// Clip for the rows, inside the panel border.
    pub clip: Place,
    pub magnifier: Place,
    /// Frame for the native search field.
    pub field: Place,
    pub escape: KeyCap,
    pub divider: Place,
    /// Message shown when nothing matches.
    pub empty: Option<Line>,
    pub rows: Vec<PartsRow>,
}

fn place(frame: Rect) -> Place {
    Place::new(frame.x, frame.y, frame.width, frame.height)
}

/// A key cap for `name` with its top-left at (`x`, `y`).
fn key_cap(name: &str, x: f64, y: f64, width: f64) -> KeyCap {
    let frame = Place::new(x, y, width, KEY_HEIGHT);
    KeyCap {
        frame,
        words: Line {
            opacity: KEY_OPACITY,
            ..Line::new(
                name,
                x + KEY_PADDING,
                y + KEY_WORDS_DOWN,
                width,
                KEY_LINE,
                KEY_FONT,
                Token::MutedForeground,
            )
        },
    }
}

fn key_width(name: &str, measure: &mut Measure) -> f64 {
    measure(name, KEY_FONT) + 2.0 * KEY_PADDING
}

impl Palette {
    /// The palette's parts from the last `layout`, with text in `language`.
    pub fn parts(&self, language: &str, measure: &mut Measure) -> Parts {
        let no_match = strings::text(language, key::PALETTE_NO_MATCH);
        let panel = place(self.panel);
        let (inner_x, inner_y) = (panel.x + 1.0, panel.y + 1.0);
        let escape_width = key_width(ESCAPE_KEY, measure);
        let escape = key_cap(
            ESCAPE_KEY,
            panel.max_x() - 1.0 - KEY_END - escape_width,
            inner_y + KEY_DOWN,
            escape_width,
        );
        let field_x = inner_x + MAGNIFIER_IN + MAGNIFIER + FIELD_GAP;
        let field = Place::new(
            field_x,
            inner_y + FIELD_DOWN,
            escape.frame.x - FIELD_GAP - field_x,
            FIELD_LINE,
        );
        let empty = self.rows.is_empty().then(|| {
            let width = measure(no_match, ROW_FONT);
            Line::new(
                no_match,
                panel.mid_x() - width / 2.0,
                inner_y + SEARCH_HEIGHT + 1.0 + NO_MATCH_DOWN,
                width + 1.0,
                ROW_LINE,
                ROW_FONT,
                Token::MutedForeground,
            )
        });
        let rows = (0..self.rows.len())
            .filter_map(|index| {
                let (frame, headed) = self.row_frame(index)?;
                let command = &self.rows[index];
                let frame = place(frame);
                let lit = index == self.active;
                let heading = headed.then(|| Heading {
                    text: command.heading(),
                    x: frame.x + ROW_WORDS_IN,
                    y: frame.y - HEADING_ABOVE,
                    font: HEADING_FONT,
                    color: Token::MutedForeground,
                    opacity: HEADING_OPACITY,
                    tracking: HEADING_TRACKING,
                });
                let mut right = frame.max_x() - ROW_WORDS_IN;
                let key = (!command.shortcut.is_empty()).then(|| {
                    let width = key_width(&command.shortcut, measure);
                    right -= width;
                    let key = key_cap(&command.shortcut, right, frame.y + ROW_KEY_DOWN, width);
                    right -= ROW_KEY_GAP;
                    key
                });
                let x = frame.x + ROW_WORDS_IN;
                let color = if lit {
                    Token::Foreground
                } else {
                    Token::MutedForeground
                };
                let title = Line::new(
                    command.title.as_str(),
                    x,
                    frame.y + ROW_WORDS_DOWN,
                    right - x,
                    ROW_LINE,
                    ROW_FONT,
                    color,
                );
                Some(PartsRow {
                    id: command.id.clone(),
                    frame,
                    lit,
                    heading,
                    title,
                    key,
                })
            })
            .collect();
        Parts {
            panel,
            clip: panel.outset(-1.0),
            magnifier: Place::new(
                panel.x + MAGNIFIER_IN + 1.0,
                inner_y + MAGNIFIER_DOWN,
                MAGNIFIER,
                MAGNIFIER,
            ),
            field,
            escape,
            divider: Place::new(inner_x, inner_y + SEARCH_HEIGHT, panel.width - 2.0, 1.0),
            empty,
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<Command> {
        vec![
            Command::new("a", "Open Folder", "Workspace", &["browse"], ""),
            Command::new(
                "b",
                "Show or Hide Hidden Files",
                "Workspace",
                &["dot", "dotfiles"],
                "⌘⇧.",
            ),
            Command::new("c", "cloudflare:wrangler", "Navigation", &[], ""),
        ]
    }

    fn ids(query: &str) -> Vec<String> {
        rank(&list(), query)
            .into_iter()
            .map(|command| command.id)
            .collect()
    }

    #[test]
    fn a_commands_shortcut_reads_as_its_menu_items_keys() {
        let drive = drive_commands("en");
        assert_eq!(drive[0].shortcut, crate::menus::HIDDEN_FILES_KEYS.text());
        assert_eq!(
            crate::menus::palette_command(&drive[0].id),
            Some(crate::menus::BarCommand::ToggleHiddenFiles)
        );
    }

    #[test]
    fn starting_a_terminal_is_a_command_with_its_menu_items_key() {
        let drive = drive_commands("en");
        let start = drive.last().expect("the command");
        assert_eq!(start.title, "New terminal");
        assert_eq!(start.shortcut, crate::menus::START_TERMINAL_KEYS.text());
        assert_eq!(
            crate::menus::palette_command(&start.id),
            Some(crate::menus::BarCommand::StartTerminal)
        );
    }

    #[test]
    fn each_running_session_is_a_command_found_by_task_folder_agent_or_name() {
        use crate::sessions::Running;
        use crate::terminal::activity::Activity;
        let session =
            |id: &str, name: &str, folder: &str, doing: &str, agent: &str, activity| Running {
                id: id.to_owned(),
                name: name.to_owned(),
                folder: folder.to_owned(),
                doing: doing.to_owned(),
                agent: agent.to_owned(),
                question: String::new(),
                activity,
            };
        let running = [
            session("s", "Terminal", "Drive", "", "", Activity::Working),
            session(
                "o",
                "Outreach",
                "Sales / Q4 leads",
                "Draft the emails",
                "Claude Code",
                Activity::NeedsYou,
            ),
        ];
        assert!(session_commands(&[], "en").is_empty());
        let sessions = session_commands(&running, "en");
        // In the sessions menu's order, each titled as its row and then its folder.
        assert_eq!(
            sessions
                .iter()
                .map(|command| (command.id.as_str(), command.title.as_str()))
                .collect::<Vec<_>>(),
            [
                ("session:o", "Draft the emails · Sales / Q4 leads"),
                ("session:s", "Terminal · Drive"),
            ]
        );
        assert_eq!(sessions[0].heading(), "SESSIONS");
        assert_eq!(sessions[0].keywords, ["Claude Code", "Outreach"]);
        assert_eq!(sessions[1].keywords, ["Terminal"]);
        assert_eq!(crate::menus::session_entry(&sessions[0].id), Some("o"));
        let mut offered = drive_commands("en");
        offered.extend(sessions);
        let found = |query: &str| -> Vec<String> {
            rank(&offered, query)
                .into_iter()
                .map(|command| command.id)
                .filter(|id| id.starts_with("session:"))
                .collect()
        };
        assert_eq!(found("emails"), ["session:o"], "by its task");
        assert_eq!(found("q4"), ["session:o"], "by its folder");
        assert_eq!(found("claude"), ["session:o"], "by its agent");
        assert_eq!(found("outr"), ["session:o"], "by its terminal's name");
        assert_eq!(found("drive"), ["session:s"]);
        // Opened with sessions offered, the palette heads them after the drive's commands.
        let mut palette = Palette::open(offered);
        palette.layout(1280.0, 800.0);
        assert_eq!(palette.rows().len(), 5);
        assert!(
            palette.row_frame(3).expect("row").1,
            "Sessions has a heading"
        );
        palette.search("emails");
        assert_eq!(
            palette.key(PaletteKey::Return),
            PaletteAnswer::Run("session:o".to_owned())
        );
    }

    #[test]
    fn the_keys_move_round_the_list_and_return_runs_the_lit_command() {
        let mut palette = Palette::open(list());
        assert_eq!((palette.rows().len(), palette.active()), (3, 0));
        assert_eq!(palette.key(PaletteKey::Up), PaletteAnswer::Redraw);
        assert_eq!(
            palette.active(),
            2,
            "up from the first goes round to the last"
        );
        palette.key(PaletteKey::Down);
        assert_eq!(palette.active(), 0);
        palette.key(PaletteKey::Down);
        assert_eq!(
            palette.key(PaletteKey::Return),
            PaletteAnswer::Run("b".to_owned())
        );
        assert_eq!(palette.key(PaletteKey::Escape), PaletteAnswer::Close);
        palette.search("zzz");
        assert_eq!(palette.key(PaletteKey::Down), PaletteAnswer::Redraw);
        assert_eq!(
            (palette.active(), palette.key(PaletteKey::Return)),
            (0, PaletteAnswer::Redraw),
            "nothing to run"
        );
        palette.search("");
        assert!(palette.point(2) && !palette.point(2) && !palette.point(7));
        assert_eq!(palette.active(), 2);
    }

    #[test]
    fn a_group_is_headed_in_capitals() {
        assert_eq!(drive_commands("en")[0].heading(), "WORKSPACE");
        assert_eq!(drive_commands("en")[0].title, "Show or Hide Hidden Files");
        assert_eq!(placeholder("en"), "Type a command…");
    }

    #[test]
    fn the_palette_sits_18_percent_down_with_each_group_under_its_heading() {
        let mut palette = Palette::open(list());
        palette.layout(1280.0, 800.0);
        assert_eq!(
            palette.panel(),
            Rect {
                x: 416.0,
                y: 144.0,
                width: 448.0,
                height: 44.0 + 1.0 + 4.0 + 22.0 + 56.0 + 4.0 + 22.0 + 28.0 + 4.0 + 6.0
            }
        );
        let (first, heading) = palette.row_frame(0).expect("row");
        assert_eq!(
            (first, heading),
            (
                Rect {
                    x: 421.0,
                    y: 144.0 + 1.0 + 49.0 + 22.0,
                    width: 438.0,
                    height: 28.0
                },
                true
            )
        );
        assert!(
            !palette.row_frame(1).expect("row").1,
            "the second Workspace row has no heading"
        );
        let (third, heading) = palette.row_frame(2).expect("row");
        assert!(heading && third.y == 144.0 + 1.0 + 49.0 + 22.0 + 56.0 + 4.0 + 22.0);
        let parts = palette.parts("en", &mut |text, _| text.chars().count() as f64 * 7.0);
        // "Esc" is 21 pt plus 4 pt padding each side, 12 pt from the panel's end; the field ends
        // 8 pt before it.
        assert_eq!(
            parts.escape.frame,
            Place::new(864.0 - 1.0 - 12.0 - 29.0, 158.0, 29.0, 18.0)
        );
        assert_eq!(
            parts.escape.words,
            Line {
                opacity: 0.7,
                ..Line::new(
                    "Esc",
                    826.0,
                    160.0,
                    29.0,
                    14.0,
                    KEY_FONT,
                    Token::MutedForeground
                )
            }
        );
        assert_eq!(
            parts.field,
            Place::new(453.0, 157.0, 822.0 - 8.0 - 453.0, 20.0)
        );
        assert_eq!(
            (parts.magnifier, parts.divider),
            (
                Place::new(429.0, 159.0, 16.0, 16.0),
                Place::new(417.0, 189.0, 446.0, 1.0)
            )
        );
        assert_eq!(parts.empty, None);
        let first = &parts.rows[0];
        assert!(first.lit && first.key.is_none());
        assert_eq!(
            first
                .heading
                .as_ref()
                .map(|heading| (heading.text.as_str(), heading.x, heading.y)),
            Some(("WORKSPACE", 429.0, 216.0 - 17.0))
        );
        assert_eq!(
            first.title,
            Line::new(
                "Open Folder",
                429.0,
                222.0,
                859.0 - 8.0 - 429.0,
                16.0,
                ROW_FONT,
                Token::Foreground
            )
        );
        // "⌘⇧." is three characters: a 29 pt key 8 pt from the row's end, the title 12 pt before.
        let second = &parts.rows[1];
        assert_eq!(
            second.key.as_ref().map(|key| key.frame),
            Some(Place::new(859.0 - 8.0 - 29.0, 249.0, 29.0, 18.0))
        );
        assert_eq!(
            (
                second.heading.is_none(),
                second.title.frame.width,
                second.title.color
            ),
            (true, 822.0 - 12.0 - 429.0, Token::MutedForeground)
        );
        palette.search("zzz");
        palette.layout(1280.0, 800.0);
        let parts = palette.parts("en", &mut |text, _| text.chars().count() as f64 * 7.0);
        // "No matching commands" is 20 characters, centred 28 pt below the divider.
        assert_eq!(
            parts.empty,
            Some(Line::new(
                "No matching commands",
                640.0 - 70.0,
                144.0 + 1.0 + 45.0 + 28.0,
                141.0,
                16.0,
                ROW_FONT,
                Token::MutedForeground
            ))
        );
        assert!(parts.rows.is_empty());
        palette.layout(300.0, 800.0);
        assert_eq!(
            (palette.panel().width, palette.panel().height),
            (268.0, 49.0 + 64.0 + 6.0)
        );
        assert_eq!(palette.row_frame(0), None);
    }

    #[test]
    fn ranks_commands_by_how_closely_the_query_matches() {
        assert_eq!(
            ids(""),
            ["a", "b", "c"],
            "an empty query lists everything in order"
        );
        assert_eq!(
            score_text("Show or Hide Hidden Files", "show or hide hidden files"),
            1000
        );
        assert_eq!(score_text("Show or Hide Hidden Files", "show"), 800);
        assert_eq!(score_text("Show or Hide Hidden Files", "hid"), 600);
        assert_eq!(
            score_text("cloudflare:wrangler", "wran"),
            600,
            "a colon starts a word"
        );
        assert_eq!(score_text("Show or Hide Hidden Files", "den fi"), 400);
        assert_eq!(
            score_text("Show or Hide Hidden Files", "shf"),
            40 - 18,
            "18 letters skipped"
        );
        assert_eq!(ids("dot"), ["b"], "a keyword finds what its title does not");
        assert_eq!(score(&list()[1], "otf"), 200, "inside a keyword");
        assert_eq!(
            ids("o"),
            ["a", "b", "c"],
            "a prefix, then a word start, then anywhere"
        );
        assert_eq!(
            ids("w"),
            ["c", "b", "a"],
            "a word start, a letter inside a word, then a keyword"
        );
        assert!(ids("zzz").is_empty());
    }
}
