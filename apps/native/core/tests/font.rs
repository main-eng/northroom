//! Checks font specimens (`deck::font`) against fonts fontTools wrote (`fixtures/fonts/make.py`):
//! TrueType, CFF, a collection and a Mac suitcase.

use northroom_core::deck::{font, Command, Deck, Fit, Item};
use std::path::PathBuf;

fn read(name: &str) -> Deck {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fonts")
        .join(name);
    font::read(&path).expect("read")
}

/// The face's name on its page, and the commands of its first drawn line.
fn page(deck: &Deck, index: usize) -> (String, Vec<Command>) {
    let slide = &deck.slides[index];
    let name = slide
        .items
        .iter()
        .find_map(|item| match item {
            Item::Shape(shape) => shape
                .text
                .as_ref()
                .map(|body| body.paragraphs[0].runs[0].text.clone()),
            Item::Picture(_) | Item::Figure(_) => None,
        })
        .expect("a name");
    let commands = slide
        .items
        .iter()
        .find_map(|item| match item {
            Item::Figure(figure) => Some(figure.commands.clone()),
            Item::Shape(_) | Item::Picture(_) => None,
        })
        .expect("a figure");
    (name, commands)
}

fn count(commands: &[Command], kind: fn(&Command) -> bool) -> usize {
    commands.iter().filter(|command| kind(command)).count()
}

#[test]
fn a_truetype_face_draws_its_letters_from_quadratic_outlines() {
    let deck = read("Box Sans.ttf");
    assert_eq!((deck.fit, deck.slides.len()), (Fit::Width, 1));
    let (name, commands) = page(&deck, 0);
    assert_eq!(name, "Box Sans Regular");
    // "A" and "B" in the alphabet's first line, each one closed contour; other letters have no
    // glyph and are left out.
    assert_eq!(count(&commands, |c| matches!(c, Command::Close)), 2);
    assert!(count(&commands, |c| matches!(c, Command::Quad(..))) > 0);
    // The outline is placed by the face's side bearing, 0 here, so the box starts at the margin.
    assert_eq!(commands[0], Command::Move(48.0, page_baseline(&deck)));
}

fn page_baseline(deck: &Deck) -> f64 {
    let Some(Command::Move(_, y)) = page(deck, 0).1.first().copied() else {
        panic!("a move first");
    };
    y
}

#[test]
fn a_cff_face_draws_cubic_outlines_and_a_collection_shows_each_face() {
    let (name, commands) = page(&read("Box Serif.otf"), 0);
    assert_eq!(name, "Box Serif Regular");
    assert!(count(&commands, |c| matches!(c, Command::Cubic(..))) > 0);
    let collection = read("Box Family.ttc");
    let names: Vec<String> = (0..collection.slides.len())
        .map(|index| page(&collection, index).0)
        .collect();
    assert_eq!(names, ["Box Sans Regular", "Box Serif Regular"]);
    assert_eq!(page(&read("Box Sans.dfont"), 0).0, "Box Sans Regular");
}

#[test]
fn a_file_that_is_not_a_font_says_so() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fonts/make.py");
    assert!(font::read(&path).is_err());
}
