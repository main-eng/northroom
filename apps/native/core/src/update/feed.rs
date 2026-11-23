//! The update feed: an electron-builder channel file (`<channel>-mac.yml`) at a generic feed
//! URL, the files it lists, and which zip a Mac downloads. The rules follow electron-updater
//! 6.8 (`GenericProvider`, `Provider.resolveFiles`, `findFile`,
//! `MacUpdater.filterFilesForArch`, `AppUpdater.isUpdateAvailable`; each rule below names its
//! source). Windows and Linux read the same shape from their own channel files
//! (`channel_file`) and take the one file named for their architecture (`choose_package`). A
//! file may carry a minisign `signature`, as the Linux AppImage does.
//!
//! The channel file is YAML as js-yaml's `dump` writes it: block mappings and sequences, plain,
//! quoted and block scalars. This parser reads that subset only.

use super::version::Version;

/// A parsed YAML node.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Scalar(String),
    List(Vec<Node>),
    Map(Vec<(String, Node)>),
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, node)| node),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Node::Scalar(text) => Some(text),
            _ => None,
        }
    }
}

struct Line<'a> {
    indent: usize,
    text: &'a str,
}

/// Parses the block YAML js-yaml writes.
pub fn parse_yaml(source: &str) -> Result<Node, String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut at = 0;
    let node = block(&lines, &mut at, 0)?;
    while at < lines.len() {
        if !lines[at].trim().is_empty() {
            return Err(format!("unexpected text on line {}", at + 1));
        }
        at += 1;
    }
    Ok(node)
}

fn line<'a>(lines: &[&'a str], at: usize) -> Option<Line<'a>> {
    let raw = lines.get(at)?;
    let text = raw.trim_start_matches(' ');
    Some(Line {
        indent: raw.len() - text.len(),
        text,
    })
}

fn skip_blank(lines: &[&str], at: &mut usize) {
    while *at < lines.len()
        && (lines[*at].trim().is_empty() || lines[*at].trim_start().starts_with('#'))
    {
        *at += 1;
    }
}

/// The block (a map or a list) whose first line is indented at least `minimum`.
fn block(lines: &[&str], at: &mut usize, minimum: usize) -> Result<Node, String> {
    skip_blank(lines, at);
    let Some(first) = line(lines, *at) else {
        return Ok(Node::Scalar(String::new()));
    };
    if first.indent < minimum {
        return Ok(Node::Scalar(String::new()));
    }
    if starts_item(first.text) {
        return list(lines, at, first.indent);
    }
    map(lines, at, first.indent, None)
}

/// Whether a line is a list item: a lone dash, or a dash and a space.
fn starts_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// A list whose dashes are indented `indent`.
fn list(lines: &[&str], at: &mut usize, indent: usize) -> Result<Node, String> {
    let mut items = Vec::new();
    loop {
        skip_blank(lines, at);
        let Some(current) = line(lines, *at) else {
            break;
        };
        if current.indent != indent || !starts_item(current.text) {
            break;
        }
        items.push(list_item(lines, at, indent, current.text)?);
    }
    Ok(Node::List(items))
}

/// The list item on line `text`, dash indented `indent`: a block under it, a map starting on
/// the same line, or a scalar.
fn list_item<'a>(
    lines: &[&'a str],
    at: &mut usize,
    indent: usize,
    text: &'a str,
) -> Result<Node, String> {
    let rest = text[1..].trim_start_matches(' ');
    if rest.is_empty() {
        *at += 1;
        return block(lines, at, indent + 1);
    }
    if key_of(rest).is_some() {
        // A map starting on the dash's line has its keys aligned after the dash.
        let inner = indent + (text.len() - rest.len());
        return map(lines, at, inner, Some(rest));
    }
    *at += 1;
    scalar_value(rest, lines, at, indent)
}

/// Splits a `key:` line into the key and what follows the colon.
fn key_of(text: &str) -> Option<(String, &str)> {
    let (key, rest) = if let Some(quoted) = text.strip_prefix('\'') {
        let end = single_quoted_end(quoted)?;
        (quoted[..end].replace("''", "'"), &quoted[end + 1..])
    } else if text.starts_with('"') {
        let (key, used) = double_quoted(text)?;
        (key, &text[used..])
    } else {
        let colon = text
            .find(": ")
            .or_else(|| text.ends_with(':').then(|| text.len() - 1))?;
        (text[..colon].to_owned(), &text[colon..])
    };
    let rest = rest.strip_prefix(':')?;
    (rest.is_empty() || rest.starts_with(' ')).then(|| (key, rest.trim_start_matches(' ')))
}

/// A map whose keys are indented `indent`. `first` is a first line already trimmed to its key
/// (after a list dash).
fn map<'a>(
    lines: &[&'a str],
    at: &mut usize,
    indent: usize,
    first: Option<&'a str>,
) -> Result<Node, String> {
    let mut entries = Vec::new();
    let mut pending = first;
    while let Some(text) = pending.take().or_else(|| next_key_line(lines, at, indent)) {
        let (key, rest) =
            key_of(text).ok_or_else(|| format!("expected a key on line {}", *at + 1))?;
        *at += 1;
        let value = if rest.is_empty() {
            nested_value(lines, at, indent)?
        } else {
            scalar_value(rest, lines, at, indent)?
        };
        entries.push((key, value));
    }
    Ok(Node::Map(entries))
}

/// The next key line of a map indented `indent`, skipping blank lines; `None` at the map's end.
fn next_key_line<'a>(lines: &[&'a str], at: &mut usize, indent: usize) -> Option<&'a str> {
    skip_blank(lines, at);
    let current = line(lines, *at)?;
    (current.indent == indent && !current.text.starts_with("- ")).then_some(current.text)
}

/// The value of a key with nothing after its colon: a nested block, or a list at the same
/// indent (js-yaml indents lists, but YAML allows them level with the key).
fn nested_value(lines: &[&str], at: &mut usize, indent: usize) -> Result<Node, String> {
    skip_blank(lines, at);
    match line(lines, *at) {
        Some(next) if next.indent > indent => block(lines, at, indent + 1),
        Some(next) if next.indent == indent && next.text.starts_with("- ") => {
            block(lines, at, indent)
        }
        _ => Ok(Node::Scalar(String::new())),
    }
}

/// A value on its key's line: a scalar, an empty flow collection, or a block scalar header
/// whose lines follow, indented past `parent`.
fn scalar_value(text: &str, lines: &[&str], at: &mut usize, parent: usize) -> Result<Node, String> {
    if text == "[]" {
        return Ok(Node::List(Vec::new()));
    }
    if text == "{}" {
        return Ok(Node::Map(Vec::new()));
    }
    if let Some(header) = text.strip_prefix('|') {
        return block_scalar(header, BlockStyle::Literal, lines, at, parent).map(Node::Scalar);
    }
    if let Some(header) = text.strip_prefix('>') {
        return block_scalar(header, BlockStyle::Folded, lines, at, parent).map(Node::Scalar);
    }
    if let Some(quoted) = text.strip_prefix('\'') {
        let end = single_quoted_end(quoted).ok_or("an unclosed quote")?;
        return Ok(Node::Scalar(quoted[..end].replace("''", "'")));
    }
    if text.starts_with('"') {
        return double_quoted(text)
            .map(|(value, _)| Node::Scalar(value))
            .ok_or_else(|| "an unclosed quote".into());
    }
    // A plain scalar ends at a comment.
    let plain = text.find(" #").map_or(text, |cut| &text[..cut]).trim_end();
    Ok(Node::Scalar(plain.to_owned()))
}

fn single_quoted_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\'' {
            if bytes.get(index + 1) == Some(&b'\'') {
                index += 2;
                continue;
            }
            return Some(index);
        }
        index += 1;
    }
    None
}

/// A double-quoted scalar at the start of `text`: its value and byte length.
fn double_quoted(text: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = text.char_indices().skip(1);
    while let Some((index, character)) = chars.next() {
        match character {
            '"' => return Some((out, index + 1)),
            '\\' => {
                let (_, escape) = chars.next()?;
                let mut code = |count: usize| -> Option<char> {
                    let digits: String = (0..count)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    char::from_u32(u32::from_str_radix(&digits, 16).ok()?)
                };
                out.push(match escape {
                    '0' => '\0',
                    'a' => '\u{7}',
                    'b' => '\u{8}',
                    't' | '\t' => '\t',
                    'n' => '\n',
                    'v' => '\u{b}',
                    'f' => '\u{c}',
                    'r' => '\r',
                    'e' => '\u{1b}',
                    ' ' => ' ',
                    '"' => '"',
                    '/' => '/',
                    '\\' => '\\',
                    'N' => '\u{85}',
                    '_' => '\u{a0}',
                    'L' => '\u{2028}',
                    'P' => '\u{2029}',
                    'x' => code(2)?,
                    'u' => code(4)?,
                    'U' => code(8)?,
                    _ => return None,
                });
            }
            _ => out.push(character),
        }
    }
    None
}

/// How block scalar lines join: `|` keeps line breaks, `>` folds them.
#[derive(Clone, Copy)]
enum BlockStyle {
    Literal,
    Folded,
}

/// Chomping of trailing line breaks: `-` strips them, `+` keeps them all, else one is kept.
#[derive(Clone, Copy)]
enum Chomping {
    Strip,
    Clip,
    Keep,
}

/// A block scalar header: chomping, and explicit indentation if given.
struct BlockHeader {
    chomping: Chomping,
    indent: Option<usize>,
}

/// Parses the header after `|` or `>`; explicit indentation counts from `parent`.
fn block_header(header: &str, parent: usize) -> Result<BlockHeader, String> {
    let mut parsed = BlockHeader {
        chomping: Chomping::Clip,
        indent: None,
    };
    for character in header.trim().chars() {
        match character {
            '-' => parsed.chomping = Chomping::Strip,
            '+' => parsed.chomping = Chomping::Keep,
            digit if digit.is_ascii_digit() => {
                parsed.indent = digit.to_digit(10).map(|value| parent + value as usize)
            }
            _ => return Err(format!("a block scalar header {header:?}")),
        }
    }
    Ok(parsed)
}

/// Reads a `|` or `>` scalar with its chomping and indentation (explicit, or the first line's).
fn block_scalar(
    header: &str,
    style: BlockStyle,
    lines: &[&str],
    at: &mut usize,
    parent: usize,
) -> Result<String, String> {
    let header = block_header(header, parent)?;
    let body = block_lines(lines, at, parent, header.indent);
    // Trailing blank lines belong to chomping, not the text.
    let trailing = body.iter().rev().take_while(|text| text.is_empty()).count();
    let content = &body[..body.len() - trailing];
    let mut text = match style {
        BlockStyle::Folded => fold_lines(content),
        BlockStyle::Literal => content.join("\n"),
    };
    let breaks = match header.chomping {
        Chomping::Strip => 0,
        Chomping::Clip => usize::from(!content.is_empty()),
        Chomping::Keep => usize::from(!content.is_empty()) + trailing,
    };
    text.extend(std::iter::repeat_n('\n', breaks));
    Ok(text)
}

/// Block scalar lines from `at` while indented past `parent`, with indentation (`indent`, or the
/// first line's) removed. Blank lines are empty.
fn block_lines<'a>(
    lines: &[&'a str],
    at: &mut usize,
    parent: usize,
    mut indent: Option<usize>,
) -> Vec<&'a str> {
    let mut body: Vec<&str> = Vec::new();
    while let Some(&raw) = lines.get(*at) {
        let spaces = raw.len() - raw.trim_start_matches(' ').len();
        if raw.trim().is_empty() {
            body.push("");
            *at += 1;
            continue;
        }
        let wanted = *indent.get_or_insert(spaces);
        if spaces < wanted || wanted <= parent {
            break;
        }
        body.push(&raw[wanted..]);
        *at += 1;
    }
    body
}

/// Folds lines: a break between two non-indented lines becomes a space.
fn fold_lines(content: &[&str]) -> String {
    let mut out = String::new();
    for (index, piece) in content.iter().enumerate() {
        if index > 0 {
            let previous = content[index - 1];
            let joins = !piece.is_empty()
                && !previous.is_empty()
                && !piece.starts_with(' ')
                && !previous.starts_with(' ');
            out.push(if joins { ' ' } else { '\n' });
        }
        out.push_str(piece);
    }
    out
}

/// One file an update lists.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateFile {
    pub url: String,
    pub sha512: String,
    pub size: Option<u64>,
    /// The file's minisign signature (the `.minisig` file in base64), in Tauri's feed format.
    /// electron-builder writes none.
    pub signature: Option<String>,
}

/// What a channel file says about the latest release.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub version: String,
    pub files: Vec<UpdateFile>,
    pub release_notes: Option<String>,
    pub staging_percentage: Option<String>,
    pub minimum_system_version: Option<String>,
}

/// Parses a channel file (`parseUpdateInfo`, then `getFileList`: `files`, or the legacy
/// top-level `path` and `sha512` when absent).
pub fn parse_update_info(
    text: &str,
    channel_file: &str,
    channel_url: &str,
) -> Result<UpdateInfo, String> {
    let node = parse_yaml(text).map_err(|reason| {
        format!("Cannot parse update info from {channel_file} in the latest release artifacts ({channel_url}): {reason}")
    })?;
    let field = |name: &str| node.get(name).and_then(Node::text).map(str::to_owned);
    let mut files = Vec::new();
    if let Some(Node::List(items)) = node.get("files") {
        for item in items {
            let url = item
                .get("url")
                .and_then(Node::text)
                .unwrap_or_default()
                .to_owned();
            let sha512 = item.get("sha512").and_then(Node::text).map(str::to_owned);
            let Some(sha512) = sha512 else {
                return Err(format!(
                    "Update info doesn't contain nor sha256 neither sha512 checksum: {url}"
                ));
            };
            files.push(UpdateFile {
                url,
                sha512,
                size: item
                    .get("size")
                    .and_then(Node::text)
                    .and_then(|size| size.parse().ok()),
                signature: item
                    .get("signature")
                    .and_then(Node::text)
                    .map(str::to_owned),
            });
        }
    }
    if files.is_empty() {
        match (field("path"), field("sha512")) {
            (Some(url), Some(sha512)) => files.push(UpdateFile {
                url,
                sha512,
                size: None,
                signature: None,
            }),
            (Some(url), None) => {
                return Err(format!(
                    "Update info doesn't contain nor sha256 neither sha512 checksum: {url}"
                ))
            }
            _ => return Err("No files provided".into()),
        }
    }
    Ok(UpdateInfo {
        version: field("version").unwrap_or_default(),
        files,
        release_notes: field("releaseNotes"),
        staging_percentage: field("stagingPercentage"),
        minimum_system_version: field("minimumSystemVersion"),
    })
}

/// The feed URL as a base, always ending in `/` (`newBaseUrl`).
pub fn base_url(feed: &str) -> String {
    let (address, search) = feed
        .split_once('?')
        .map_or((feed, None), |(address, search)| (address, Some(search)));
    let mut base = address.to_owned();
    if !base.ends_with('/') {
        base.push('/');
    }
    if let Some(search) = search {
        base.push('?');
        base.push_str(search);
    }
    base
}

/// Writes the characters a WHATWG URL parser escapes in a path.
fn escape_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/%[]|".contains(&byte);
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `new URL(path, base)` for a channel file path: absolute, root-relative, or beside the
/// channel file. The base's query is carried over (`newUrlFromBase`).
pub fn resolve(path: &str, base: &str) -> String {
    let (base_address, search) = base
        .split_once('?')
        .map_or((base, None), |(address, search)| (address, Some(search)));
    let resolved = if path.contains("://") {
        let (scheme, rest) = path.split_once("://").unwrap_or(("", path));
        let (host, tail) = rest
            .split_once('/')
            .map_or((rest, ""), |(host, tail)| (host, tail));
        format!("{scheme}://{host}/{}", escape_path(tail))
    } else if let Some(rooted) = path.strip_prefix('/') {
        let origin_end = base_address.find("://").map_or(0, |at| at + 3);
        let host_end = base_address[origin_end..]
            .find('/')
            .map_or(base_address.len(), |at| origin_end + at);
        format!("{}/{}", &base_address[..host_end], escape_path(rooted))
    } else {
        format!("{base_address}{}", escape_path(path))
    };
    match search {
        Some(search) if !search.is_empty() && !resolved.contains('?') => {
            format!("{resolved}?{search}")
        }
        _ => resolved,
    }
}

/// The channel file URL. A feed query is kept; otherwise a cache buster is added
/// (`noCache=<Unix ms in base 32>`).
pub fn channel_url(feed: &str, channel_file: &str, now_ms: u64) -> String {
    let base = base_url(feed);
    if base.contains('?') {
        return resolve(channel_file, &base);
    }
    format!(
        "{}?noCache={}",
        resolve(channel_file, &base),
        radix32(now_ms)
    )
}

fn radix32(mut value: u64) -> String {
    const DIGITS: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    if value == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 32) as usize]);
        value /= 32;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Update platforms, each with its own channel file and package kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    #[cfg(target_os = "macos")]
    pub const RUNNING: Platform = Platform::Mac;
    #[cfg(target_os = "windows")]
    pub const RUNNING: Platform = Platform::Windows;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub const RUNNING: Platform = Platform::Linux;
}

/// The channel file name: `<channel>-mac.yml`, `<channel>-windows.yml` or `<channel>-linux.yml`.
pub fn channel_file(channel: &str, platform: Platform) -> String {
    let suffix = match platform {
        Platform::Mac => "mac",
        Platform::Windows => "windows",
        Platform::Linux => "linux",
    };
    format!("{channel}-{suffix}.yml")
}

fn path_of(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |at| at + 3);
    let path = url[after_scheme..]
        .find('/')
        .map_or("", |at| &url[after_scheme + at..]);
    path.split(['?', '#']).next().unwrap_or(path)
}

/// The zip a Mac downloads: arm64 files on an arm64 Mac when any exist, never on Intel
/// (`filterFilesForArch`), then the zip named for the architecture, or the first zip
/// (`findFile(files, "zip", ["pkg", "dmg"])`). No zip is an error, as in MacUpdater, and so is
/// any non-zip `findFile` would fall back to.
pub fn choose_zip<'a>(
    info: &'a UpdateInfo,
    base: &str,
    arm64: bool,
) -> Result<(String, &'a UpdateFile), String> {
    let files: Vec<(String, &UpdateFile)> = info
        .files
        .iter()
        .map(|file| (resolve(&file.url, base), file))
        .collect();
    let is_arm64 = |(url, file): &(String, &UpdateFile)| {
        path_of(url).contains("arm64") || file.url.contains("arm64")
    };
    let wanted: Vec<&(String, &UpdateFile)> = if arm64 && files.iter().any(is_arm64) {
        files.iter().filter(|file| is_arm64(file)).collect()
    } else {
        files.iter().filter(|file| !is_arm64(file)).collect()
    };
    if wanted.is_empty() {
        return Err("No files provided".into());
    }
    let arch = if arm64 { "arm64" } else { "x64" };
    let ends_with = |url: &str, extension: &str| {
        path_of(url)
            .to_lowercase()
            .ends_with(&format!(".{extension}"))
    };
    let zips: Vec<&&(String, &UpdateFile)> = wanted
        .iter()
        .filter(|(url, _)| ends_with(url, "zip"))
        .collect();
    let chosen = zips
        .iter()
        .find(|(url, file)| path_of(url).contains(arch) || file.url.contains(arch))
        .or_else(|| zips.first())
        .map(|found| (**found).clone())
        .or_else(|| {
            wanted
                .iter()
                .find(|(url, _)| !ends_with(url, "pkg") && !ends_with(url, "dmg"))
                .map(|found| (*found).clone())
        });
    match chosen {
        Some((url, file)) if ends_with(&url, "zip") => Ok((url, file)),
        Some(_) => Err("ZIP file not provided".into()),
        None => Err("ZIP file not provided".into()),
    }
}

/// The file to download: a Mac's zip (`choose_zip`), or the Windows setup or Linux AppImage
/// named for this architecture (`Northroom-<version>-x64.exe`,
/// `Northroom-<version>-x86_64.AppImage`). Any other file is refused.
pub fn choose_package<'a>(
    info: &'a UpdateInfo,
    base: &str,
    platform: Platform,
    arm64: bool,
) -> Result<(String, &'a UpdateFile), String> {
    let (extension, arch) = match (platform, arm64) {
        (Platform::Mac, _) => return choose_zip(info, base, arm64),
        (Platform::Windows, false) => ("exe", "x64"),
        (Platform::Windows, true) => ("exe", "arm64"),
        (Platform::Linux, false) => ("appimage", "x86_64"),
        (Platform::Linux, true) => ("appimage", "aarch64"),
    };
    info.files
        .iter()
        .map(|file| (resolve(&file.url, base), file))
        .find(|(url, _)| {
            let path = path_of(url).to_lowercase();
            path.ends_with(&format!("-{arch}.{extension}"))
        })
        .ok_or_else(|| format!("The update has no .{extension} file for {arch}"))
}

/// Whether the feed's version updates `current` (`isUpdateAvailable` without downgrade): not
/// equal, system new enough, inside the rollout, and newer.
pub fn is_update_available(
    info: &UpdateInfo,
    current: &str,
    os_release: &str,
    staging_id: &str,
) -> Result<bool, String> {
    let latest = Version::parse(&info.version).ok_or_else(|| {
        format!(
            "This file could not be downloaded, or the latest version (from update server) does not have a valid semver version: \"{}\"",
            info.version
        )
    })?;
    let current = Version::parse(current)
        .ok_or_else(|| format!("App version is not a valid semver version: \"{current}\""))?;
    if latest == current {
        return Ok(false);
    }
    if !system_is_new_enough(info, os_release)
        || !within_rollout(info.staging_percentage.as_deref(), staging_id)
    {
        return Ok(false);
    }
    Ok(latest > current)
}

/// Whether the kernel release (`os.release()`) meets the update's `minimumSystemVersion`
/// (`checkIfUpdateSupported`). An incomparable value passes.
fn system_is_new_enough(info: &UpdateInfo, os_release: &str) -> bool {
    let Some(minimum) = info
        .minimum_system_version
        .as_deref()
        .filter(|text| !text.is_empty())
    else {
        return true;
    };
    match (Version::parse(os_release), Version::parse(minimum)) {
        (Some(system), Some(minimum)) => system >= minimum,
        _ => true,
    }
}

/// Whether this computer is in the rollout (`isStagingMatch`): everyone with no percentage,
/// otherwise when the id's last four bytes, as a fraction of 2³²−1, fall below it.
pub fn within_rollout(percentage: Option<&str>, staging_id: &str) -> bool {
    let Some(raw) = percentage else { return true };
    // Parsed like JavaScript's `parseInt`: an optional sign, then the leading digits.
    let trimmed = raw.trim();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let leading: String = digits.chars().take_while(char::is_ascii_digit).collect();
    let Ok(value) = leading.parse::<f64>() else {
        return true;
    };
    let fraction = sign * value / 100.0;
    let hex: String = staging_id.chars().filter(char::is_ascii_hexdigit).collect();
    let Some(last) = hex
        .get(24..32)
        .and_then(|text| u32::from_str_radix(text, 16).ok())
    else {
        return true;
    };
    f64::from(last) / f64::from(u32::MAX) < fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_address() {
        assert_eq!(
            channel_url(
                "https://api.northroom.com/api/download/update",
                "native-mac.yml",
                1_763_000_000_000
            ),
            "https://api.northroom.com/api/download/update/native-mac.yml?noCache=1j9tfq7g0"
        );
        assert_eq!(
            channel_url("http://127.0.0.1:8123/feed/?t=1", "native-mac.yml", 5),
            "http://127.0.0.1:8123/feed/native-mac.yml?t=1"
        );
        assert_eq!(radix32(0), "0");
    }

    #[test]
    fn each_platform_reads_its_own_channel_and_file() {
        assert_eq!(channel_file("native", Platform::Mac), "native-mac.yml");
        assert_eq!(
            channel_file("native", Platform::Windows),
            "native-windows.yml"
        );
        assert_eq!(channel_file("native", Platform::Linux), "native-linux.yml");
        let text = "version: 0.3.1\nfiles:\n  - url: Northroom-0.3.1-arm64.zip\n    sha512: a\n  - url: Northroom-0.3.1-x64.exe\n    sha512: b\n    size: 9\n  - url: Northroom-0.3.1-arm64.exe\n    sha512: c\n  - url: Northroom-0.3.1-x86_64.AppImage\n    sha512: d\n    signature: c2ln\npath: Northroom-0.3.1-x64.exe\nsha512: b\n";
        let info = parse_update_info(text, "native-windows.yml", "http://x.test/").unwrap();
        let base = base_url("https://api.northroom.com/api/download/update");
        let chosen = |platform, arm64| {
            choose_package(&info, &base, platform, arm64)
                .map(|(url, file)| (url, file.sha512.clone()))
        };
        assert_eq!(
            chosen(Platform::Windows, false),
            Ok((
                "https://api.northroom.com/api/download/update/Northroom-0.3.1-x64.exe".into(),
                "b".into()
            ))
        );
        assert_eq!(
            chosen(Platform::Windows, true).map(|(_, sha)| sha),
            Ok("c".into())
        );
        assert_eq!(
            chosen(Platform::Linux, false).map(|(_, sha)| sha),
            Ok("d".into())
        );
        assert_eq!(
            chosen(Platform::Linux, true),
            Err("The update has no .appimage file for aarch64".into())
        );
        assert_eq!(
            chosen(Platform::Mac, true).map(|(_, sha)| sha),
            Ok("a".into())
        );
        let linux = info.files.iter().find(|file| file.sha512 == "d").unwrap();
        assert_eq!(linux.signature.as_deref(), Some("c2ln"));
        assert_eq!(info.files[1].size, Some(9));
        assert_eq!(info.files[1].signature, None);
    }

    #[test]
    fn nested_and_level_lists() {
        let node = parse_yaml("a:\n- x\n- y\nb: 'it''s'\nc: \"\\u00e9\"\nd: []\n").unwrap();
        assert_eq!(
            node.get("a"),
            Some(&Node::List(vec![
                Node::Scalar("x".into()),
                Node::Scalar("y".into())
            ]))
        );
        assert_eq!(node.get("b").and_then(Node::text), Some("it's"));
        assert_eq!(node.get("c").and_then(Node::text), Some("é"));
        assert_eq!(node.get("d"), Some(&Node::List(Vec::new())));
    }
}
