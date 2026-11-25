//! Checks the readers of flowing documents against files other programs wrote. Each fixture is
//! in `fixtures/flow`, with the script that made it where a script did.

use northroom_core::deck::flow::{self, Block, Flow};
use northroom_core::deck::{doc, docx, odt, rtf, Align, Colour, Fit, Paragraph, Source, Spacing};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/flow")
        .join(name)
}

fn paragraphs(flow: &Flow) -> Vec<&Paragraph> {
    flow.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph),
            Block::Table(_) | Block::Picture { .. } | Block::PageBreak => None,
        })
        .collect()
}

fn text(paragraph: &Paragraph) -> String {
    paragraph.runs.iter().map(|run| run.text.as_str()).collect()
}

fn hex(digits: &str) -> Colour {
    Colour::hex(digits).expect("a colour")
}

/// Ten points a character at size 20, and in proportion at other sizes.
fn measure(text: &str, font: northroom_core::board::card::Font) -> f64 {
    text.chars().count() as f64 * font.size / 2.0
}

#[test]
fn a_word_document_reads_with_its_styles_lists_table_break_and_picture() {
    let flow = docx::read(&fixture("python-docx.docx")).expect("read");
    assert_eq!(flow.page.width, 612.0);
    assert_eq!(flow.page.margins, [90.0, 72.0, 90.0, 72.0]);
    let written = paragraphs(&flow);
    // The Title style: 26 points in a theme blue the style writes out.
    assert_eq!(text(written[0]), "Launch plan");
    assert_eq!(written[0].runs[0].size, 26.0);
    // Heading 1, based on Normal: bold, 14 points, space before, Normal's line spacing.
    let heading = written[1];
    assert_eq!(
        (heading.runs[0].bold, heading.runs[0].size, heading.before),
        (true, 14.0, Spacing::Points(24.0))
    );
    assert_eq!(heading.line, Spacing::Share(1.15));
    // Normal text, with one run made bold and green.
    let body = written[2];
    assert_eq!(
        text(body),
        "Our regulars already buy monthly. A subscription makes it easy."
    );
    assert_eq!(
        (body.runs[1].bold, body.runs[1].colour, body.runs[0].colour),
        (true, hex("1F6F4A"), Colour::BLACK)
    );
    assert_eq!(
        (written[3].align, written[3].runs[0].size),
        (Align::Center, 12.0)
    );
    let marks: Vec<Option<&str>> = written[4..8]
        .iter()
        .map(|paragraph| paragraph.bullet.as_deref())
        .collect();
    assert_eq!(marks, [Some("•"), Some("•"), Some("1."), Some("2.")]);
    assert_eq!((written[4].left, written[4].indent), (18.0, -18.0));

    let Some(Block::Table(table)) = flow
        .blocks
        .iter()
        .find(|block| matches!(block, Block::Table(_)))
    else {
        panic!("the table");
    };
    assert_eq!(table.columns, [144.0, 144.0, 144.0]);
    let cells: Vec<Vec<String>> = table
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| text(&cell.paragraphs[0])).collect())
        .collect();
    assert_eq!(cells, [["Plan", "Bags", "Price"], ["Solo", "1", "$19"]]);
    let tail: Vec<&Block> = flow.blocks.iter().rev().take(3).collect();
    assert!(matches!(
        tail[..],
        [
            Block::Picture {
                picture: 0,
                width: 144.0,
                height: 72.0
            },
            Block::Paragraph(_),
            Block::PageBreak
        ]
    ));
    assert_eq!(
        flow.pictures,
        [Source::Entry("word/media/image1.png".to_owned())]
    );

    let deck = flow::paginate(flow, &mut measure);
    assert_eq!((deck.fit, deck.slides.len()), (Fit::Width, 2));
}

#[test]
fn the_sample_letter_reads_with_its_sizes_colours_and_line_breaks() {
    let flow = docx::read(&fixture("Wholesale letter.docx")).expect("read");
    let written = paragraphs(&flow);
    let name = &written[1].runs[0];
    assert_eq!(
        (name.text.as_str(), name.size, name.bold, name.colour),
        ("HARBOR COFFEE", 29.0, true, hex("1B5E3A"))
    );
    // An empty first paragraph is as tall as its runs.
    assert_eq!(written[0].size, 16.0);
    let address = text(written[4]);
    assert_eq!(
        address,
        "Jess Laurent\nTidewater Café\n12 Fore Street\nPortland, ME 04101"
    );
    assert_eq!(written[8].left, 24.0);
    assert_eq!(written.len(), 15);
}

#[test]
fn a_file_that_is_not_a_word_document_says_so() {
    let workbook =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workbooks/Budget.xlsx");
    assert!(docx::read(&workbook).is_err());
}

#[test]
fn an_opendocument_text_reads_with_its_styles_lists_table_break_and_picture() {
    let flow = odt::read(&fixture("odfpy.odt")).expect("read");
    // A4 with 2.5 cm sides and 2 cm top and bottom.
    assert!((flow.page.width - 595.28).abs() < 0.01);
    assert!((flow.page.margins[0] - 70.87).abs() < 0.01);
    let written = paragraphs(&flow);
    let heading = &written[0].runs[0];
    assert_eq!(
        (
            heading.text.as_str(),
            heading.size,
            heading.bold,
            heading.colour
        ),
        ("Launch plan", 20.0, true, hex("1f6f4a"))
    );
    // The heading's style is based on Text, whose spacing it keeps.
    assert_eq!(written[0].after, Spacing::Points(6.0));
    assert_eq!(
        text(written[1]),
        "Our regulars already buy monthly.\nSecond line"
    );
    assert!(written[1].runs[1].bold && !written[1].runs[0].bold);
    assert_eq!(written[2].align, Align::Center);
    let marks: Vec<Option<&str>> = written[3..7]
        .iter()
        .map(|paragraph| paragraph.bullet.as_deref())
        .collect();
    assert_eq!(marks, [Some("–"), Some("–"), Some("1."), Some("2.")]);
    // Half a centimetre before the label and half for it.
    assert!((written[3].left - 28.35).abs() < 0.01);
    let Some(Block::Table(table)) = flow
        .blocks
        .iter()
        .find(|block| matches!(block, Block::Table(_)))
    else {
        panic!("the table");
    };
    assert_eq!((table.rows[0].len(), table.rows[0][0].span), (1, 2));
    assert_eq!(table.rows[0][0].fill, Some(hex("f6f3ec")));
    assert_eq!(text(&table.rows[1][1].paragraphs[0]), "$19");
    assert!(flow
        .blocks
        .iter()
        .any(|block| matches!(block, Block::PageBreak)));
    assert_eq!(
        flow.pictures,
        [Source::Entry("Pictures/green.png".to_owned())]
    );
    assert!(matches!(
        flow.blocks.last(),
        Some(Block::Picture { picture: 0, .. })
    ));
}

#[test]
fn the_sample_opendocument_letter_reads_like_its_word_twin() {
    let flow = odt::read(&fixture("Wholesale letter.odt")).expect("read");
    let word = docx::read(&fixture("Wholesale letter.docx")).expect("read");
    let words = |flow: &Flow| -> Vec<String> {
        paragraphs(flow)
            .into_iter()
            .map(|paragraph| {
                text(paragraph)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|line| !line.is_empty())
            .collect()
    };
    assert_eq!(words(&flow), words(&word));
    assert_eq!(paragraphs(&flow)[1].runs[0].colour, hex("1b5e3a"));
}

#[test]
fn the_sample_rich_text_letters_read_like_their_word_twin() {
    let word = docx::read(&fixture("Wholesale letter.docx")).expect("read");
    let words = |flow: &Flow| -> Vec<String> {
        paragraphs(flow)
            .into_iter()
            .map(|paragraph| {
                text(paragraph)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|line| !line.is_empty())
            .collect()
    };
    let plain = rtf::read(&fixture("Wholesale letter.rtf")).expect("read");
    assert_eq!(words(&plain), words(&word));
    let heading = &paragraphs(&plain)[1].runs[0];
    assert_eq!(
        (
            heading.text.as_str(),
            heading.bold,
            heading.colour,
            heading.size
        ),
        ("HARBOR COFFEE", true, hex("1b5e3a"), 29.0)
    );
    assert_eq!(plain.pictures, []);

    let package = rtf::read(&fixture("Wholesale letter.rtfd")).expect("read");
    assert_eq!(words(&package), words(&word));
    assert_eq!(
        package.pictures,
        [Source::File(
            fixture("Wholesale letter.rtfd").join("Attachment.png")
        )]
    );
    assert!(matches!(
        package.blocks[0],
        Block::Picture { picture: 0, .. }
    ));
}

#[test]
fn the_sample_word_97_letter_reads_like_its_word_twin() {
    let word = docx::read(&fixture("Wholesale letter.docx")).expect("read");
    let old = doc::read(&fixture("Wholesale letter.doc")).expect("read");
    let words = |flow: &Flow| -> Vec<String> {
        paragraphs(flow)
            .into_iter()
            .map(|paragraph| {
                text(paragraph)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|line| !line.is_empty())
            .collect()
    };
    assert_eq!(words(&old), words(&word));
    let written = paragraphs(&old);
    let heading = &written[1].runs[0];
    assert_eq!(
        (
            heading.text.as_str(),
            heading.size,
            heading.bold,
            heading.colour
        ),
        ("HARBOR COFFEE", 29.0, true, hex("1b5e3a"))
    );
    // Spacing and runs from the paragraph's and characters' formatting.
    assert_eq!(written[2].after, Spacing::Points(37.3));
    let coffee = &written[8];
    assert_eq!(
        (coffee.runs[0].text.as_str(), coffee.runs[0].bold),
        ("Morning Tide", true)
    );
    assert!(!coffee.runs[1].bold);
}
