use super::*;
use std::fs;

/// A folder of the drive with a live page in it, and the shell's resources beside it.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    resources: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("Site");
    let resources = dir.path().join("Resources");
    for folder in [
        root.join("scenes"),
        root.join("media"),
        root.join("fonts"),
        resources.join("vendor/gsap-3.15.0"),
        resources.join("fonts"),
    ] {
        fs::create_dir_all(folder).expect("folder");
    }
    fs::write(root.join("index.html"), "<p>index</p>").expect("index");
    fs::write(
        root.join("scenes/a.html"),
        "<!doctype html><html><head><title>a</title></head><body></body></html>",
    )
    .expect("page");
    fs::write(root.join("scenes/a b.html"), "<p>spaced</p>").expect("page");
    fs::write(root.join("media/clip.mp4"), (0..100u8).collect::<Vec<_>>()).expect("clip");
    fs::write(root.join("media/data.json"), "{}").expect("json");
    fs::write(root.join("fonts/Checker-Regular.ttf"), b"ttf").expect("font");
    fs::write(
        resources.join("vendor/gsap-3.15.0/gsap.min.js"),
        "/* gsap */",
    )
    .expect("gsap");
    fs::write(resources.join("fonts/Inter-latin.woff2"), b"woff2").expect("font");
    fs::write(dir.path().join("outside.txt"), "outside").expect("outside");
    Fixture {
        _dir: dir,
        root,
        resources,
    }
}

/// The live page `index.html` over the fixture's folder.
fn folder_host(fixture: &Fixture) -> Host {
    Host::page(&fixture.root.join("index.html"), &fixture.resources)
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply
        .headers
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.as_str())
}

fn bytes(reply: &Reply) -> Vec<u8> {
    match &reply.body {
        Body::Bytes(bytes) => bytes.clone(),
        Body::File {
            path,
            offset,
            length,
        } => {
            let all = fs::read(path).expect("file");
            all[*offset as usize..(*offset + *length) as usize].to_vec()
        }
    }
}

#[test]
fn a_file_of_the_folder_is_served_whole_with_the_headers_pages_rely_on() {
    let fixture = fixture();
    let host = folder_host(&fixture);
    let answer = host.answer("nr://files/media/clip.mp4", None);
    assert_eq!(answer.status, 200);
    assert_eq!(header(&answer, "Content-Type"), Some("video/mp4"));
    assert_eq!(header(&answer, "Accept-Ranges"), Some("bytes"));
    assert_eq!(header(&answer, "Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(header(&answer, "Cache-Control"), Some("no-store"));
    assert_eq!(header(&answer, "Content-Length"), Some("100"));
    assert_eq!(header(&answer, "Content-Range"), None);
    // Media is left in its file for the shell to read.
    assert!(matches!(
        &answer.body,
        Body::File {
            offset: 0,
            length: 100,
            ..
        }
    ));
    assert_eq!(bytes(&answer), (0..100u8).collect::<Vec<_>>());
    let log = host.log_json();
    assert_eq!(log["requests"], 1);
    assert_eq!(log["rangeRequests"], 0);
    assert_eq!(log["served"], json!(["media/clip.mp4"]));
}

#[test]
fn a_range_header_is_read_as_one_span_inside_the_file() {
    assert_eq!(span(None, 100), Span::Whole);
    assert_eq!(
        span(Some("bytes=0-9"), 100),
        Span::Part { first: 0, last: 9 }
    );
    assert_eq!(
        span(Some("bytes=90-"), 100),
        Span::Part {
            first: 90,
            last: 99
        }
    );
    assert_eq!(
        span(Some("bytes=-10"), 100),
        Span::Part {
            first: 90,
            last: 99
        }
    );
    assert_eq!(
        span(Some("bytes=-500"), 100),
        Span::Part { first: 0, last: 99 }
    );
    assert_eq!(
        span(Some("bytes=50-500"), 100),
        Span::Part {
            first: 50,
            last: 99
        }
    );
    assert_eq!(
        span(Some("bytes=+5-6"), 100),
        Span::Part { first: 5, last: 6 }
    );
    // Several ranges, or not bytes: the whole file, as a plain answer.
    assert_eq!(span(Some("bytes=0-1,5-6"), 100), Span::Whole);
    assert_eq!(span(Some("items=0-1"), 100), Span::Whole);
    // Unparseable numbers serve the whole file.
    assert_eq!(
        span(Some("bytes=abc"), 100),
        Span::Part { first: 0, last: 99 }
    );
    assert_eq!(
        span(Some("bytes=-"), 100),
        Span::Part { first: 0, last: 99 }
    );
    assert_eq!(
        span(Some("bytes=1-2-3"), 100),
        Span::Part { first: 0, last: 99 }
    );
    assert_eq!(
        span(Some("bytes=5-x"), 100),
        Span::Part { first: 5, last: 99 }
    );
    // Past the end, backwards, empty.
    assert_eq!(span(Some("bytes=100-"), 100), Span::Unsatisfiable);
    assert_eq!(span(Some("bytes=9-3"), 100), Span::Unsatisfiable);
    assert_eq!(span(Some("bytes=-0"), 100), Span::Unsatisfiable);
    assert_eq!(span(Some("bytes=0-"), 0), Span::Unsatisfiable);
}

#[test]
fn a_range_is_answered_with_its_part_or_416() {
    let fixture = fixture();
    let host = folder_host(&fixture);
    let part = host.answer("nr://files/media/clip.mp4", Some("bytes=10-19"));
    assert_eq!(part.status, 206);
    assert_eq!(header(&part, "Content-Range"), Some("bytes 10-19/100"));
    assert_eq!(header(&part, "Content-Length"), Some("10"));
    assert_eq!(bytes(&part), (10..20u8).collect::<Vec<_>>());
    let text = host.answer("nr://files/media/data.json", Some("bytes=1-"));
    assert_eq!((text.status, bytes(&text)), (206, b"}".to_vec()));
    let past = host.answer("nr://files/media/clip.mp4", Some("bytes=200-"));
    assert_eq!(past.status, 416);
    assert_eq!(
        past.headers,
        vec![
            ("Content-Range", "bytes */100".to_owned()),
            ("Content-Length", "0".to_owned())
        ]
    );
    let log = host.log_json();
    assert_eq!(
        (log["requests"].as_u64(), log["rangeRequests"].as_u64()),
        (Some(3), Some(3))
    );
}

// Uses Unix symlinks.
#[cfg(unix)]
#[test]
fn nothing_outside_the_folder_is_served() {
    let fixture = fixture();
    std::os::unix::fs::symlink(
        fixture.root.parent().expect("parent").join("outside.txt"),
        fixture.root.join("leak.txt"),
    )
    .expect("link");
    std::os::unix::fs::symlink(
        fixture.root.parent().expect("parent"),
        fixture.root.join("leakdir"),
    )
    .expect("link");
    let host = folder_host(&fixture);
    for url in [
        "nr://files/../outside.txt",
        "nr://files/%2e%2e/outside.txt",
        "nr://files/leak.txt",
        "nr://files/leakdir/outside.txt",
        "nr://files/leakdir/nothing.txt",
        "nr://other/scenes/a.html",
    ] {
        assert_eq!(host.answer(url, None).status, 403, "{url}");
    }
    assert_eq!(
        host.lines(Line::Refused),
        [
            "/../outside.txt is outside the page's folder",
            "/leak.txt is outside the page's folder",
            "/leakdir/outside.txt is outside the page's folder",
            "/leakdir/nothing.txt is outside the page's folder",
            "/scenes/a.html is outside the page's folder",
        ]
    );
    // Refusals are not counted as requests.
    assert_eq!(host.log_json()["requests"], 0);
}

#[test]
fn what_is_not_there_is_missing_and_logged_once() {
    let fixture = fixture();
    let host = folder_host(&fixture);
    for url in [
        "nr://files/media/none.png",
        "nr://files//media/none.png",
        "nr://files/scenes",
        "nr://files/scenes/",
        "nr://files/",
        "nr://files/__nr/vendor/none.js",
        "nr://files/__nr/fonts/None.woff2",
    ] {
        assert_eq!(host.answer(url, None).status, 404, "{url}");
    }
    assert_eq!(
        host.lines(Line::Missing),
        [
            "media/none.png",
            "scenes",
            "",
            "__nr/vendor/none.js",
            "__nr/fonts/None.woff2"
        ]
    );
    let fresh: Vec<_> = std::iter::from_fn(|| host.take_line())
        .map(|(line, text)| (line.name(), text))
        .collect();
    assert_eq!(fresh.len(), 5);
    assert_eq!(fresh[0], ("missing", "media/none.png".to_owned()));
    assert_eq!(host.take_line(), None);
}

#[test]
fn addresses_are_decoded_once() {
    let fixture = fixture();
    fs::write(fixture.root.join("media/100%.png"), b"png").expect("file");
    fs::write(fixture.root.join("media/a%41.png"), b"png").expect("file");
    let host = folder_host(&fixture);
    assert!(
        bytes(&host.answer("nr://files/scenes/a%20b.html?x=1#v2", None))
            .ends_with(b"<p>spaced</p>")
    );
    assert_eq!(host.answer("nr://files/media/100%25.png", None).status, 200);
    // A literal `%41` in a name is not read as `A`.
    assert_eq!(host.answer("nr://files/media/a%2541.png", None).status, 200);
    assert_eq!(host.answer("nr://files/media/a%ZZ.png", None).status, 404);
    assert_eq!(host.lines(Line::Missing), ["media/a%ZZ.png"]);
}

#[test]
fn every_kind_of_file_is_served_with_its_mime_type() {
    for (name, wanted) in [
        ("a.HTML", "text/html; charset=utf-8"),
        ("a.mjs", "text/javascript; charset=utf-8"),
        ("a.json", "application/json"),
        ("a.css", "text/css; charset=utf-8"),
        ("a.vtt", "text/plain; charset=utf-8"),
        ("a.jpeg", "image/jpeg"),
        ("a.svg", "image/svg+xml"),
        ("a.m4v", "video/mp4"),
        ("a.mov", "video/quicktime"),
        ("a.aac", "audio/mp4"),
        ("a.wav", "audio/wav"),
        ("a.woff2", "font/woff2"),
        ("a.otf", "font/otf"),
        ("a.glb", "model/gltf-binary"),
        ("a.gltf", "model/gltf+json"),
        ("a.ktx2", "application/octet-stream"),
        ("a.wasm", "application/wasm"),
        ("a.PDF", "application/pdf"),
        ("a", "application/octet-stream"),
    ] {
        assert_eq!(mime(Path::new(name)), wanted, "{name}");
    }
}

#[test]
fn the_host_serves_its_own_libraries() {
    let fixture = fixture();
    let host = folder_host(&fixture);
    let gsap = host.answer("nr://files/__nr/vendor/gsap-3.15.0/gsap.min.js", None);
    assert_eq!((gsap.status, bytes(&gsap)), (200, b"/* gsap */".to_vec()));
    assert_eq!(
        host.answer("nr://files/__nr/vendor/../../outside.txt", None)
            .status,
        403
    );
    // Without its trailing slash the address names no library, and the rest of `__nr/` holds
    // nothing: missing, never read from the page's folder.
    assert_eq!(host.answer("nr://files/__nr/vendor/", None).status, 404);
    // The library is the host's, not the folder's: not among the files the pages read.
    assert_eq!(host.log_json()["served"], json!([]));
    assert_eq!(host.answer("nr://files/__nr/other.html", None).status, 404);
    assert_eq!(
        host.lines(Line::Missing),
        ["__nr/vendor", "__nr/other.html"]
    );
}

#[test]
fn a_live_page_and_a_still_serve_their_own_folder() {
    let fixture = fixture();
    let page = Host::page(&fixture.root.join("scenes/a b.html"), &fixture.resources);
    assert_eq!(page.entry(), "nr://files/a%20b.html");
    assert_eq!(
        bytes(&page.answer("nr://files/a.html", None))[..9],
        *b"<!doctype"
    );
    assert_eq!(
        page.answer("nr://files/../media/clip.mp4", None).status,
        403
    );
    // A live page has no stage.
    assert_eq!(page.answer("nr://files/__nr/stage.html", None).status, 404);
    let still = Host::stills(&fixture.resources);
    assert_eq!(still.entry(), "nr://files/__nr/stage.html");
    let text =
        String::from_utf8(bytes(&still.answer("nr://files/__nr/stage.html", None))).expect("text");
    let import_map = text
        .split_once("<script type=\"importmap\">")
        .and_then(|(_, script)| script.split_once("</script>"))
        .map(|(json, _)| serde_json::from_str::<Value>(json).expect("import map"))
        .expect("import map script");
    assert_eq!(
        import_map,
        json!({"imports": {"three": "nr://files/__nr/vendor/three-0.186.0/three.module.js", "three/addons/": "nr://files/__nr/vendor/three-0.186.0/addons/"}})
    );
    assert!(
        text.contains("async function drawStill(bytes)")
            && text.contains("window.__nrDrawStill = async (address, turn) => {")
            && text.contains("const say = (message) => window.__nrPost(message);"),
        "{text}"
    );
    assert!(text.ends_with("</html>"));
}

#[test]
fn a_still_host_serves_only_the_model_it_draws_now_and_its_folder() {
    let fixture = fixture();
    let still = Host::stills(&fixture.resources);
    // Nothing of its own: no folder until a model is handed to it.
    assert_eq!(still.answer("nr://files/scenes/a.html", None).status, 403);
    assert_eq!(
        still.answer("nr://files/__nr/model/1/a.glb", None).status,
        403
    );

    fs::write(fixture.root.join("media/robot arm.glb"), b"glTF").expect("model");
    let (call, turn) = still.draw_model(&fixture.root.join("media/robot arm.glb"));
    assert_eq!(turn, 1);
    assert_eq!(
        call,
        r#"window.__nrDrawStill("nr://files/__nr/model/1/robot%20arm.glb", 1)"#
    );
    assert_eq!(
        bytes(&still.answer("nr://files/__nr/model/1/robot%20arm.glb", None)),
        b"glTF"
    );
    // What the model reads beside it, and nothing outside its folder.
    assert_eq!(
        still
            .answer("nr://files/__nr/model/1/data.json", None)
            .status,
        200
    );
    assert_eq!(
        still
            .answer("nr://files/__nr/model/1/../scenes/a.html", None)
            .status,
        403
    );
    assert_eq!(
        still
            .answer("nr://files/__nr/model/1/gone.bin", None)
            .status,
        404
    );

    // The next model takes the next turn; the one before is no longer served.
    let (_, turn) = still.draw_model(&fixture.root.join("scenes/a.html"));
    assert_eq!(turn, 2);
    assert_eq!(
        still
            .answer("nr://files/__nr/model/1/robot%20arm.glb", None)
            .status,
        403
    );
    assert_eq!(
        still.answer("nr://files/__nr/model/2/a.html", None).status,
        200
    );
}

#[test]
fn pages_and_scripts_are_rewritten_as_they_are_served() {
    let fixture = fixture();
    fs::write(fixture.root.join("scenes/b.html"), "\u{feff}<html><head><link href=\"https://fonts.googleapis.com/css2?family=Inter\" rel=\"stylesheet\"></head></html>").expect("page");
    fs::write(
        fixture.root.join("scenes/c.js"),
        "\u{feff}import 'https://unpkg.com/gsap@3.12.5/dist/ScrollTrigger.min.js';",
    )
    .expect("script");
    fs::write(
        fixture.root.join("scenes/d.js"),
        b"\xff\xfe not text https://unpkg.com/gsap/dist/all.js",
    )
    .expect("bytes");
    let host = folder_host(&fixture);
    let page =
        String::from_utf8(bytes(&host.answer("nr://files/scenes/b.html", None))).expect("text");
    assert!(
        page.starts_with("<html><head><script type=\"importmap\">"),
        "{page}"
    );
    assert!(!page.contains("googleapis"), "{page}");
    let script =
        String::from_utf8(bytes(&host.answer("nr://files/scenes/c.js", None))).expect("text");
    assert_eq!(
        script,
        "import 'nr://files/__nr/vendor/gsap-3.15.0/ScrollTrigger.min.js';"
    );
    // Bytes that are not UTF-8 are served as they are.
    assert_eq!(
        bytes(&host.answer("nr://files/scenes/d.js", None)),
        b"\xff\xfe not text https://unpkg.com/gsap/dist/all.js".to_vec()
    );
}

#[test]
fn cdn_links_become_the_pinned_copies() {
    let vendor = "nr://files/__nr/vendor";
    for (link, wanted) in [
        (
            "https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/all.min.js",
            format!("{vendor}/gsap-3.15.0/gsap.min.js"),
        ),
        (
            "https://cdn.jsdelivr.net/npm/gsap@3.12.5/dist/all.js",
            format!("{vendor}/gsap-3.15.0/gsap.min.js"),
        ),
        (
            "https://cdn.jsdelivr.net/npm/gsap@3.12.5/dist/gsap.min.js",
            format!("{vendor}/gsap-3.15.0/gsap.min.js"),
        ),
        (
            "https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/CustomEase.min.js",
            format!("{vendor}/gsap-3.15.0/CustomEase.min.js"),
        ),
        (
            "http://unpkg.com/gsap/dist/MotionPathPlugin.js",
            format!("{vendor}/gsap-3.15.0/MotionPathPlugin.min.js"),
        ),
        (
            "https://esm.sh/gsap/gsap.js",
            format!("{vendor}/gsap-3.15.0/gsap.min.js"),
        ),
        (
            "https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.module.js",
            format!("{vendor}/three-0.186.0/three.module.js"),
        ),
        (
            "https://unpkg.com/three@0.150/build/three.module.min.js",
            format!("{vendor}/three-0.186.0/three.module.js"),
        ),
        (
            "https://cdnjs.cloudflare.com/ajax/libs/three.js/r128/build/three.module.js",
            format!("{vendor}/three-0.186.0/three.module.js"),
        ),
        (
            "https://cdn.jsdelivr.net/npm/three@0.170.0/build/three.webgpu.js",
            format!("{vendor}/three-0.186.0/three.webgpu.js"),
        ),
        (
            "https://cdn.jsdelivr.net/npm/three@0.160.0/examples/jsm/loaders/GLTFLoader.js",
            format!("{vendor}/three-0.186.0/addons/loaders/GLTFLoader.js"),
        ),
        (
            "https://unpkg.com/three/examples/jsm/controls/OrbitControls.js",
            format!("{vendor}/three-0.186.0/addons/controls/OrbitControls.js"),
        ),
        (
            "https://fonts.googleapis.com/css2?family=Inter:wght@400;700&display=swap",
            "nr://files/__nr/fonts.css?family=Inter:wght@400;700&display=swap".to_owned(),
        ),
        (
            "http://fonts.googleapis.com/css?family=Lobster",
            "nr://files/__nr/fonts.css?family=Lobster".to_owned(),
        ),
    ] {
        assert_eq!(
            rewrite_links(&format!("<script src=\"{link}\"></script>")),
            format!("<script src=\"{wanted}\"></script>"),
            "{link}"
        );
    }
    // Not a library link, or not from a CDN: left alone.
    for text in [
        "https://example.com/gsap@3/dist/all.js",
        "https://cdn.jsdelivr.net/npm/gsap@3.12.5/+esm",
        "https://cdn.jsdelivr.net/npm/gsap@/dist/all.js",
        "https://unpkg.com/three/build/three.cjs",
        "https://unpkg.com/three/r128/examples/jsm/x.js",
        "https://fonts.gstatic.com/s/inter/v12/a.woff2",
        "no links",
    ] {
        assert_eq!(rewrite_links(text), text);
    }
    // Several links, and a version ending at a quote.
    let both = rewrite_links("'https://unpkg.com/gsap@3' 'https://unpkg.com/gsap@3/dist/Flip.js' 'https://unpkg.com/three@1/build/three.module.js'");
    assert_eq!(both, format!("'https://unpkg.com/gsap@3' '{vendor}/gsap-3.15.0/Flip.min.js' '{vendor}/three-0.186.0/three.module.js'"));
}

#[test]
fn a_page_gets_an_import_map_and_loses_its_font_host_links() {
    let map = import_map();
    assert!(map.starts_with(r#"<script type="importmap">{"imports":{"three":"nr://files/__nr/vendor/three-0.186.0/three.module.js","three/webgpu":"#));
    assert!(map.ends_with(r#""gsap":"nr://files/__nr/vendor/gsap-3.15.0/gsap.min.js"}}</script>"#));
    assert_eq!(
        prepare_page("<HTML><Head lang=\"en\"><title>t</title></head>"),
        format!("<HTML><Head lang=\"en\">{map}<title>t</title></head>")
    );
    assert_eq!(
        prepare_page("<!DOCTYPE html><body>x</body>"),
        format!("<!DOCTYPE html>{map}<body>x</body>")
    );
    assert_eq!(prepare_page("<p>x</p>"), format!("{map}<p>x</p>"));
    // `<head` also starts `<header`: the map goes after that tag too.
    assert_eq!(
        prepare_page("<header>x</header>"),
        format!("<header>{map}x</header>")
    );
    for own in [
        "<head><script type=\"importmap\">{}</script></head>",
        "<script TYPE = 'ImportMap'>{}</script>",
        "<script type=\"importmap'>{}</script>",
    ] {
        assert_eq!(prepare_page(own), own);
    }
    assert_eq!(
        prepare_page("<script type=importmap>{}</script>"),
        format!("{map}<script type=importmap>{{}}</script>")
    );
    let page = "<head><link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin><LINK rel=preconnect href=\"https://FONTS.googleapis.com\"><link href=\"style.css\" rel=\"stylesheet\"><link href=\"https://fonts.googleapis.com/css2?family=Inter\" rel=\"stylesheet\"></head>";
    // The stylesheet link itself now names the host's fonts.css and stays.
    assert_eq!(prepare_page(page), format!("<head>{map}<link href=\"style.css\" rel=\"stylesheet\"><link href=\"nr://files/__nr/fonts.css?family=Inter\" rel=\"stylesheet\"></head>"));
}

#[test]
fn font_files_are_matched_to_families_by_name() {
    let folder = tempfile::tempdir().expect("temp dir");
    for name in [
        "Inter-latin.woff2",
        "Inter-Bold.ttf",
        "Inter-BoldItalic.otf",
        "Inter-400.woff",
        "Inter Tight-Regular.woff2",
        "Inter[wght].ttf",
        "InterVariable-Italic.ttf",
        "Inter-BoldLight.ttf",
        "Inter-2000.ttf",
        "Inter.txt",
        "._Inter-Black.ttf",
    ] {
        fs::write(folder.path().join(name), b"x").expect("write");
    }
    let found = faces(folder.path(), "Inter");
    let named: Vec<(&str, Option<u32>, bool, &str)> = found
        .iter()
        .map(|face| (face.file.as_str(), face.weight, face.italic, face.format))
        .collect();
    assert_eq!(
        named,
        [
            ("Inter-2000.ttf", None, false, "truetype"),
            ("Inter-400.woff", Some(400), false, "woff"),
            ("Inter-Bold.ttf", Some(700), false, "truetype"),
            ("Inter-BoldItalic.otf", Some(700), true, "opentype"),
            ("Inter-latin.woff2", None, false, "woff2"),
            ("InterVariable-Italic.ttf", None, true, "truetype"),
            ("Inter[wght].ttf", None, false, "truetype"),
        ]
    );
    assert_eq!(faces(folder.path(), "Inter Tight").len(), 1);
    assert!(faces(folder.path(), "Roboto").is_empty());
    assert!(faces(folder.path(), "  ").is_empty());
    assert!(faces(&folder.path().join("none"), "Inter").is_empty());
}

#[test]
fn a_google_fonts_link_is_answered_from_local_faces() {
    let fixture = fixture();
    let host = folder_host(&fixture);
    let css = host.answer("nr://files/__nr/fonts.css?family=Inter:wght@400;700&family=Checker&family=Lobster|Pacifico&display=swap", None);
    assert_eq!(
        header(&css, "Content-Type"),
        Some("text/css; charset=utf-8")
    );
    let text = String::from_utf8(bytes(&css)).expect("text");
    assert_eq!(
        text,
        "@font-face{font-family:'Inter';src:url('nr://files/__nr/fonts/Inter-latin.woff2') format('woff2');font-weight:100 900;font-style:normal;font-display:block}\n\
         @font-face{font-family:'Checker';src:url('nr://files/__nr/fonts/Checker-Regular.ttf') format('truetype');font-weight:400;font-style:normal;font-display:block}\n"
    );
    assert_eq!(
        host.lines(Line::Font),
        [
            "Lobster is not packaged and not in the page's fonts/ folder; the page falls back to another face",
            "Pacifico is not packaged and not in the page's fonts/ folder; the page falls back to another face",
        ]
    );
    assert_eq!(
        host.stylesheet("family=Open+Sans:ital@1&=family=Open%20Sans|&family=&family"),
        ""
    );
    assert_eq!(host.lines(Line::Font).len(), 3, "Open Sans once");
    // The faces the stylesheet names are served, the folder's own first, and count among the
    // files the pages read.
    assert_eq!(
        bytes(&host.answer("nr://files/__nr/fonts/Checker-Regular.ttf", None)),
        b"ttf"
    );
    assert_eq!(
        bytes(&host.answer("nr://files/__nr/fonts/Inter-latin.woff2", None)),
        b"woff2"
    );
    assert_eq!(
        host.answer("nr://files/__nr/fonts/../scenes/a.html", None)
            .status,
        404
    );
    assert_eq!(
        host.log_json()["served"],
        json!(["fonts/Checker-Regular.ttf"])
    );
}

// Uses Unix symlinks.
#[cfg(unix)]
#[test]
fn a_film_font_linked_out_of_the_folder_is_not_served() {
    let fixture = fixture();
    std::os::unix::fs::symlink(
        fixture.root.parent().expect("parent").join("outside.txt"),
        fixture.root.join("fonts/Leak-Regular.ttf"),
    )
    .expect("link");
    let host = folder_host(&fixture);
    assert_eq!(
        host.answer("nr://files/__nr/fonts/Leak-Regular.ttf", None)
            .status,
        404
    );
}

// Uses Unix symlinks.
#[cfg(unix)]
#[test]
fn a_path_through_more_links_than_are_followed_is_refused() {
    let fixture = fixture();
    let link = |from: &Path, to: &Path| std::os::unix::fs::symlink(to, from).expect("link");
    // A short chain inside the folder is followed to its file.
    link(&fixture.root.join("near"), Path::new("media/data.json"));
    // A chain one link longer than the limit (Windows follows 63), ending outside the folder.
    let most = MOST_LINKS as usize;
    for at in 0..most {
        link(
            &fixture.root.join(format!("chain{at}")),
            Path::new(&format!("chain{}", at + 1)),
        );
    }
    link(
        &fixture.root.join(format!("chain{most}")),
        &fixture.root.parent().expect("parent").join("outside.txt"),
    );
    let host = folder_host(&fixture);
    assert_eq!(bytes(&host.answer("nr://files/near", None)), b"{}");
    assert_eq!(host.answer("nr://files/chain0", None).status, 403);
    assert_eq!(host.answer("nr://files/chain1", None).status, 403);
}

// Uses a Unix named pipe.
#[cfg(unix)]
#[test]
fn only_plain_files_are_read() {
    let fixture = fixture();
    for pipe in [
        "media/pipe.mp4",
        "scenes/pipe.html",
        "fonts/Pipe-Regular.ttf",
    ] {
        let path = std::ffi::CString::new(fixture.root.join(pipe).as_os_str().as_encoded_bytes())
            .expect("path");
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "{pipe}");
    }
    let host = folder_host(&fixture);
    // Opening a pipe for reading blocks until a writer appears, so none of these may be opened.
    for url in [
        "nr://files/media/pipe.mp4",
        "nr://files/scenes/pipe.html",
        "nr://files/__nr/fonts/Pipe-Regular.ttf",
    ] {
        assert_eq!(host.answer(url, None).status, 404, "{url}");
    }
}

#[test]
fn a_page_s_links_and_a_still_s_frames_stay_local() {
    let fixture = fixture();
    let page = Host::page(&fixture.root.join("scenes/a.html"), &fixture.resources);
    assert!(page.navigate("nr://files/a%20b.html", true, false));
    assert!(
        !page.navigate("https://example.com/", true, false)
            && !page.navigate("wss://example.com/", false, false)
    );
    assert_eq!(
        page.lines(Line::Blocked),
        [
            "navigation https://example.com/",
            "frame wss://example.com/"
        ]
    );
    let still = Host::stills(&fixture.resources);
    assert!(
        still.navigate("nr://files/__nr/stage.html", true, false)
            && still.navigate("nr://files/x", false, false)
    );
    assert!(
        !still.navigate("about:blank", false, false)
            && !still.navigate("https://example.com/", true, false)
    );
    assert!(still.lines(Line::Blocked).is_empty());
}

#[test]
fn page_messages_become_log_lines() {
    let fixture = fixture();
    let host = folder_host(&fixture);
    assert_eq!(host.page_message(r#"{"type":"blocked","kind":"fetch","url":"https://example.com/ping","page":"scenes/a.html"}"#), None);
    assert_eq!(
        host.page_message(r#"{"type":"blocked","page":"scenes/a.html"}"#),
        None
    );
    assert_eq!(
        host.page_message(r#"{"type":"error","text":"boom (a.html:3)","page":"scenes/a.html"}"#),
        None
    );
    assert_eq!(
        host.page_message(r#"{"type":"error","text":"unhandled rejection: x"}"#),
        None
    );
    assert_eq!(
        host.page_message(r#"{"type":"reloaded","page":"scenes/a.html"}"#),
        Some("scenes/a.html".to_owned())
    );
    assert_eq!(
        host.page_message(r#"{"type":"trace","text":"loaded"}"#),
        None
    );
    assert_eq!(host.page_message("not json"), None);
    assert_eq!(
        host.lines(Line::Blocked),
        [
            "fetch https://example.com/ping (scenes/a.html)",
            "request  (scenes/a.html)"
        ]
    );
    assert_eq!(
        host.lines(Line::Error),
        ["scenes/a.html: boom (a.html:3)", "unhandled rejection: x"]
    );
    // Once each, until cleared; counts stay.
    host.page_message(r#"{"type":"error","text":"boom (a.html:3)","page":"scenes/a.html"}"#);
    assert_eq!(std::iter::from_fn(|| host.take_line()).count(), 4);
    host.clear_log();
    assert!(host.lines(Line::Error).is_empty());
    host.page_message(r#"{"type":"error","text":"boom (a.html:3)","page":"scenes/a.html"}"#);
    assert_eq!(host.lines(Line::Error).len(), 1);
}

#[test]
fn problems_are_told_in_the_hosts_words() {
    assert_eq!(
        Problem::StageTimeout.text(""),
        "the stage did not load in 30 s"
    );
    assert_eq!(
        Problem::StillTimeout.text(""),
        "the model was not drawn in 30 s"
    );
    assert_eq!(
        Problem::ShowFailed.text("TypeError"),
        "the stage could not show the frame: TypeError"
    );
    assert_eq!(
        Problem::RulesStored.text("/tmp/x"),
        "the network block list could not be stored in /tmp/x"
    );
    // The platform gave no reason.
    assert_eq!(
        Problem::RulesFailed.text(""),
        "the network block list could not be made: unknown"
    );
    assert_eq!(
        Problem::NoPicture.text(""),
        "the web view gave no picture: unknown"
    );
    assert_eq!(
        Problem::PictureUnsized {
            width: 1920,
            height: 1080
        }
        .text(""),
        "the picture could not be drawn at 1920×1080"
    );
    assert_eq!(
        Problem::ExportCancelled.text(""),
        "the export was cancelled; no file was left"
    );
    assert_eq!(
        Problem::StepUnread.text("{}"),
        "the export's next step could not be read: {}"
    );
    assert_eq!(
        Problem::PngFailed { frame: 3 }.text(""),
        "frame 3 could not be encoded as PNG"
    );
    assert_eq!(
        Problem::FrameEarly { frame: 0 }.text(""),
        "frame 0 came before the movie was open"
    );
    assert_eq!(
        Problem::EncoderMissing.text("hevc"),
        if cfg!(target_os = "macos") {
            "the hevc encoder is not available on this Mac"
        } else {
            "the hevc encoder is not available on this computer"
        }
    );
    assert_eq!(
        Problem::EncoderStopped.text(""),
        "the encoder stopped: unknown"
    );
    assert_eq!(
        Problem::FrameUnencoded { frame: 7 }.text("disk full"),
        "frame 7 could not be encoded: disk full"
    );
    assert_eq!(
        Problem::SoundUnwritten.text(""),
        "the sound could not be written: unknown"
    );
    assert_eq!(
        Problem::Unfinished.text("gone"),
        "the file could not be finished: gone"
    );
    assert_eq!(
        Problem::SoundOutput.text("busy"),
        "a player's sound output did not start: busy"
    );
    assert_eq!(Problem::NoStep.text("zoom"), "no step zoom");
    assert_eq!(
        Problem::CoreUnread.text("{"),
        "the core's answer could not be read: {"
    );
    assert_eq!(
        Problem::NotHere.text(""),
        "scene pages are not shown on this platform yet"
    );
}

#[test]
fn a_trace_line_says_when() {
    assert_eq!(trace_line(1.23456, "shown"), "host 1.235: shown\n");
}

#[test]
fn one_block_list_stops_everything_off_the_machine() {
    let rules: Value = serde_json::from_str(BLOCK_RULES).expect("rules");
    let filters: Vec<&str> = rules
        .as_array()
        .expect("list")
        .iter()
        .map(|rule| rule["trigger"]["url-filter"].as_str().expect("filter"))
        .collect();
    assert_eq!(filters, ["^https?://", "^wss?://", "^ftp://"]);
    assert!(rules
        .as_array()
        .expect("list")
        .iter()
        .all(|rule| rule["action"]["type"] == "block"));
}

#[test]
fn a_transparent_picture_is_had_from_black_and_white() {
    // Opaque red; half-covered white (premultiplied 128 over black, 255 over white); nothing.
    let black = [255, 0, 0, 255, 128, 128, 128, 255, 0, 0, 0, 255];
    let white = [255, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255];
    let mut out = [0; 12];
    unground(&black, &white, &mut out);
    assert_eq!(out, [255, 0, 0, 255, 128, 128, 128, 128, 0, 0, 0, 0]);
    // A colour is never above its alpha; light darker than dark is opaque.
    let mut out = [0; 8];
    unground(
        &[200, 10, 10, 255, 20, 0, 0, 255],
        &[100, 10, 10, 255, 255, 255, 255, 255],
        &mut out,
    );
    assert_eq!(out, [200, 10, 10, 255, 20, 0, 0, 20]);
}

#[test]
fn templates_fill_once() {
    assert_eq!(
        fill("a{{x}}b{{y}}c{{z}}\n", &[("x", "{{y}}"), ("y", "2")]),
        "a{{y}}b2c{{z}}"
    );
    assert_eq!(fill("{{x}", &[("x", "1")]), "{{x}");
}

#[test]
fn each_host_kind_configures_its_web_view_and_its_crash() {
    // A live page plays as a browser does, a picture but no sound on its own; a still plays
    // nothing. None opens windows, takes the screen or sends media away.
    for (kind, video, sound) in [(Kind::Page, true, false), (Kind::Still, false, false)] {
        let web = kind.web();
        assert_eq!(
            (web.video_plays_alone, web.sound_plays_alone),
            (video, sound),
            "{kind:?}"
        );
        assert!(
            !web.opens_windows && !web.fullscreen && !web.airplay,
            "{kind:?}"
        );
    }
    // Only a live page loads itself again when its process ends.
    assert!(Kind::Page.reloads_on_crash());
    assert!(!Kind::Still.reloads_on_crash());
}

#[test]
fn a_still_is_drawn_on_the_square_its_script_draws_on() {
    assert!(STILL_SCRIPT.contains(&format!("const SIZE = {STILL_STAGE},")));
}

// A page's database calls.

/// The URL `window.__nr.sql` requests: the call as `q`, encoded as `encodeURIComponent` does.
fn sql_address(file: &str, run: Value) -> String {
    let call = json!({ "file": file, "run": run }).to_string();
    let q: String = call
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("nr://files/__nr/sql?q={q}")
}

/// The host's JSON answer to a page's call, which is a 200 on success or failure.
fn asked(host: &Host, file: &str, run: Value) -> Value {
    let reply = host.answer(&sql_address(file, run), None);
    assert_eq!(reply.status, 200);
    assert!(reply
        .headers
        .contains(&("Content-Type", "application/json; charset=utf-8".to_owned())));
    serde_json::from_slice(&bytes(&reply)).expect("the answer is JSON")
}

#[test]
fn a_live_page_makes_reads_and_writes_a_database_of_its_folder() {
    let fixture = fixture();
    let page = Host::page(&fixture.root.join("scenes/a.html"), &fixture.resources);
    // The page's folder is `scenes`: a path from the page, as a link's.
    let made = asked(
        &page,
        "/data.sqlite",
        json!([
            ["CREATE TABLE notes (id INTEGER PRIMARY KEY, body)", []],
            [
                "INSERT INTO notes (body) VALUES (?), (?)",
                ["first", "second"]
            ]
        ]),
    );
    assert_eq!(made["results"][1]["changes"], 2);
    assert!(crate::sheets::database::is_database(
        &fixture.root.join("scenes/data.sqlite")
    ));
    let read = asked(
        &page,
        "data.sqlite",
        json!([["SELECT body FROM notes WHERE id > ? ORDER BY id", [1]]]),
    );
    assert_eq!(read["results"][0]["rows"], json!([{ "body": "second" }]));
    // The database is not a file the page read, so changing it does not reload the page.
    assert!(page.log().served.is_empty());
    assert_eq!(
        asked(&page, "data.sqlite", json!([["SELECT * FROM nope", []]])),
        json!({ "error": "no such table: nope" })
    );
}

#[test]
fn a_page_s_call_reaches_nothing_outside_its_folder() {
    let fixture = fixture();
    let page = Host::page(&fixture.root.join("scenes/a.html"), &fixture.resources);
    let outside = fixture.root.join("media/other.sqlite");
    for file in [
        "../media/other.sqlite",
        "/../media/other.sqlite",
        "%2e%2e/media/other.sqlite",
    ] {
        let answer = asked(&page, file, json!([["CREATE TABLE t (i)", []]]));
        assert!(answer["error"].is_string(), "{file}: {answer}");
    }
    assert!(!outside.exists());
    // Nor by its SQL: nothing is attached, and nothing is written elsewhere.
    asked(&page, "in.sqlite", json!([["CREATE TABLE t (i)", []]]));
    for statement in [
        format!("ATTACH DATABASE '{}' AS other", outside.display()),
        format!("VACUUM INTO '{}'", outside.display()),
    ] {
        let answer = asked(&page, "in.sqlite", json!([[statement, []]]));
        assert!(answer["error"].is_string(), "{statement}: {answer}");
    }
    assert!(!outside.exists());
    // A folder is no database, and a call that is not one says so.
    fs::create_dir(fixture.root.join("scenes/folder")).expect("folder");
    assert_eq!(
        asked(&page, "folder", json!([["SELECT 1", []]])),
        json!({ "error": "folder is not a file" })
    );
    let malformed = page.answer("nr://files/__nr/sql?q=%7B%7D", None);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes(&malformed)).expect("JSON"),
        json!({ "error": "the call is not {file, run}" })
    );
}

#[test]
fn a_still_asks_no_database() {
    let fixture = fixture();
    let still = Host::stills(&fixture.resources);
    assert_eq!(
        asked(
            &still,
            "/media/numbers.sqlite",
            json!([["SELECT i FROM n", []]])
        ),
        json!({ "error": "a still asks no database" })
    );
}

#[test]
fn a_page_is_told_once_when_something_else_changes_a_database_it_asked() {
    let fixture = fixture();
    let page = Host::page(&fixture.root.join("scenes/a.html"), &fixture.resources);
    let file = fixture.root.join("scenes/live.sqlite");
    assert!(!page.data_changed(), "it has asked nothing");
    asked(&page, "live.sqlite", json!([["CREATE TABLE t (i)", []]]));
    asked(
        &page,
        "live.sqlite",
        json!([["INSERT INTO t VALUES (1)", []]]),
    );
    // The page's own writes are not reported back to it.
    assert!(!page.data_changed());
    let change = |statement: &str| {
        let run = crate::sheets::database::asked(&json!([[statement, []]])).expect("a call");
        crate::sheets::database::ask(&file, &run, Access::Writes).expect("a change");
    };
    change("INSERT INTO t SELECT i FROM t UNION ALL SELECT 2 UNION ALL SELECT 3");
    assert!(page.data_changed());
    assert!(!page.data_changed(), "told once");
    // A change is still reported when the page's own next call lands on top of it.
    change("CREATE TABLE more (a, b, c)");
    asked(
        &page,
        "live.sqlite",
        json!([["SELECT count(*) FROM t", []]]),
    );
    assert!(page.data_changed());
    assert!(!page.data_changed());
}

#[test]
fn every_page_is_given_the_database_call() {
    assert!(PAGE_SCRIPT.contains("nr.sql = function (file, sql, values)"));
    assert!(PAGE_SCRIPT.contains("\"/__nr/sql?q=\""));
    assert!(PAGE_SCRIPT.starts_with(PAGE_ERRORS));
    assert_eq!(
        DATA_CHANGED_CALL,
        "window.dispatchEvent(new Event('nr:change')); undefined"
    );
}
