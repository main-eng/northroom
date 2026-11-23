//! Layout of the settings window's page (`Page`). A scrolling column spans the window; the page
//! sits 64 pt down in it, centred, at most 768 pt wide and at least 48 pt from the window's
//! sides. It holds the title, the summary, then each section's heading over a box of rows. A
//! shell builds a view for each control (`Page::controls`), reports its size, and draws the
//! boxes, lines and text at these frames, in points from the page's top-left.

use super::{Control, Page, Row, Section, CONTROL_GAP};
use crate::board::card::Font;
use crate::dialog::layout::{Line, Measure, Size, UNBOUNDED};
use crate::geometry::Rect;
use crate::text::wrap::{self, Breaks};
use crate::theme::Token;

/// The page's top offset in the column, its side margin and its width limits. It is centred
/// and as wide as the column less the margins, within the limits.
const PAGE_TOP: f64 = 64.0;
const PAGE_SIDE: f64 = 48.0;
const PAGE_MAX: f64 = 768.0;
const PAGE_MIN: f64 = 200.0;

const TITLE_FONT: Font = Font::sans(20.0, Font::SEMIBOLD);
const TITLE_LINE: f64 = 28.0;
const TEXT_FONT: Font = Font::sans(14.0, Font::REGULAR);
const LABEL_FONT: Font = Font::sans(14.0, Font::MEDIUM);
const SMALL_FONT: Font = Font::sans(12.0, Font::REGULAR);
const LINE: f64 = 20.0;
const SMALL_LINE: f64 = 16.0;
/// Text alone at the right of a row sits in a line of page text, which is this tall.
const PAGE_LINE: f64 = 24.0;
/// Opacity of a section's description relative to its colour.
const QUIET: f64 = 0.7;
/// A box's inset for its rows: 16 pt of padding plus the 1 px edge.
const INSET: f64 = 17.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// The page's column, which scrolls.
    pub column: Rect,
    /// What scrolls in the column, and the page on it.
    pub document: Rect,
    pub page_frame: Rect,
    pub page: PageLayout,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageLayout {
    pub height: f64,
    /// The sections' boxes, and the lines between their rows.
    pub boxes: Vec<Rect>,
    pub dividers: Vec<Rect>,
    pub lines: Vec<Line>,
    /// Each control, in the order `Page::controls` lists them.
    pub controls: Vec<Placed>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placed {
    pub frame: Rect,
    /// Text set in a line of page text (Inter 16 on 24 px lines) rather than as a flex item, so it
    /// sits on the baseline as a lone `<span>` in a `<div>` does.
    pub page_line: bool,
}

/// Lays out `page` in a `width` by `height` settings window, with control sizes in
/// `Page::controls` order.
pub fn window(
    page: &Page,
    width: f64,
    height: f64,
    sizes: &[Size],
    measure: &mut Measure,
) -> Window {
    let column = Rect::new(0.0, 0.0, width, height);
    let content = (width - 2.0 * PAGE_SIDE).clamp(PAGE_MIN, PAGE_MAX);
    let page = lay_out(page, content, sizes, measure);
    Window {
        column,
        document: Rect::new(0.0, 0.0, width, page.height + 2.0 * PAGE_TOP),
        page_frame: Rect::new(
            ((width - content) / 2.0).max(0.0).round(),
            PAGE_TOP,
            content,
            page.height,
        ),
        page,
    }
}

/// Controls placed side by side, `gap` apart and centred on a common line: their combined size
/// and each one's frame from the group's top-left.
struct Group {
    size: Size,
    members: Vec<Placed>,
}

impl Group {
    fn of(controls: &[&Control], sizes: &[Size]) -> Option<Self> {
        match (controls, sizes) {
            ([], _) | (_, []) => None,
            // Text alone at the right of a row is a span in a block, not a flex item.
            ([Control::Words { .. }], [size]) => {
                let size = Size {
                    width: size.width,
                    height: PAGE_LINE,
                };
                Some(Self {
                    size,
                    members: vec![Placed {
                        frame: Rect::new(0.0, 0.0, size.width, size.height),
                        page_line: true,
                    }],
                })
            }
            _ => {
                let height = sizes.iter().map(|size| size.height).fold(0.0, f64::max);
                let mut x = 0.0;
                let members = sizes
                    .iter()
                    .map(|size| {
                        let frame = Rect::new(
                            x,
                            ((height - size.height) / 2.0).round(),
                            size.width,
                            size.height,
                        );
                        x += size.width + CONTROL_GAP;
                        Placed {
                            frame,
                            page_line: false,
                        }
                    })
                    .collect();
                Some(Self {
                    size: Size {
                        width: x - CONTROL_GAP,
                        height,
                    },
                    members,
                })
            }
        }
    }

    fn one(size: Size) -> Self {
        Self {
            size,
            members: vec![Placed {
                frame: Rect::new(0.0, 0.0, size.width, size.height),
                page_line: false,
            }],
        }
    }

    /// Writes the controls' frames into `placed` from index `first`, with the group's top-left at
    /// (`x`, `y`).
    fn put(&self, x: f64, y: f64, first: usize, placed: &mut [Placed]) {
        for (index, member) in self.members.iter().enumerate() {
            placed[first + index] = Placed {
                frame: Rect::new(
                    x + member.frame.x,
                    y + member.frame.y,
                    member.frame.width,
                    member.frame.height,
                ),
                ..*member
            };
        }
    }
}

/// The control sizes in `Page::controls` order, consumed in that order.
struct Sizes<'a> {
    sizes: &'a [Size],
    next: usize,
}

/// The size of a control the shell gave no size for.
const NO_SIZE: Size = Size {
    width: 0.0,
    height: 0.0,
};

impl Sizes<'_> {
    /// The next `count` controls: the first one's index in page order, and their sizes. A missing
    /// size is `NO_SIZE`.
    fn take(&mut self, count: usize) -> (usize, Vec<Size>) {
        let first = self.next;
        self.next += count;
        (
            first,
            (first..self.next)
                .map(|index| self.sizes.get(index).copied().unwrap_or(NO_SIZE))
                .collect(),
        )
    }
}

/// A row's controls as one group, if it has any, and the first one's index in page order.
struct RowControls {
    group: Option<Group>,
    first: usize,
}

impl RowControls {
    fn size(&self) -> Size {
        self.group.as_ref().map_or(NO_SIZE, |group| group.size)
    }

    /// The group, if there is one, with its top-left at (`x`, `y`).
    fn put(&self, x: f64, y: f64, placed: &mut [Placed]) {
        if let Some(group) = &self.group {
            group.put(x, y, self.first, placed);
        }
    }
}

/// `lines` one under another from (`x`, `top`), each `height` tall.
fn stacked<'a>(
    lines: &'a [String],
    x: f64,
    top: f64,
    width: f64,
    height: f64,
    font: Font,
    color: Token,
) -> impl Iterator<Item = Line> + 'a {
    lines.iter().enumerate().map(move |(index, line)| {
        Line::new(
            line,
            x,
            top + index as f64 * height,
            width,
            height,
            font,
            color,
        )
    })
}

/// Lays a page out top to bottom, `width` wide; `y` is where the next item goes.
struct PageWriter<'m, 'f> {
    width: f64,
    y: f64,
    placed: PageLayout,
    measure: &'m mut Measure<'f>,
}

fn lay_out(page: &Page, width: f64, sizes: &[Size], measure: &mut Measure) -> PageLayout {
    let mut writer = PageWriter {
        width,
        y: 0.0,
        placed: PageLayout {
            controls: vec![Placed::default(); page.controls().count()],
            ..PageLayout::default()
        },
        measure,
    };
    let mut sizes = Sizes { sizes, next: 0 };
    writer.put_title(page);
    for section in &page.sections {
        writer.put_section(section, &mut sizes);
    }
    writer.placed.height = writer.y;
    writer.placed
}

impl PageWriter<'_, '_> {
    /// `text` wrapped per `breaks` onto lines no wider than `width` in `font`.
    fn wrap_lines(&mut self, text: &str, width: f64, breaks: Breaks, font: Font) -> Vec<String> {
        wrap::lines(text, width, breaks, &mut |line| (self.measure)(line, font))
    }

    /// The title and its summary.
    fn put_title(&mut self, page: &Page) {
        let width = self.width;
        self.placed.lines.push(Line::new(
            &page.title,
            0.0,
            self.y,
            width,
            TITLE_LINE,
            TITLE_FONT,
            Token::Foreground,
        ));
        self.y += TITLE_LINE;
        if let Some(summary) = &page.summary {
            self.y += 4.0;
            // The description wraps between words to the page's width.
            let lines = self.wrap_lines(summary, width, Breaks::Words, TEXT_FONT);
            self.placed.lines.extend(stacked(
                &lines,
                0.0,
                self.y,
                UNBOUNDED,
                LINE,
                TEXT_FONT,
                Token::MutedForeground,
            ));
            self.y += lines.len() as f64 * LINE;
        }
        self.y += 32.0;
    }

    /// A section's heading, then its box of rows.
    fn put_section(&mut self, section: &Section, sizes: &mut Sizes) {
        let action = section.action.as_ref().map(|_| sizes.take(1));
        let rows: Vec<(usize, Vec<Size>)> = section
            .rows
            .iter()
            .map(|row| sizes.take(row.controls.len()))
            .collect();
        self.put_section_heading(section, action.map(|(first, size)| (first, size[0])));
        let top = self.y;
        // The box's 1 px edge is above its first row and below its last.
        self.y += 1.0;
        for (index, (row, (first, row_sizes))) in section.rows.iter().zip(&rows).enumerate() {
            let controls: Vec<&Control> = row.controls.iter().collect();
            let controls = RowControls {
                group: Group::of(&controls, row_sizes),
                first: *first,
            };
            self.put_row(index, row, &controls);
        }
        self.y += 1.0;
        self.placed
            .boxes
            .push(Rect::new(0.0, top, self.width, self.y - top));
        self.y += 40.0;
    }

    /// The section's label and description, and its action, `(first, size)`, at the right.
    fn put_section_heading(&mut self, section: &Section, action: Option<(usize, Size)>) {
        let width = self.width;
        // The heading is as tall as the control beside it, with the label centred in it.
        let mut header = LINE;
        if let (None, Some((_, size))) = (&section.description, &action) {
            header = header.max(size.height);
        }
        self.placed.lines.push(Line::new(
            &section.label,
            0.0,
            self.y + (header - LINE) / 2.0,
            width,
            LINE,
            LABEL_FONT,
            Token::MutedForeground,
        ));
        if let Some(description) = &section.description {
            self.placed.lines.push(Line {
                opacity: QUIET,
                ..Line::new(
                    description,
                    0.0,
                    self.y + 24.0,
                    width,
                    SMALL_LINE,
                    SMALL_FONT,
                    Token::MutedForeground,
                )
            });
            header = 40.0;
        }
        if let Some((first, size)) = action {
            Group::one(size).put(
                width - size.width,
                self.y + (header - size.height) / 2.0,
                first,
                &mut self.placed.controls,
            );
        }
        self.y += header + 16.0;
    }

    /// A row's divider is its own top edge and takes 1 pt of the row. The first row has none.
    fn divide_from_above(&mut self, index: usize) {
        if index > 0 {
            self.placed
                .dividers
                .push(Rect::new(1.0, self.y, self.width - 2.0, 1.0));
            self.y += 1.0;
        }
    }

    /// A row's label at the left, centred on its controls at the right. The row is 32 pt taller
    /// than the taller of the two.
    fn put_row(&mut self, index: usize, row: &Row, controls: &RowControls) {
        self.divide_from_above(index);
        let size = controls.size();
        let room = self.width
            - 2.0 * INSET
            - if controls.group.is_some() {
                size.width + 16.0
            } else {
                0.0
            };
        let row_height = 32.0 + LINE.max(size.height);
        self.placed.lines.push(Line::new(
            &row.label,
            INSET,
            self.y + (row_height - LINE) / 2.0,
            room,
            LINE,
            LABEL_FONT,
            Token::Foreground,
        ));
        controls.put(
            self.width - INSET - size.width,
            self.y + (row_height - size.height) / 2.0,
            &mut self.placed.controls,
        );
        self.y += row_height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::typeset::test_measure::sevens;

    fn size(width: f64, height: f64) -> Size {
        Size { width, height }
    }

    /// A control that hands back `action`, for a test that only places it.
    fn slider(action: &str) -> Control {
        Control::Slider {
            action: action.to_owned(),
            value: 0.0,
        }
    }

    #[test]
    fn the_page_is_centred_at_most_768_wide_and_sits_64_down() {
        let page = Page::titled("Appearance");
        let wide = window(&page, 1400.0, 900.0, &[], &mut sevens);
        assert_eq!(wide.column, Rect::new(0.0, 0.0, 1400.0, 900.0));
        assert_eq!(wide.page_frame, Rect::new(316.0, 64.0, 768.0, 28.0 + 32.0));
        assert_eq!(wide.document, Rect::new(0.0, 0.0, 1400.0, 60.0 + 128.0));
        // A narrower window keeps 48 pt either side of the page, and the page at least 200 pt wide.
        let narrow = window(&page, 600.0, 600.0, &[], &mut sevens);
        assert_eq!(
            (narrow.page_frame.x, narrow.page_frame.width),
            (48.0, 504.0)
        );
        let narrowest = window(&page, 240.0, 600.0, &[], &mut sevens);
        assert_eq!(
            (narrowest.page_frame.x, narrowest.page_frame.width),
            (20.0, 200.0)
        );
    }

    #[test]
    fn a_row_centres_its_words_and_its_controls_at_the_right_and_rows_are_divided() {
        let mut page = Page::titled("Appearance");
        page.summary = Some("word ".repeat(30));
        let mut section = Section::new("Light");
        section.action = Some(slider("reset"));
        page.sections.push(section);
        page.row(Row::new("Accent").with(slider("name")));
        page.row(
            Row::new("Contrast")
                .with(slider("a"))
                .with(Control::words("on")),
        );
        let sizes = [
            size(60.0, 26.0),
            size(192.0, 26.0),
            size(32.0, 18.4),
            size(14.0, 20.0),
        ];
        let placed = lay_out(&page, 700.0, &sizes, &mut sevens);
        // 30 words of 28 and a space: 20 on the first line of 700.
        let summary: Vec<&Line> = placed
            .lines
            .iter()
            .filter(|line| line.text.starts_with("word"))
            .collect();
        assert_eq!(summary.len(), 2);
        assert_eq!(
            (summary[1].frame.y, summary[1].frame.width),
            (32.0 + 20.0, UNBOUNDED)
        );
        // The heading is as tall as Reset, which sits at the right of it.
        let heading = 28.0 + 4.0 + 40.0 + 32.0;
        assert_eq!(
            placed.controls[0].frame,
            Rect::new(640.0, heading, 60.0, 26.0)
        );
        let label = placed
            .lines
            .iter()
            .find(|line| line.text == "Light" && line.font == LABEL_FONT)
            .expect("the heading");
        assert_eq!(label.frame.y, heading + 3.0);
        // The first row: 32 pt around its label, with its control centred on it.
        let top = heading + 26.0 + 16.0;
        assert_eq!(
            placed.controls[1].frame,
            Rect::new(700.0 - 17.0 - 192.0, top + 1.0 + 16.0, 192.0, 26.0)
        );
        let accent = placed
            .lines
            .iter()
            .find(|line| line.text == "Accent")
            .expect("the label");
        assert_eq!(
            (accent.frame.y, accent.frame.width, accent.color),
            (
                top + 1.0 + 19.0,
                700.0 - 34.0 - 192.0 - 16.0,
                Token::Foreground
            )
        );
        // The second row is under its dividing line; its controls side by side, 8 apart.
        assert_eq!(placed.dividers, [Rect::new(1.0, top + 59.0, 698.0, 1.0)]);
        let second = top + 60.0;
        assert_eq!(
            placed.controls[2].frame,
            Rect::new(700.0 - 17.0 - 54.0, second + 17.0, 32.0, 18.4)
        );
        assert_eq!(
            placed.controls[3].frame,
            Rect::new(700.0 - 17.0 - 14.0, second + 16.0, 14.0, 20.0)
        );
        assert_eq!(
            placed.boxes,
            [Rect::new(0.0, top, 700.0, 1.0 + 58.0 + 1.0 + 52.0 + 1.0)]
        );
        assert_eq!(placed.height, top + 113.0 + 40.0);
    }

    #[test]
    fn words_alone_stand_in_a_page_line() {
        let mut page = Page::titled("Appearance");
        page.sections.push(Section::new("Theme"));
        page.row(Row::new("Theme").with(Control::words("Dark")));
        let placed = lay_out(&page, 600.0, &[size(14.0, 20.0)], &mut sevens);
        let top = 28.0 + 32.0 + 20.0 + 16.0;
        assert_eq!(
            placed.controls[0],
            Placed {
                frame: Rect::new(600.0 - 17.0 - 14.0, top + 1.0 + 16.0, 14.0, 24.0),
                page_line: true
            }
        );
    }
}
