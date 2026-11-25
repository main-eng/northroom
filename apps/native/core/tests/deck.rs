//! Checks the PowerPoint reader (`deck::pptx`) against files other programs wrote:
//! `fixtures/deck/python-pptx.pptx`, made by python-pptx from its default template
//! (`fixtures/deck/make.py`), and `fixtures/deck/Pitch deck.pptx`, the sample deck the
//! sample-files script writes.

use northroom_core::deck::{
    pptx, Align, Anchor, Colour, Deck, Form, Item, ReadError, Shape, Slide, Source,
};
use northroom_core::geometry::Rect;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/deck")
        .join(name)
}

fn read(name: &str) -> Deck {
    pptx::read(&fixture(name)).expect("read")
}

fn shapes(slide: &Slide) -> Vec<&Shape> {
    slide
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Shape(shape) => Some(shape),
            Item::Picture(_) => None,
        })
        .collect()
}

/// A shape's text, its paragraphs joined by `|`.
fn words(shape: &Shape) -> String {
    let paragraphs = shape.text.iter().flat_map(|body| &body.paragraphs);
    let lines: Vec<String> = paragraphs
        .map(|paragraph| paragraph.runs.iter().map(|run| run.text.as_str()).collect())
        .collect();
    lines.join("|")
}

fn hex(digits: &str) -> Colour {
    Colour::hex(digits).expect("a colour")
}

#[test]
fn placeholders_take_their_place_and_look_from_the_layout_and_master() {
    let deck = read("python-pptx.pptx");
    assert_eq!(
        (deck.width, deck.height, deck.slides.len()),
        (720.0, 540.0, 4)
    );

    let title = shapes(&deck.slides[0]);
    assert_eq!(words(title[0]), "Harbor Monthly");
    // The slide's title gives no frame; the title layout's does.
    assert_eq!(title[0].frame, Rect::new(54.0, 167.75, 612.0, 115.75));
    let body = title[0].text.as_ref().expect("text");
    assert_eq!(body.anchor, Anchor::Middle);
    let heading = &body.paragraphs[0];
    assert_eq!((heading.align, heading.runs[0].size), (Align::Center, 44.0));
    // The subtitle's grey is the layout's: the text colour made lighter.
    assert_eq!(words(title[1]), "A coffee subscription");
    let subtitle = &title[1].text.as_ref().expect("text").paragraphs[0];
    assert_eq!(
        (subtitle.runs[0].size, subtitle.runs[0].colour),
        (32.0, hex("404040"))
    );

    let bullets = shapes(&deck.slides[1])[1].text.as_ref().expect("text");
    let first = &bullets.paragraphs[0];
    assert_eq!(
        (
            first.bullet.as_deref(),
            first.left,
            first.indent,
            first.runs[0].size
        ),
        (Some("•"), 27.0, -27.0, 32.0)
    );
    let second = &bullets.paragraphs[1];
    assert_eq!(
        (second.bullet.as_deref(), second.runs[0].size),
        (Some("–"), 28.0)
    );
    let marked = &second.runs[1];
    assert_eq!(
        (marked.text.as_str(), marked.bold, marked.colour),
        (" guaranteed", true, hex("1F6F4A"))
    );
}

#[test]
fn shapes_take_their_colours_from_their_own_fill_then_their_style() {
    let deck = read("python-pptx.pptx");
    let drawn = shapes(&deck.slides[2]);
    let forms: Vec<Form> = drawn.iter().map(|shape| shape.form).collect();
    // The arrow holds no text and has no outline here, so it is left out; the chevron holds
    // text and is a box. The last box sits in a group.
    assert!(matches!(
        forms[..],
        [
            Form::Box,
            Form::Rounded(_),
            Form::Oval,
            Form::Box,
            Form::Line { rising: true },
            Form::Box,
            Form::Box
        ]
    ));
    let accent = hex("4F81BD");
    // Styled by the theme: filled with accent 1, text in the light colour.
    assert_eq!(drawn[0].fill, Some(accent));
    let label = &drawn[0].text.as_ref().expect("text").paragraphs[0].runs[0];
    assert_eq!(
        (label.text.as_str(), label.colour),
        ("Styled box", Colour::WHITE)
    );
    // Its own fill and no line.
    assert_eq!(
        (drawn[1].fill, drawn[1].stroke),
        (Some(hex("F6F3EC")), None)
    );
    // No fill, the style's line.
    assert_eq!(drawn[2].fill, None);
    assert_eq!(drawn[2].stroke.map(|stroke| stroke.colour), Some(accent));
    assert_eq!(words(drawn[3]), "Next");
    let note = drawn[5].text.as_ref().expect("text");
    assert_eq!(
        (words(drawn[5]).as_str(), note.wraps),
        ("A text box", false)
    );
    assert_eq!(note.paragraphs[0].runs[0].size, 14.0);
    assert_eq!(drawn[6].frame, Rect::new(576.0, 216.0, 72.0, 72.0));
}

#[test]
fn a_table_is_a_box_for_each_cell_in_the_default_style() {
    let deck = read("python-pptx.pptx");
    let drawn = shapes(&deck.slides[3]);
    let cells: Vec<(String, Rect)> = drawn[1..]
        .iter()
        .map(|cell| (words(cell), cell.frame))
        .collect();
    assert_eq!(
        cells,
        [
            ("Plan".to_owned(), Rect::new(72.0, 144.0, 216.0, 36.0)),
            ("Price".to_owned(), Rect::new(288.0, 144.0, 216.0, 36.0)),
            ("Solo".to_owned(), Rect::new(72.0, 180.0, 216.0, 36.0)),
            ("$19".to_owned(), Rect::new(288.0, 180.0, 216.0, 36.0)),
            ("Pair".to_owned(), Rect::new(72.0, 216.0, 216.0, 36.0)),
            ("$33".to_owned(), Rect::new(288.0, 216.0, 216.0, 36.0)),
        ]
    );
    let accent = hex("4F81BD");
    let header = &drawn[1].text.as_ref().expect("text").paragraphs[0].runs[0];
    assert_eq!(drawn[1].fill, Some(accent));
    assert_eq!((header.bold, header.colour), (true, Colour::WHITE));
    // Bands under the header, each lighter than the accent and different from the other.
    let (first, second) = (
        drawn[3].fill.expect("a fill"),
        drawn[5].fill.expect("a fill"),
    );
    assert!(first != second && first != accent && second != accent);
}

#[test]
fn the_sample_deck_reads_with_its_pictures_backgrounds_and_text() {
    let deck = read("Pitch deck.pptx");
    assert_eq!(
        (deck.width, deck.height, deck.slides.len()),
        (960.0, 540.0, 6)
    );
    assert_eq!(
        deck.pictures,
        [
            "ppt/media/image1.jpeg",
            "ppt/media/image2.jpeg",
            "ppt/media/image3.png"
        ]
        .map(|name| Source::Entry(name.to_owned()))
    );
    // The third is a PNG, read out of the file by its name.
    let badge = deck.pictures[2]
        .read(&fixture("Pitch deck.pptx"))
        .expect("bytes");
    assert!(badge.starts_with(b"\x89PNG"));
    let missing = Source::Entry("ppt/media/none.png".to_owned());
    assert!(missing.read(&fixture("Pitch deck.pptx")).is_err());

    let cover = &deck.slides[0];
    assert_eq!(cover.ground, hex("1F6F4A"));
    let Item::Picture(photograph) = &cover.items[0] else {
        panic!("the cover starts with its photograph");
    };
    assert_eq!(photograph.frame, Rect::new(420.0, 0.0, 540.0, 540.0));
    let title = shapes(cover)[1];
    let run = &title.text.as_ref().expect("text").paragraphs[0].runs[0];
    assert_eq!(
        (run.text.as_str(), run.size, run.bold, run.colour),
        ("Harbor", 60.0, true, hex("F6F3EC"))
    );
    // The second slide sets no background; its master's shows.
    assert_eq!(deck.slides[1].ground, hex("F6F3EC"));
    let every_word: Vec<String> = deck
        .slides
        .iter()
        .flat_map(|slide| shapes(slide).into_iter().map(words))
        .filter(|words| !words.is_empty())
        .collect();
    assert_eq!(every_word.len(), 61);
}

#[test]
fn a_file_that_is_not_a_presentation_says_so() {
    let folder = tempfile::tempdir().expect("a folder");
    let path = folder.path().join("deck.pptx");
    std::fs::write(&path, b"not a zip").expect("write");
    assert!(matches!(pptx::read(&path), Err(ReadError::Damaged(_))));
    // A workbook is a readable package of another kind.
    let workbook =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workbooks/Budget.xlsx");
    assert_eq!(pptx::read(&workbook), Err(ReadError::NotItsKind));
}
