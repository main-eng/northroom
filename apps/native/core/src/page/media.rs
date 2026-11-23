//! Board media blocks and a page's pictures.
//!
//! A media block is `![caption](asset:<id>)` on a line of its own. The `asset:` id resolves to
//! nothing in a drive, so the block draws as an empty frame reading "Board media" over its
//! caption. The block is atomic: it has no text, and its caption is a separate plain field.
//!
//! A picture in a drive page is an inline image, `![alt](name.png)`, naming a file beside the
//! page. This module resolves which file.
//!
//! A video or sound plays in a page when its block is only a raw HTML `<video>` or `<audio>`
//! element, as on GitHub. `![alt](clip.mp4)` stays an image, as CommonMark specifies. Only
//! files in the drive play, addressed from the page's folder.

use std::path::{Path, PathBuf};

use crate::page::inline::Image;
use crate::page::{BlockKind, Page};
use crate::workspace::{EntryKind, Workspace};

impl Page {
    /// A media block's caption; `None` for any other block.
    pub fn caption(&self, index: usize) -> Option<&str> {
        match &self.blocks.get(index)?.kind {
            BlockKind::Media { caption, .. } => Some(caption),
            _ => None,
        }
    }

    /// Replaces a media block's caption. Returns whether anything changed.
    pub fn set_caption(&mut self, index: usize, new_caption: &str) -> bool {
        match self.blocks.get_mut(index).map(|block| &mut block.kind) {
            Some(BlockKind::Media { caption, .. }) if caption != new_caption => {
                new_caption.clone_into(caption);
                true
            }
            _ => false,
        }
    }
}

/// Whether the text starts with a URL scheme (`^[a-z][a-z\d+.-]*:`, case-insensitive).
fn starts_with_scheme(text: &str) -> bool {
    let mut characters = text.chars();
    if !characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
    {
        return false;
    }
    for character in characters {
        match character {
            ':' => return true,
            'a'..='z' | 'A'..='Z' | '0'..='9' | '+' | '.' | '-' => {}
            _ => return false,
        }
    }
    false
}

/// JavaScript's `decodeURIComponent`: `None` where it throws (a stray `%`, or escapes that are
/// not UTF-8).
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = bytes.get(index + 1..index + 3)?;
            let high = char::from(digits[0]).to_digit(16)?;
            let low = char::from(digits[1]).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// The file an image's `src` names in its page's folder (`Chart%201.png` or `./Chart 1.png`),
/// or `None` for a URL or another folder.
pub fn image_file_name(src: &str) -> Option<String> {
    let name = src.strip_prefix("./").unwrap_or(src);
    if name.is_empty() || starts_with_scheme(name) || name.contains(['/', '\\']) {
        return None;
    }
    // A stray `%` that is not an escape: use the name verbatim.
    Some(decode_uri_component(name).unwrap_or_else(|| name.to_owned()))
}

/// What an image of a drive page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageShows {
    /// The picture file with this id beside the page, and its display size in pixels after EXIF
    /// rotation. The size is `None` when the platform cannot decode the file, which shows as a
    /// broken `<img>`.
    Picture {
        id: String,
        size: Option<(u32, u32)>,
    },
    /// A video or sound beside the page. An image cannot draw these, so the alt text shows in the
    /// line's own type, or nothing when there is none.
    Words(String),
    /// No such file beside the page, or a non-media file: the alt text, or the address when there
    /// is none, small and muted.
    Missing(String),
}

/// What `image` shows in the page at `page_path`.
pub fn image_shows(workspace: &mut Workspace, page_path: &Path, image: &Image) -> ImageShows {
    let found = image_file_name(&image.src)
        .zip(page_path.parent())
        .and_then(|(name, folder)| workspace.media_named(folder, &name));
    match found {
        Some((id, EntryKind::Image)) => {
            let size = workspace
                .path_of(&id)
                .ok()
                .and_then(|path| crate::media::picture_size_once(&path));
            ImageShows::Picture { id, size }
        }
        Some(_) => ImageShows::Words(image.alt.clone()),
        None => ImageShows::Missing(if image.alt.is_empty() {
            image.src.clone()
        } else {
            image.alt.clone()
        }),
    }
}

/// A page picture is never taller than this (`.page-inline-image img { max-height: 512px }`).
const PICTURE_MAX_HEIGHT: f64 = 512.0;

/// The drawn size of a `size`-pixel picture in a `width`-wide line: its own size, scaled down
/// with its aspect ratio kept to fit the line and `PICTURE_MAX_HEIGHT`.
pub fn picture_drawn_size(size: (u32, u32), width: f64) -> (f64, f64) {
    let (natural_width, natural_height) = (f64::from(size.0), f64::from(size.1));
    let scale = 1.0_f64
        .min(width / natural_width)
        .min(PICTURE_MAX_HEIGHT / natural_height);
    (natural_width * scale, natural_height * scale)
}

// Players: `<video>` and `<audio>` written as HTML.

/// The player's element. A check's report writes it in lowercase: `video` or `audio`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerElement {
    Video,
    Audio,
}

/// A `<video>` or `<audio>` element as written, reduced to what a browser reads.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerTag {
    pub element: PlayerElement,
    /// The addresses to try, in order: the element's own `src` if present (a browser then ignores
    /// its `<source>` children), else each `<source>`'s.
    pub sources: Vec<String>,
    pub controls: bool,
    pub looping: bool,
    pub muted: bool,
    /// Parsed but never acted on: a page never starts playing by itself.
    pub autoplay: bool,
    pub poster: Option<String>,
    /// In CSS pixels, from the attributes; `None` when absent, zero or a percentage.
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// The element's text content outside its child tags, whitespace collapsed: what a browser
    /// that cannot play it shows.
    pub fallback: String,
}

enum Token {
    Open {
        name: String,
        attributes: Vec<(String, String)>,
    },
    Close(String),
    Text(String),
}

/// Decodes the HTML character references people write by hand: `&amp;` and its kin, and
/// numeric ones.
fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        let reference = rest[1..]
            .find(';')
            .map(|end| &rest[1..=end])
            .filter(|name| name.len() <= 10);
        let character = reference.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let number = name.strip_prefix('#')?;
                let value = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(value)
            }
        });
        match (reference, character) {
            (Some(name), Some(character)) => {
                decoded.push(character);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// The tag at `start` (a `<`) and where it ends. `None` when no tag starts there (a literal
/// `<`), and `Some(None)` when one starts but never closes.
fn tag_at(html: &str, start: usize) -> Option<Option<(Token, usize)>> {
    let mut reader = TagReader {
        html,
        at: start + 1,
    };
    let closing = reader.byte() == Some(b'/');
    if closing {
        reader.at += 1;
    }
    if !reader.byte().is_some_and(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = reader.at;
    reader.skip_while(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    let name = html[name_start..reader.at].to_ascii_lowercase();
    if closing {
        // Browsers ignore anything after an end tag's name.
        return Some(
            html[reader.at..]
                .find('>')
                .map(|end| (Token::Close(name), reader.at + end + 1)),
        );
    }
    Some(reader.open_tag(name))
}

/// Reads a tag byte by byte; `at` is the next byte of `html`.
#[derive(Clone, Copy)]
struct TagReader<'a> {
    html: &'a str,
    at: usize,
}

impl TagReader<'_> {
    fn byte(&self) -> Option<u8> {
        self.html.as_bytes().get(self.at).copied()
    }

    /// Skips bytes while `keep` returns true.
    fn skip_while(&mut self, keep: impl Fn(u8) -> bool) {
        while self.byte().is_some_and(&keep) {
            self.at += 1;
        }
    }

    fn skip_space(&mut self) {
        self.skip_while(|byte| byte.is_ascii_whitespace());
    }

    /// The rest of an opening tag named `name`: its attributes up to `>`, and where it ends.
    /// `None` when it never closes.
    fn open_tag(mut self, name: String) -> Option<(Token, usize)> {
        let mut attributes: Vec<(String, String)> = Vec::new();
        loop {
            self.skip_space();
            match self.byte() {
                None => return None,
                Some(b'>') => return Some((Token::Open { name, attributes }, self.at + 1)),
                // `/>` ends a tag; browsers ignore the slash on non-void elements.
                Some(b'/') => {
                    self.at += 1;
                    continue;
                }
                Some(_) => {}
            }
            let (attribute, value) = self.attribute()?;
            // A repeated attribute keeps its first value, as in a browser.
            if !attributes.iter().any(|(known, _)| *known == attribute) {
                attributes.push((attribute, value));
            }
        }
    }

    /// One attribute's lowercased name and its value (empty when absent). `None` when a quoted
    /// value never closes.
    fn attribute(&mut self) -> Option<(String, String)> {
        let attribute_start = self.at;
        self.at += 1;
        self.skip_while(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b'>' | b'='));
        let attribute = self.html[attribute_start..self.at].to_ascii_lowercase();
        let mut ahead = *self;
        ahead.skip_space();
        if ahead.byte() != Some(b'=') {
            return Some((attribute, String::new()));
        }
        self.at = ahead.at + 1;
        self.skip_space();
        let value = self.value()?;
        Some((attribute, value))
    }

    /// An attribute value, quoted or not, with entities decoded. `None` when the quote never
    /// closes.
    fn value(&mut self) -> Option<String> {
        let html = self.html;
        if let Some(quote @ (b'"' | b'\'')) = self.byte() {
            let end = html[self.at + 1..].find(char::from(quote))?;
            let value = decode_entities(&html[self.at + 1..self.at + 1 + end]);
            self.at += end + 2;
            return Some(value);
        }
        let value_start = self.at;
        self.skip_while(|byte| !byte.is_ascii_whitespace() && byte != b'>');
        Some(decode_entities(&html[value_start..self.at]))
    }
}

/// The HTML as tag and text tokens, without comments. `None` when a tag never closes.
fn tokens(html: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < html.len() {
        if html[at..].starts_with("<!--") {
            at += 4 + html[at + 4..].find("-->")? + 3;
            continue;
        }
        if html.as_bytes()[at] == b'<' {
            if let Some(tag) = tag_at(html, at) {
                let (token, next) = tag?;
                tokens.push(token);
                at = next;
                continue;
            }
        }
        let next = html[at..]
            .char_indices()
            .skip(1)
            .find(|&(_, character)| character == '<')
            .map_or(html.len(), |(found, _)| at + found);
        tokens.push(Token::Text(decode_entities(&html[at..next])));
        at = next;
    }
    Some(tokens)
}

/// A width or height attribute in CSS pixels: its leading number, as a browser parses a
/// dimension (`480`, `480px`). A percentage or anything not positive gives `None`.
fn dimension(value: &str) -> Option<f64> {
    let value = value.trim_start();
    let end = value
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(value.len());
    let number: f64 = value[..end].parse().ok()?;
    (number > 0.0 && !value[end..].starts_with('%')).then_some(number)
}

/// Parses `html` as a single `<video>` or `<audio>` element with only whitespace and comments
/// around it, end tag optional, as a browser would. `None` for any other HTML.
pub fn player_tag(html: &str) -> Option<PlayerTag> {
    let tokens = tokens(html)?;
    let mut tokens = tokens
        .into_iter()
        .filter(|token| !matches!(token, Token::Text(text) if text.trim().is_empty()));
    let Some(Token::Open { name, attributes }) = tokens.next() else {
        return None;
    };
    let element = match name.as_str() {
        "video" => PlayerElement::Video,
        "audio" => PlayerElement::Audio,
        _ => return None,
    };
    let attribute = |wanted: &str| {
        attributes
            .iter()
            .find(|(known, _)| known == wanted)
            .map(|(_, value)| value.clone())
    };
    let inside = read_player_inside(tokens.by_ref(), &name)?;
    // After its end tag, only space and comments.
    if inside.closed && tokens.next().is_some() {
        return None;
    }
    let present = |wanted: &str| attributes.iter().any(|(known, _)| known == wanted);
    Some(PlayerTag {
        element,
        // Its own `src` overrides its `<source>` children.
        sources: attribute("src").map_or(inside.sources, |src| vec![src]),
        controls: present("controls"),
        looping: present("loop"),
        muted: present("muted"),
        autoplay: present("autoplay"),
        poster: attribute("poster").filter(|poster| !poster.trim().is_empty()),
        width: attribute("width").as_deref().and_then(dimension),
        height: attribute("height").as_deref().and_then(dimension),
        fallback: inside
            .fallback
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    })
}

/// A player's contents up to its end tag, if it has one.
#[derive(Default)]
struct PlayerInside {
    /// The addresses of its `<source>` children that have one.
    sources: Vec<String>,
    /// The fallback text shown when it cannot play.
    fallback: Vec<String>,
    closed: bool,
}

/// Reads `tokens` up to the end tag of the player named `name`. `None` when another player is
/// nested inside, since that is not a single element.
fn read_player_inside(tokens: impl Iterator<Item = Token>, name: &str) -> Option<PlayerInside> {
    let mut inside = PlayerInside::default();
    for token in tokens {
        match token {
            Token::Close(end) if end == name => {
                inside.closed = true;
                break;
            }
            Token::Open { name: inner, .. } if inner == "video" || inner == "audio" => return None,
            Token::Open {
                name: inner,
                attributes,
            } if inner == "source" => {
                let src = attributes
                    .into_iter()
                    .find(|(known, _)| known == "src")
                    .map(|(_, value)| value);
                inside
                    .sources
                    .extend(src.filter(|src| !src.trim().is_empty()));
            }
            Token::Text(words) => inside.fallback.push(words),
            // `<track>`, and the markup of the fallback text.
            Token::Open { .. } | Token::Close(_) => {}
        }
    }
    Some(inside)
}

/// The drive file a player address names relative to the page's folder (`clip.mp4`,
/// `./Clips/My%20clip.mov`, `../Shared/tone.wav`), as its id and kind from the folder listing.
/// `None` for an address with a scheme (`https:`, `file:`), an absolute path, one that leaves
/// the drive (via `..` or a symlink), and a file that is unlisted or not media. A `#t=` or `?`
/// suffix is not part of the name.
pub fn drive_media(
    workspace: &mut Workspace,
    page_path: &Path,
    address: &str,
) -> Option<(String, EntryKind)> {
    let (parts, name) = relative_address(address)?;
    let folder = walk_folders(page_path.parent()?.to_path_buf(), &parts)?;
    // Compare canonical paths, so neither `..` nor a symlink can lead out.
    let real_root = std::fs::canonicalize(workspace.root()).ok()?;
    let inside =
        |path: &Path| std::fs::canonicalize(path).is_ok_and(|real| real.starts_with(&real_root));
    if !inside(&folder) || !inside(&folder.join(&name)) {
        return None;
    }
    workspace.media_named(&folder, &name)
}

/// A page-relative address split into decoded folders and file name, without its `#t=` or `?`
/// suffix. `None` for an address with a scheme, an absolute path, or no file name.
fn relative_address(address: &str) -> Option<(Vec<String>, String)> {
    let address = address.trim();
    let address = address.split(['#', '?']).next().unwrap_or_default();
    if address.is_empty()
        || starts_with_scheme(address)
        || address.starts_with('/')
        || address.contains('\\')
    {
        return None;
    }
    let mut parts: Vec<String> = address
        .split('/')
        .map(|part| decode_uri_component(part).unwrap_or_else(|| part.to_owned()))
        .collect();
    let name = parts.pop()?;
    if name.is_empty()
        || name == "."
        || name == ".."
        || parts.iter().chain([&name]).any(|part| part.contains('/'))
    {
        return None;
    }
    Some((parts, name))
}

/// The folder `parts` lead to from `folder`; `None` when `..` climbs past the root.
fn walk_folders(mut folder: PathBuf, parts: &[String]) -> Option<PathBuf> {
    for part in parts {
        match part.as_str() {
            "" | "." => {}
            ".." => {
                if !folder.pop() {
                    return None;
                }
            }
            other => folder.push(other),
        }
    }
    Some(folder)
}

/// What a player of a drive page plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plays {
    /// A picture with its sound.
    Video,
    Sound,
}

impl Plays {
    /// Its name in a check's report, where a player with nothing to play says `nothing`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Sound => "sound",
        }
    }
}

/// What a player on a drive page shows and plays, for the shell to draw (`nr_page_player_lay`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerShows {
    /// The element as written.
    pub element: PlayerElement,
    /// What it plays and the id of that file; `None` when no address names a video or a sound in
    /// the drive.
    pub plays: Option<(Plays, String)>,
    /// The id of the poster's picture; empty when it has none in the drive.
    pub poster: String,
    /// What shows when it has nothing to play: its fallback text, or else its file name.
    pub words: String,
    pub controls: bool,
    pub looping: bool,
    pub muted: bool,
    pub autoplay: bool,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

/// The readable last part of an address: `clips/My%20clip.mp4#t=3` gives `My clip.mp4`.
fn address_name(address: &str) -> String {
    let path = address.trim().split(['#', '?']).next().unwrap_or_default();
    let last = path.rsplit('/').next().unwrap_or_default();
    decode_uri_component(last).unwrap_or_else(|| last.to_owned())
}

/// What `tag` shows in the page at `page_path`. The first address naming a video or sound in the
/// drive plays, as a browser falls through its sources. An `<audio>` plays a video's sound, and
/// a `<video>` of a sound plays as a sound.
pub fn player_shows(workspace: &mut Workspace, page_path: &Path, tag: &PlayerTag) -> PlayerShows {
    let plays =
        tag.sources.iter().find_map(
            |address| match drive_media(workspace, page_path, address)? {
                (id, EntryKind::Video) if tag.element == PlayerElement::Video => {
                    Some((Plays::Video, id))
                }
                (id, EntryKind::Video | EntryKind::Audio) => Some((Plays::Sound, id)),
                _ => None,
            },
        );
    let poster = tag
        .poster
        .as_deref()
        .and_then(|address| drive_media(workspace, page_path, address))
        .filter(|(_, kind)| *kind == EntryKind::Image)
        .map(|(id, _)| id)
        .unwrap_or_default();
    let named = tag
        .sources
        .first()
        .map(|address| address_name(address))
        .unwrap_or_default();
    PlayerShows {
        element: tag.element,
        plays,
        poster,
        words: if tag.fallback.is_empty() {
            named
        } else {
            tag.fallback.clone()
        },
        controls: tag.controls,
        looping: tag.looping,
        muted: tag.muted,
        autoplay: tag.autoplay,
        width: tag.width,
        height: tag.height,
    }
}

impl Page {
    /// A player block's HTML as written; `None` for any other block.
    pub fn player_html(&self, index: usize) -> Option<&str> {
        match &self.blocks.get(index)?.kind {
            BlockKind::Player { html } => Some(html),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Block;

    fn page(markdown: &str) -> Page {
        let mut next = 0;
        Page::from_file("Page", markdown, "en", &mut || {
            next += 1;
            format!("b{next}")
        })
    }

    fn kinds(page: &Page) -> Vec<&BlockKind> {
        page.blocks
            .iter()
            .map(|block: &Block| &block.kind)
            .collect()
    }

    #[test]
    fn a_caption_is_set_and_written_without_brackets() {
        let mut page = page("![old](asset:abc-123)\n");
        assert_eq!(page.caption(0), Some("old"));
        assert!(page.set_caption(0, "a [new] one"));
        assert!(!page.set_caption(0, "a [new] one"));
        assert_eq!(page.caption(0), Some("a [new] one"));
        assert_eq!(page.to_file(), "![a new one](asset:abc-123)\n");
    }

    #[test]
    fn only_a_media_block_has_a_caption() {
        let mut page = page("words\n\n---\n");
        assert_eq!(page.caption(0), None);
        assert!(!page.set_caption(0, "x"));
        assert!(!page.set_caption(1, "x"));
        assert!(!page.set_caption(9, "x"));
    }

    #[test]
    fn a_media_block_has_no_words() {
        let mut page = page("![cap](asset:1)\n\nbelow\n");
        assert_eq!(page.text(0), None);
        assert!(!page.set_text(0, "typed"));
        // Enter does not split it, and a mark or paste has nothing to land in.
        assert_eq!(page.split(0, 0, 0), None);
        assert_eq!(page.paste_markdown(0, 0, 0, "x"), None);
        assert_eq!(page.caption(0), Some("cap"));
    }

    #[test]
    fn backspace_below_media_never_joins_its_caption() {
        // A non-empty line stays where it is, as below a table.
        let mut with_words = page("![cap](asset:1)\n\nbelow\n");
        assert_eq!(
            with_words.merge_backward(1),
            Some((1, crate::page::edit::Caret::Start))
        );
        assert_eq!(with_words.caption(0), Some("cap"));
        assert_eq!(with_words.text(1), Some("below"));

        // Delete at the end of the line above leaves the block alone too.
        let mut above = page("above\n\n![cap](asset:1)\n");
        let _ = above.merge_forward(0);
        assert_eq!(above.text(0), Some("above"));
        assert_eq!(above.caption(1), Some("cap"));
    }

    #[test]
    fn media_is_deleted_copied_and_moved_whole() {
        let mut page = page("one\n\n![cap](asset:1)\n\ntwo\n");
        assert!(page.duplicate(1, 1));
        assert_eq!(
            page.to_file(),
            "one\n\n![cap](asset:1)\n\n![cap](asset:1)\n\ntwo\n"
        );
        assert_eq!(page.move_by_step(1, 1, true), Some(0));
        assert!(matches!(kinds(&page)[0], BlockKind::Media { .. }));
        let _ = page.delete_range(0, 0);
        assert_eq!(page.to_file(), "one\n\n![cap](asset:1)\n\ntwo\n");
        // The line added below it by Enter is a paragraph.
        let added = page.insert_after(1);
        assert_eq!(
            page.blocks[added].kind,
            BlockKind::Paragraph {
                text: String::new()
            }
        );
    }

    fn tag(html: &str) -> PlayerTag {
        player_tag(html).unwrap_or_else(|| panic!("a player: {html}"))
    }

    #[test]
    fn the_two_forms_models_write_are_players() {
        let video = tag(r#"<video src="clip.mp4" controls></video>"#);
        assert_eq!(video.element, PlayerElement::Video);
        assert_eq!(video.sources, ["clip.mp4"]);
        assert!(video.controls && !video.looping && !video.muted && !video.autoplay);
        assert_eq!(
            (
                video.width,
                video.height,
                video.poster.as_deref(),
                video.fallback.as_str()
            ),
            (None, None, None, "")
        );

        let audio = tag(r#"<audio src="tone.wav" controls></audio>"#);
        assert_eq!(
            (audio.element, audio.sources.as_slice()),
            (PlayerElement::Audio, ["tone.wav".to_owned()].as_slice())
        );
    }

    #[test]
    fn names_and_values_are_read_as_a_browser_reads_them() {
        // Any case, single quotes, no quotes, bare and valued booleans, `px`, entities.
        let read = tag("<VIDEO SRC='My clip.mp4' Controls=\"\" LOOP muted=muted AutoPlay width=480 height=270px poster=still.png></Video>");
        assert_eq!(read.sources, ["My clip.mp4"]);
        assert!(read.controls && read.looping && read.muted && read.autoplay);
        assert_eq!(
            (read.width, read.height, read.poster.as_deref()),
            (Some(480.0), Some(270.0), Some("still.png"))
        );
        assert_eq!(
            tag(r#"<audio src="a&amp;b.wav"></audio>"#).sources,
            ["a&b.wav"]
        );
        // A repeated attribute keeps its first value; a percentage or empty value gives no size.
        let twice =
            tag(r#"<video src="first.mp4" src="second.mp4" width="100%" height="0"></video>"#);
        assert_eq!(
            (twice.sources.as_slice(), twice.width, twice.height),
            (["first.mp4".to_owned()].as_slice(), None, None)
        );
        // Without an end tag, or self-closed, it is still one element, as in a browser.
        assert_eq!(tag(r#"<video src="a.mp4" controls>"#).sources, ["a.mp4"]);
        assert_eq!(tag(r#"<audio src="a.wav" controls />"#).sources, ["a.wav"]);
        // Space and comments around it.
        assert!(player_tag("<!-- the intro -->\n<video src=\"a.mp4\"></video>\n").is_some());
    }

    #[test]
    fn sources_are_tried_in_order_unless_the_element_names_its_own() {
        let nested = tag(
            "<video controls width=\"240\">\n  <source src=\"a.webm\" type=\"video/webm\">\n  <source src=\"b.mp4\" type=\"video/mp4\">\n  <track kind=\"captions\" src=\"c.vtt\">\n  Your browser <b>cannot</b> play this.\n</video>",
        );
        assert_eq!(nested.sources, ["a.webm", "b.mp4"]);
        assert_eq!(nested.fallback, "Your browser cannot play this.");
        assert_eq!(nested.width, Some(240.0));
        assert_eq!(
            tag(r#"<video src="own.mp4"><source src="other.mp4"></video>"#).sources,
            ["own.mp4"]
        );
        assert!(tag("<audio controls>Sound is not here.</audio>")
            .sources
            .is_empty());
    }

    #[test]
    fn any_other_html_is_no_player() {
        for html in [
            "<p>words</p>",
            "<videos src=\"a.mp4\"></videos>",
            "<div><video src=\"a.mp4\"></video></div>",
            "<video src=\"a.mp4\"></video> and words after",
            "words before <video src=\"a.mp4\"></video>",
            "<video src=\"a.mp4\"></video>\n<audio src=\"b.wav\"></audio>",
            "<video><audio src=\"b.wav\"></audio></video>",
            "<video src=\"a.mp4\"",
            "<video src=\"a.mp4></video>",
            "<!-- never closed <video src=\"a.mp4\"></video>",
            "plain words",
            "",
        ] {
            assert_eq!(player_tag(html), None, "{html}");
        }
    }

    #[test]
    fn a_page_keeps_its_players_as_written_and_other_html_as_words() {
        let markdown = "# Players\n\n<video src=\"clip.mp4\" controls></video>\n\n<audio src=\"tone.wav\" controls></audio>\n\n<video controls width=\"240\">\n  <source src=\"sub/nested.mp4\" type=\"video/mp4\">\n</video>\n\n![x](clip.mp4)\n\n<p>Other HTML</p>\n\n- <video src=\"clip.mp4\"></video>\n\nLast.\n";
        let read = page(markdown);
        let shown: Vec<&BlockKind> = kinds(&read);
        assert!(
            matches!(shown[0], BlockKind::Player { html } if html == "<video src=\"clip.mp4\" controls></video>")
        );
        assert!(
            matches!(shown[1], BlockKind::Player { html } if html == "<audio src=\"tone.wav\" controls></audio>")
        );
        assert!(
            matches!(shown[2], BlockKind::Player { html } if html.starts_with("<video controls") && html.ends_with("</video>") && html.contains('\n'))
        );
        // An image of a video stays an image; other HTML stays text; a list item stays one.
        assert_eq!(
            shown[3],
            &BlockKind::Paragraph {
                text: "![x](clip.mp4)".into()
            }
        );
        assert_eq!(
            shown[4],
            &BlockKind::Paragraph {
                text: "<p>Other HTML</p>".into()
            }
        );
        assert!(matches!(shown[5], BlockKind::Bullet { .. }));
        assert_eq!(
            read.player_html(0),
            Some("<video src=\"clip.mp4\" controls></video>")
        );
        assert_eq!(read.player_html(3), None);
        // Read and written back, the file is byte-for-byte identical.
        assert_eq!(read.to_file(), markdown);
    }

    #[test]
    fn a_player_is_a_whole_never_typed_in() {
        let mut read = page("above\n\n<video src=\"clip.mp4\" controls></video>\n\nbelow\n");
        assert_eq!(read.text(1), None);
        assert!(!read.set_text(1, "typed"));
        assert_eq!(read.split(1, 0, 0), None);
        // Backspace at the start of the line below and Delete at the end of the line above
        // leave it as it is.
        assert_eq!(
            read.merge_backward(2),
            Some((2, crate::page::edit::Caret::Start))
        );
        let _ = read.merge_forward(0);
        assert_eq!(
            read.to_file(),
            "above\n\n<video src=\"clip.mp4\" controls></video>\n\nbelow\n"
        );
        // Copied, moved and deleted whole.
        assert!(read.duplicate(1, 1));
        assert_eq!(read.markdown_of(1, 2), "<video src=\"clip.mp4\" controls></video>\n\n<video src=\"clip.mp4\" controls></video>");
        let _ = read.delete_range(1, 2);
        assert_eq!(read.to_file(), "above\n\nbelow\n");
        // Pasted, it is a player again.
        assert!(read
            .insert_markdown_after(0, "<audio src=\"tone.wav\" controls></audio>")
            .is_some());
        assert!(matches!(kinds(&read)[1], BlockKind::Player { .. }));
    }

    // Uses Unix symlinks.
    #[cfg(unix)]
    #[test]
    fn a_player_plays_a_listed_file_of_the_drive_and_nothing_from_outside() {
        let outside = tempfile::tempdir().expect("outside");
        let root = tempfile::tempdir().expect("folder");
        let metadata = tempfile::tempdir().expect("metadata");
        let folder = root.path().join("Notes");
        std::fs::create_dir_all(folder.join("Clips")).expect("folders");
        std::fs::create_dir_all(root.path().join("Shared")).expect("shared");
        for name in [
            "clip.mp4",
            "tone.wav",
            "still.png",
            "Clips/My clip.mov",
            "film.mkv",
            "notes.txt",
        ] {
            std::fs::write(folder.join(name), b"x").expect("file");
        }
        std::fs::write(root.path().join("Shared/up.mp4"), b"x").expect("file");
        std::fs::write(outside.path().join("away.mp4"), b"x").expect("file");
        std::os::unix::fs::symlink(outside.path().join("away.mp4"), folder.join("linked.mp4"))
            .expect("link");
        std::os::unix::fs::symlink(outside.path(), folder.join("Away")).expect("link");
        let page_path = folder.join("Page.md");
        std::fs::write(&page_path, "x").expect("page");
        let mut workspace = Workspace::open(root.path(), metadata.path()).expect("workspace");
        let mut kind =
            |address: &str| drive_media(&mut workspace, &page_path, address).map(|(_, kind)| kind);
        assert_eq!(kind("clip.mp4"), Some(EntryKind::Video));
        assert_eq!(kind("./clip.mp4#t=2"), Some(EntryKind::Video));
        assert_eq!(kind("tone.wav?v=1"), Some(EntryKind::Audio));
        assert_eq!(kind("Clips/My%20clip.mov"), Some(EntryKind::Video));
        assert_eq!(kind("../Shared/up.mp4"), Some(EntryKind::Video));
        assert_eq!(kind("film.mkv"), Some(EntryKind::Video));
        assert_eq!(kind("still.png"), Some(EntryKind::Image));
        for address in [
            "https://example.com/clip.mp4",
            "http://127.0.0.1/clip.mp4",
            "file:///tmp/clip.mp4",
            "/clip.mp4",
            "../../away.mp4",
            "linked.mp4",
            "Away/away.mp4",
            "gone.mp4",
            "notes.txt",
            "Clips",
            "",
        ] {
            assert_eq!(kind(address), None, "{address}");
        }

        let mut shows = |html: &str| player_shows(&mut workspace, &page_path, &tag(html));
        let video = shows(r#"<video src="clip.mp4" controls poster="still.png"></video>"#);
        let plays = |shows: &PlayerShows| shows.plays.as_ref().map(|(plays, _)| *plays);
        assert_eq!(
            (video.element, plays(&video), video.controls),
            (PlayerElement::Video, Some(Plays::Video), true)
        );
        // The first source that is in the drive plays.
        let nested = shows("<video controls><source src=\"https://example.com/a.mp4\"><source src=\"Clips/My%20clip.mov\"></video>");
        assert_eq!(plays(&nested), Some(Plays::Video));
        // A sound in a video element, or a video in an audio element, plays as a sound.
        assert_eq!(
            plays(&shows(r#"<video src="tone.wav" controls></video>"#)),
            Some(Plays::Sound)
        );
        assert_eq!(
            plays(&shows(r#"<audio src="clip.mp4" controls></audio>"#)),
            Some(Plays::Sound)
        );
        // Nothing to play: the fallback text, or else the file name.
        let remote =
            shows(r#"<video src="https://example.com/films/Remote%20cut.mp4" controls></video>"#);
        assert_eq!(
            (remote.plays, remote.words.as_str()),
            (None, "Remote cut.mp4")
        );
        let missing = shows("<audio src=\"gone.wav\" controls>The sound is not here.</audio>");
        assert_eq!(
            (missing.plays, missing.words.as_str()),
            (None, "The sound is not here.")
        );
        assert_eq!(shows(r#"<video src="still.png"></video>"#).plays, None);
        // Autoplay is parsed and reported, never acted on here.
        assert!(shows(r#"<video src="clip.mp4" autoplay></video>"#).autoplay);
        // The ids are the files' own, as the folder lists them.
        assert_eq!(
            workspace
                .path_of(&video.plays.expect("plays").1)
                .expect("path"),
            folder.join("clip.mp4")
        );
        assert_eq!(
            workspace.path_of(&video.poster).expect("path"),
            folder.join("still.png")
        );
    }

    #[test]
    fn a_picture_fits_its_line_and_is_never_taller_than_512() {
        assert_eq!(picture_drawn_size((640, 360), 1000.0), (640.0, 360.0));
        assert_eq!(picture_drawn_size((2000, 1000), 1000.0), (1000.0, 500.0));
        assert_eq!(picture_drawn_size((400, 1024), 1000.0), (200.0, 512.0));
        // Both at once: the smaller scale wins.
        assert_eq!(picture_drawn_size((4000, 4000), 600.0), (512.0, 512.0));
    }

    #[test]
    fn a_picture_is_a_listed_picture_file_beside_the_page() {
        let root = tempfile::tempdir().expect("folder");
        let metadata = tempfile::tempdir().expect("metadata");
        let folder = root.path().join("Notes");
        std::fs::create_dir_all(folder.join("sub")).expect("folders");
        for name in [
            "Chart 1.png",
            ".hidden.png",
            "clip.mp4",
            "tone.wav",
            "notes.txt",
            "sub/deep.png",
        ] {
            std::fs::write(folder.join(name), b"x").expect("file");
        }
        std::fs::write(folder.join("Page.md"), "![a](Chart%201.png)\n").expect("page");
        let mut workspace = Workspace::open(root.path(), metadata.path()).expect("workspace");
        let page = folder.join("Page.md");
        let image = |alt: &str, src: &str| Image {
            alt: alt.to_owned(),
            src: src.to_owned(),
        };

        let ImageShows::Picture { id, size } =
            image_shows(&mut workspace, &page, &image("a", "Chart%201.png"))
        else {
            panic!("a picture");
        };
        assert_eq!(
            workspace.path_of(&id).expect("path"),
            folder.join("Chart 1.png")
        );
        // The platform cannot decode it: a broken image with no size.
        assert_eq!(size, None);
        assert_eq!(
            image_shows(&mut workspace, &page, &image("", "./Chart 1.png")),
            ImageShows::Picture { id, size }
        );
        // A video or a sound written as an image shows its alt text, or nothing.
        assert_eq!(
            image_shows(&mut workspace, &page, &image("clip", "clip.mp4")),
            ImageShows::Words("clip".into())
        );
        assert_eq!(
            image_shows(&mut workspace, &page, &image("", "tone.wav")),
            ImageShows::Words(String::new())
        );
        for src in [
            "chart 1.png", // names are matched as written
            "missing.png", // no such file
            ".hidden.png", // not listed
            "notes.txt",   // not media
            "sub",
            "sub/deep.png", // another folder
            "https://example.com/a.png",
            "asset:abc",
        ] {
            assert_eq!(
                image_shows(&mut workspace, &page, &image("", src)),
                ImageShows::Missing(src.into()),
                "{src}"
            );
            assert_eq!(
                image_shows(&mut workspace, &page, &image("alt", src)),
                ImageShows::Missing("alt".into()),
                "{src}"
            );
        }
    }

    #[test]
    fn a_picture_the_platform_reads_shows_at_its_size() {
        let root = tempfile::tempdir().expect("folder");
        let metadata = tempfile::tempdir().expect("metadata");
        // The scratch drive's picture beside its page of every block (`scripts/scratch-drive.sh`).
        let picture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/drive-media/gradient.png");
        std::fs::copy(&picture, root.path().join("gradient.png")).expect("picture");
        std::fs::write(
            root.path().join("All blocks.md"),
            "![gradient](gradient.png)\n",
        )
        .expect("page");
        let mut workspace = Workspace::open(root.path(), metadata.path()).expect("workspace");
        let page = workspace.root().join("All blocks.md");
        let image = Image {
            alt: "gradient".to_owned(),
            src: "gradient.png".to_owned(),
        };
        let ImageShows::Picture { id, size } = image_shows(&mut workspace, &page, &image) else {
            panic!("a picture");
        };
        assert_eq!(size, Some((1280, 720)));
        let path = workspace.path_of(&id).expect("path");
        assert_eq!(crate::media::picture_size_once(&path), Some((1280, 720)));
    }
}
