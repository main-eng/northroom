//! Reads a Keynote, Pages or Numbers document as the preview picture it carries. Apple doesn't
//! document the format its content is stored in, so the picture is the document's first slide,
//! page or sheet as the application last saved it, and the rest is not shown.
//!
//! A document is a zip, or on a Mac a folder package, holding `preview.jpg`. Documents from
//! before 2013 keep it as `QuickLook/Thumbnail.jpg` instead.

use super::{flow, read_bytes, Deck, ReadError, Result, Source};
use crate::package::{ZipArchive, PART_MAX_BYTES};
use std::path::Path;

/// Where the preview is, newest layout first.
const PREVIEWS: [&str; 2] = ["preview.jpg", "QuickLook/Thumbnail.jpg"];
/// A zip's first bytes.
const ZIP_SIGNATURE: &[u8] = b"PK\x03\x04";

/// Whether the file at `path` is laid out as a document of this kind: a folder or a zip. A file
/// of another kind with the same name (a `.key` that is a private key) is not.
pub fn is_document(path: &Path) -> bool {
    if path.is_dir() {
        return true;
    }
    let mut first = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut first))
        .is_ok_and(|()| first == ZIP_SIGNATURE)
}

/// The preview's source and bytes.
fn find_preview(path: &Path) -> Result<(Source, Vec<u8>)> {
    if path.is_dir() {
        for name in PREVIEWS {
            let file = path.join(name);
            if let Ok(bytes) = std::fs::read(&file) {
                return Ok((Source::File(file), bytes));
            }
        }
        return Err(ReadError::Missing(PREVIEWS[0].to_owned()));
    }
    let bytes = read_bytes(path)?;
    let archive = ZipArchive::open(&bytes)?;
    for name in PREVIEWS {
        if let Some(preview) = archive.read_entry(name, PART_MAX_BYTES)? {
            return Ok((Source::Entry(name.to_owned()), preview));
        }
    }
    Err(ReadError::Missing(PREVIEWS[0].to_owned()))
}

/// Reads the document at `path` as a page holding its preview.
pub fn read(path: &Path) -> Result<Deck> {
    let (source, bytes) = find_preview(path)?;
    let (width, height) = flow::picture_pixels(&bytes)
        .ok_or_else(|| ReadError::Damaged("The preview is not a picture.".to_owned()))?;
    Ok(super::picture::deck_of(source, width, height))
}
