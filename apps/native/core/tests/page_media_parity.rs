//! Checks board media blocks and page pictures against recorded results
//! (`fixtures/page_media.json`) for the same Markdown, captions, lines and image URLs.

use northroom_core::page::inline::parse;
use northroom_core::page::media::image_file_name;
use northroom_core::page::{blocks_from_markdown, markdown_from_blocks, Block, BlockKind};
use serde_json::{json, Value};

fn ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("b{next}")
    }
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/page_media.json")).expect("fixture")
}

#[test]
fn reads_and_writes_media_blocks_as_recorded() {
    let fixture = fixture();
    let pages = fixture["pages"].as_array().expect("pages");
    for case in pages {
        let markdown = case["markdown"].as_str().expect("markdown");
        let blocks = blocks_from_markdown(markdown, &mut ids());
        assert_eq!(
            serde_json::to_value(&blocks).expect("blocks"),
            case["blocks"],
            "blocks of {markdown:?}"
        );
        assert_eq!(
            json!(markdown_from_blocks(&blocks)),
            case["written"],
            "written from {markdown:?}"
        );
    }
    assert!(pages.len() >= 20);
    for case in fixture["spaced"].as_array().expect("spaced") {
        let markdown = case["markdown"].as_str().expect("markdown");
        let kinds: Vec<Value> = blocks_from_markdown(markdown, &mut ids())
            .iter()
            .map(|block| serde_json::to_value(block).expect("block")["type"].clone())
            .collect();
        assert_eq!(json!(kinds), case["kinds"], "kinds of {markdown:?}");
    }
}

#[test]
fn writes_a_caption_as_recorded() {
    let fixture = fixture();
    for case in fixture["captions"].as_array().expect("captions") {
        let caption = case["caption"].as_str().expect("caption");
        let block = Block {
            id: "b1".into(),
            kind: BlockKind::Media {
                asset_id: "abc-123".into(),
                caption: caption.into(),
            },
        };
        let written = markdown_from_blocks(&[block]);
        assert_eq!(json!(written), case["written"], "caption {caption:?}");
        let read = blocks_from_markdown(&written, &mut ids());
        assert_eq!(
            serde_json::to_value(&read).expect("blocks"),
            case["read"],
            "read back {caption:?}"
        );
    }
}

#[test]
fn finds_a_lines_images_as_recorded() {
    let fixture = fixture();
    for case in fixture["lines"].as_array().expect("lines") {
        let text = case["text"].as_str().expect("text");
        let images: Vec<Value> = parse(text)
            .iter()
            .map(|segment| {
                segment
                    .image
                    .as_ref()
                    .map_or(Value::Null, |image| json!([image.alt, image.src]))
            })
            .collect();
        assert_eq!(json!(images), case["images"], "images of {text:?}");
    }
}

#[test]
fn names_a_pictures_file_as_recorded() {
    let fixture = fixture();
    let sources = fixture["sources"].as_array().expect("sources");
    for case in sources {
        let src = case["src"].as_str().expect("src");
        assert_eq!(json!(image_file_name(src)), case["name"], "file of {src:?}");
    }
    assert!(sources.len() >= 30);
}
