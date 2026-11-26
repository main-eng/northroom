//! The app's single set of glyphs, used by every core model (menu entries, settings pages, rows,
//! tabs, cards). Each is a Lucide (ISC) line drawing on a `GLYPH_BOX`-unit grid, stroked
//! `GLYPH_STROKE` wide with round caps and joins to match Inter's weight at the sizes they sit
//! beside. A shell draws the SVG (`Glyph::svg`) in the view's colour. Across the FFI a glyph is
//! its name (`Glyph::name`).

/// Side of the glyph grid.
pub const GLYPH_BOX: u32 = 24;

/// Stroke width, in grid units.
pub const GLYPH_STROKE: f64 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    ArrowLeft,
    ArrowRight,
    House,
    Folder,
    Eyedropper,
    SlidersHorizontal,
    Plus,
    File,
    Close,
    CaretUp,
    CaretDown,
    CaretRight,
    FilmSlate,
    FilePlus,
    FolderPlus,
    UploadSimple,
    SquaresFour,
    TerminalWindow,
    FolderOpen,
    PencilSimple,
    Trash,
    FileText,
    FileCode,
    Check,
    Gear,
    FilmStrip,
    MusicNote,
    Image,
    Cube,
    Palette,
    Sun,
    Moon,
    Monitor,
    Play,
    PlayFill,
    Pause,
    ArrowsInLineHorizontal,
    Warning,
    DotsSixVertical,
    Copy,
    Clipboard,
    FileZip,
    ArrowSquareOut,
    Export,
    TextT,
    TextHOne,
    TextHTwo,
    TextHThree,
    ListBullets,
    ListNumbers,
    CheckSquare,
    Quotes,
    Code,
    Minus,
    Table,
    CircleNotch,
    MagnifyingGlass,
    ArrowCircleUp,
}

impl Glyph {
    pub const ALL: [Glyph; 58] = [
        Glyph::ArrowLeft,
        Glyph::ArrowRight,
        Glyph::House,
        Glyph::Folder,
        Glyph::Eyedropper,
        Glyph::SlidersHorizontal,
        Glyph::Plus,
        Glyph::File,
        Glyph::Close,
        Glyph::CaretUp,
        Glyph::CaretDown,
        Glyph::CaretRight,
        Glyph::FilmSlate,
        Glyph::FilePlus,
        Glyph::FolderPlus,
        Glyph::UploadSimple,
        Glyph::SquaresFour,
        Glyph::TerminalWindow,
        Glyph::FolderOpen,
        Glyph::PencilSimple,
        Glyph::Trash,
        Glyph::FileText,
        Glyph::FileCode,
        Glyph::Check,
        Glyph::Gear,
        Glyph::FilmStrip,
        Glyph::MusicNote,
        Glyph::Image,
        Glyph::Cube,
        Glyph::Palette,
        Glyph::Sun,
        Glyph::Moon,
        Glyph::Monitor,
        Glyph::Play,
        Glyph::PlayFill,
        Glyph::Pause,
        Glyph::ArrowsInLineHorizontal,
        Glyph::Warning,
        Glyph::DotsSixVertical,
        Glyph::Copy,
        Glyph::Clipboard,
        Glyph::FileZip,
        Glyph::ArrowSquareOut,
        Glyph::Export,
        Glyph::TextT,
        Glyph::TextHOne,
        Glyph::TextHTwo,
        Glyph::TextHThree,
        Glyph::ListBullets,
        Glyph::ListNumbers,
        Glyph::CheckSquare,
        Glyph::Quotes,
        Glyph::Code,
        Glyph::Minus,
        Glyph::Table,
        Glyph::CircleNotch,
        Glyph::MagnifyingGlass,
        Glyph::ArrowCircleUp,
    ];

    /// The name a shell knows the glyph by.
    pub fn name(self) -> &'static str {
        match self {
            Glyph::ArrowLeft => "arrowLeft",
            Glyph::ArrowRight => "arrowRight",
            Glyph::House => "house",
            Glyph::Folder => "folder",
            Glyph::Eyedropper => "eyedropper",
            Glyph::SlidersHorizontal => "slidersHorizontal",
            Glyph::Plus => "plus",
            Glyph::File => "file",
            Glyph::Close => "close",
            Glyph::CaretUp => "caretUp",
            Glyph::CaretDown => "caretDown",
            Glyph::CaretRight => "caretRight",
            Glyph::FilmSlate => "filmSlate",
            Glyph::FilePlus => "filePlus",
            Glyph::FolderPlus => "folderPlus",
            Glyph::UploadSimple => "uploadSimple",
            Glyph::SquaresFour => "squaresFour",
            Glyph::TerminalWindow => "terminalWindow",
            Glyph::FolderOpen => "folderOpen",
            Glyph::PencilSimple => "pencilSimple",
            Glyph::Trash => "trash",
            Glyph::FileText => "fileText",
            Glyph::FileCode => "fileCode",
            Glyph::Check => "check",
            Glyph::Gear => "gear",
            Glyph::FilmStrip => "filmStrip",
            Glyph::MusicNote => "musicNote",
            Glyph::Image => "image",
            Glyph::Cube => "cube",
            Glyph::Palette => "palette",
            Glyph::Sun => "sun",
            Glyph::Moon => "moon",
            Glyph::Monitor => "monitor",
            Glyph::Play => "play",
            Glyph::PlayFill => "playFill",
            Glyph::Pause => "pause",
            Glyph::ArrowsInLineHorizontal => "arrowsInLineHorizontal",
            Glyph::Warning => "warning",
            Glyph::DotsSixVertical => "dotsSixVertical",
            Glyph::Copy => "copy",
            Glyph::Clipboard => "clipboard",
            Glyph::FileZip => "fileZip",
            Glyph::ArrowSquareOut => "arrowSquareOut",
            Glyph::Export => "export",
            Glyph::TextT => "textT",
            Glyph::TextHOne => "textHOne",
            Glyph::TextHTwo => "textHTwo",
            Glyph::TextHThree => "textHThree",
            Glyph::ListBullets => "listBullets",
            Glyph::ListNumbers => "listNumbers",
            Glyph::CheckSquare => "checkSquare",
            Glyph::Quotes => "quotes",
            Glyph::Code => "code",
            Glyph::Minus => "minus",
            Glyph::Table => "table",
            Glyph::CircleNotch => "circleNotch",
            Glyph::MagnifyingGlass => "magnifyingGlass",
            Glyph::ArrowCircleUp => "arrowCircleUp",
        }
    }

    /// The glyph `name` names.
    pub fn named(name: &str) -> Option<Glyph> {
        Self::ALL.into_iter().find(|glyph| glyph.name() == name)
    }

    /// The shapes as SVG elements on the `GLYPH_BOX` grid. They set no stroke (`Glyph::svg`
    /// applies one to all); filled shapes and dots set a black fill.
    pub fn shapes(self) -> &'static str {
        match self {
            Glyph::ArrowLeft => r##"<path d="m12 19l-7-7l7-7m7 7H5"/>"##,
            Glyph::ArrowRight => r##"<path d="M5 12h14m-7-7l7 7l-7 7"/>"##,
            Glyph::House => {
                r##"<path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"/><path d="M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"##
            }
            Glyph::Folder => {
                r##"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"##
            }
            Glyph::Eyedropper => {
                r##"<path d="m12 9l-8.414 8.414A2 2 0 0 0 3 18.828v1.344a2 2 0 0 1-.586 1.414A2 2 0 0 1 3.828 21h1.344a2 2 0 0 0 1.414-.586L15 12"/><path d="m18 9l.4.4a1 1 0 1 1-3 3l-3.8-3.8a1 1 0 1 1 3-3l.4.4l3.4-3.4a1 1 0 1 1 3 3zM2 22l.414-.414"/>"##
            }
            Glyph::SlidersHorizontal => {
                r##"<path d="M10 5H3m9 14H3M14 3v4m2 10v4m5-9h-9m9 7h-5m5-14h-7m-6 5v4m0-2H3"/>"##
            }
            Glyph::Plus => r##"<path d="M5 12h14m-7-7v14"/>"##,
            Glyph::File => {
                r##"<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/>"##
            }
            Glyph::Close => r##"<path d="M18 6L6 18M6 6l12 12"/>"##,
            Glyph::CaretUp => r##"<path d="m18 15l-6-6l-6 6"/>"##,
            Glyph::CaretDown => r##"<path d="m6 9l6 6l6-6"/>"##,
            Glyph::CaretRight => r##"<path d="m9 18l6-6l-6-6"/>"##,
            Glyph::FilmSlate => {
                r##"<path d="m12.296 3.464l3.02 3.956M20.2 6L3 11l-.9-2.4c-.3-1.1.3-2.2 1.3-2.5l13.5-4c1.1-.3 2.2.3 2.5 1.3zM3 11h18v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zm3.18-5.724l3.1 3.899"/>"##
            }
            Glyph::FilePlus => {
                r##"<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5M9 15h6m-3 3v-6"/>"##
            }
            Glyph::FolderPlus => {
                r##"<path d="M12 10v6m-3-3h6m5 7a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"##
            }
            Glyph::UploadSimple => {
                r##"<path d="M12 3v12m5-7l-5-5l-5 5m14 7v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>"##
            }
            Glyph::SquaresFour => {
                r##"<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>"##
            }
            Glyph::TerminalWindow => {
                r##"<path d="m7 11l2-2l-2-2m4 6h4"/><rect width="18" height="18" x="3" y="3" rx="2" ry="2"/>"##
            }
            Glyph::FolderOpen => {
                r##"<path d="m6 14l1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"##
            }
            Glyph::PencilSimple => {
                r##"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497zM15 5l4 4"/>"##
            }
            Glyph::Trash => {
                r##"<path d="M10 11v6m4-6v6m5-11v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"##
            }
            Glyph::FileText => {
                r##"<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5M10 9H8m8 4H8m8 4H8"/>"##
            }
            Glyph::FileCode => {
                r##"<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5m-10 4.5L8 15l2 2.5m4-5l2 2.5l-2 2.5"/>"##
            }
            Glyph::Check => r##"<path d="M20 6L9 17l-5-5"/>"##,
            Glyph::Gear => {
                r##"<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0a2.34 2.34 0 0 0 3.319 1.915a2.34 2.34 0 0 1 2.33 4.033a2.34 2.34 0 0 0 0 3.831a2.34 2.34 0 0 1-2.33 4.033a2.34 2.34 0 0 0-3.319 1.915a2.34 2.34 0 0 1-4.659 0a2.34 2.34 0 0 0-3.32-1.915a2.34 2.34 0 0 1-2.33-4.033a2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>"##
            }
            Glyph::FilmStrip => {
                r##"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M7 3v18M3 7.5h4M3 12h18M3 16.5h4M17 3v18m0-13.5h4m-4 9h4"/>"##
            }
            Glyph::MusicNote => {
                r##"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"##
            }
            Glyph::Image => {
                r##"<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15l-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>"##
            }
            Glyph::Cube => {
                r##"<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7l8.7 5l8.7-5M12 22V12"/>"##
            }
            Glyph::Palette => {
                r##"<path d="M12 22a1 1 0 0 1 0-20a10 9 0 0 1 10 9a5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z"/><circle cx="13.5" cy="6.5" r=".5" fill="#000"/><circle cx="17.5" cy="10.5" r=".5" fill="#000"/><circle cx="6.5" cy="12.5" r=".5" fill="#000"/><circle cx="8.5" cy="7.5" r=".5" fill="#000"/>"##
            }
            Glyph::Sun => {
                r##"<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.93 4.93l1.41 1.41m11.32 11.32l1.41 1.41M2 12h2m16 0h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>"##
            }
            Glyph::Moon => {
                r##"<path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401"/>"##
            }
            Glyph::Monitor => {
                r##"<rect width="20" height="14" x="2" y="3" rx="2"/><path d="M8 21h8m-4-4v4"/>"##
            }
            Glyph::Play => {
                r##"<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"##
            }
            Glyph::PlayFill => {
                r##"<path fill="#000" d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"##
            }
            Glyph::Pause => {
                r##"<rect width="5" height="18" x="14" y="3" rx="1"/><rect width="5" height="18" x="5" y="3" rx="1"/>"##
            }
            Glyph::ArrowsInLineHorizontal => {
                r##"<path d="M2 12h6m14 0h-6M12 2v2m0 4v2m0 4v2m0 4v2m7-13l-3 3l3 3M5 15l3-3l-3-3"/>"##
            }
            Glyph::Warning => {
                r##"<path d="m21.73 18l-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3M12 9v4m0 4h.01"/>"##
            }
            Glyph::DotsSixVertical => {
                r##"<circle cx="9" cy="12" r="1"/><circle cx="9" cy="5" r="1"/><circle cx="9" cy="19" r="1"/><circle cx="15" cy="12" r="1"/><circle cx="15" cy="5" r="1"/><circle cx="15" cy="19" r="1"/>"##
            }
            Glyph::Copy => {
                r##"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"##
            }
            Glyph::Clipboard => {
                r##"<rect width="8" height="4" x="8" y="2" rx="1" ry="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/>"##
            }
            Glyph::FileZip => {
                r##"<path d="M13.659 22H18a2 2 0 0 0 2-2V8a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 14 2H6a2 2 0 0 0-2 2v11.5"/><path d="M14 2v5a1 1 0 0 0 1 1h5M8 12v-1m0 7v-2m0-9V6"/><circle cx="8" cy="20" r="2"/>"##
            }
            Glyph::ArrowSquareOut => {
                r##"<path d="M21 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h6m10 0l-9 9m3-9h6v6"/>"##
            }
            Glyph::Export => {
                r##"<path d="M12 2v13m4-9l-4-4l-4 4m-4 6v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8"/>"##
            }
            Glyph::TextT => {
                r##"<path d="M12 4v16M4 7V5a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v2M9 20h6"/>"##
            }
            Glyph::TextHOne => r##"<path d="M4 12h8m-8 6V6m8 12V6m5 6l3-2v8"/>"##,
            Glyph::TextHTwo => {
                r##"<path d="M4 12h8m-8 6V6m8 12V6m9 12h-4c0-4 4-3 4-6c0-1.5-2-2.5-4-1"/>"##
            }
            Glyph::TextHThree => {
                r##"<path d="M4 12h8m-8 6V6m8 12V6m5.5 4.5c1.7-1 3.5 0 3.5 1.5a2 2 0 0 1-2 2m-2 3.5c2 1.5 4 .3 4-1.5a2 2 0 0 0-2-2"/>"##
            }
            Glyph::ListBullets => {
                r##"<path d="M3 5h.01M3 12h.01M3 19h.01M8 5h13M8 12h13M8 19h13"/>"##
            }
            Glyph::ListNumbers => {
                r##"<path d="M11 5h10m-10 7h10m-10 7h10M4 4h1v5M4 9h2m.5 11H3.4c0-1 2.6-1.925 2.6-3.5a1.5 1.5 0 0 0-2.6-1.02"/>"##
            }
            Glyph::CheckSquare => {
                r##"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="m16 9l-5.5 5.5L8 12"/>"##
            }
            Glyph::Quotes => {
                r##"<path d="M16 3a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2a1 1 0 0 1 1 1v1a2 2 0 0 1-2 2a1 1 0 0 0-1 1v2a1 1 0 0 0 1 1a6 6 0 0 0 6-6V5a2 2 0 0 0-2-2zM5 3a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2a1 1 0 0 1 1 1v1a2 2 0 0 1-2 2a1 1 0 0 0-1 1v2a1 1 0 0 0 1 1a6 6 0 0 0 6-6V5a2 2 0 0 0-2-2z"/>"##
            }
            Glyph::Code => r##"<path d="m16 18l6-6l-6-6M8 6l-6 6l6 6"/>"##,
            Glyph::Minus => r##"<path d="M5 12h14"/>"##,
            Glyph::Table => {
                r##"<path d="M12 3v18"/><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18M3 15h18"/>"##
            }
            Glyph::CircleNotch => r##"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"##,
            Glyph::MagnifyingGlass => {
                r##"<path d="m21 21l-4.34-4.34"/><circle cx="11" cy="11" r="8"/>"##
            }
            Glyph::ArrowCircleUp => {
                r##"<circle cx="12" cy="12" r="10"/><path d="m16 12l-4-4l-4 4m4 4V8"/>"##
            }
        }
    }

    /// The glyph as a full SVG document in black, stroked `GLYPH_STROKE` wide with round caps
    /// and joins. A shell uses it as a mask to paint through.
    pub fn svg(self) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {GLYPH_BOX} {GLYPH_BOX}\" \
             fill=\"none\" stroke=\"#000\" stroke-width=\"{GLYPH_STROKE}\" stroke-linecap=\"round\" \
             stroke-linejoin=\"round\">{}</svg>",
            self.shapes()
        )
    }
}

/// Media control glyphs for the players (`player`): Material icons as Chromium ships them, in a
/// `MEDIA_BOX`-unit box. Each is raw SVG shapes (`<path d=…/>`, `<circle …/>`) that a shell
/// fills, or strokes, in the view's colour. The sound pill has its own glyphs, drawn across its
/// full 32 pt buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaGlyph {
    Play,
    Pause,
    Sound,
    Muted,
    Fullscreen,
    LeaveFullscreen,
    /// The three dots that open the menu.
    More,
    /// The menu's rows: Download, Captions, Playback speed, Picture in Picture.
    Download,
    Captions,
    Speed,
    PictureInPicture,
    /// The tick on the chosen speed, and the back arrow to the menu.
    Tick,
    Back,
    /// The sound pill (`player::sound_controls`): play, pause, speaker on and muted, and the
    /// menu dots.
    PillPlay,
    PillPause,
    PillSound,
    PillMuted,
    PillMore,
}

/// Side of the media glyph box.
pub const MEDIA_BOX: u32 = 24;

impl MediaGlyph {
    pub const ALL: [MediaGlyph; 18] = [
        MediaGlyph::Play,
        MediaGlyph::Pause,
        MediaGlyph::Sound,
        MediaGlyph::Muted,
        MediaGlyph::Fullscreen,
        MediaGlyph::LeaveFullscreen,
        MediaGlyph::More,
        MediaGlyph::Download,
        MediaGlyph::Captions,
        MediaGlyph::Speed,
        MediaGlyph::PictureInPicture,
        MediaGlyph::Tick,
        MediaGlyph::Back,
        MediaGlyph::PillPlay,
        MediaGlyph::PillPause,
        MediaGlyph::PillSound,
        MediaGlyph::PillMuted,
        MediaGlyph::PillMore,
    ];

    /// The name a shell knows the glyph by.
    pub fn name(self) -> &'static str {
        match self {
            MediaGlyph::Play => "mediaPlay",
            MediaGlyph::Pause => "mediaPause",
            MediaGlyph::Sound => "mediaSound",
            MediaGlyph::Muted => "mediaMuted",
            MediaGlyph::Fullscreen => "mediaFullscreen",
            MediaGlyph::LeaveFullscreen => "mediaLeaveFullscreen",
            MediaGlyph::More => "mediaMore",
            MediaGlyph::Download => "mediaDownload",
            MediaGlyph::Captions => "mediaCaptions",
            MediaGlyph::Speed => "mediaSpeed",
            MediaGlyph::PictureInPicture => "mediaPictureInPicture",
            MediaGlyph::Tick => "mediaTick",
            MediaGlyph::Back => "mediaBack",
            MediaGlyph::PillPlay => "mediaPillPlay",
            MediaGlyph::PillPause => "mediaPillPause",
            MediaGlyph::PillSound => "mediaPillSound",
            MediaGlyph::PillMuted => "mediaPillMuted",
            MediaGlyph::PillMore => "mediaPillMore",
        }
    }

    /// The glyph `name` names.
    pub fn named(name: &str) -> Option<MediaGlyph> {
        Self::ALL.into_iter().find(|glyph| glyph.name() == name)
    }

    /// The shapes as SVG elements in a `MEDIA_BOX`-unit box.
    pub fn shapes(self) -> &'static str {
        match self {
            MediaGlyph::Play => r#"<path d="M8 5v14l11-7z"/>"#,
            MediaGlyph::Pause => r#"<path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/>"#,
            MediaGlyph::Sound => {
                r#"<path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/>"#
            }
            MediaGlyph::Muted => {
                r#"<path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z"/>"#
            }
            MediaGlyph::Fullscreen => {
                r#"<path d="M7 14H5v5h5v-2H7v-3zm-2-4h2V7h3V5H5v5zm12 7h-3v2h5v-5h-2v3zM14 5v2h3v3h2V5h-5z"/>"#
            }
            MediaGlyph::LeaveFullscreen => {
                r#"<path d="M5 16h3v3h2v-5H5v2zm3-8H5v2h5V5H8v3zm6 11h2v-3h3v-2h-5v5zm2-11V5h-2v5h5V8h-3z"/>"#
            }
            MediaGlyph::More => {
                r#"<circle cx="12" cy="6" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="18" r="2"/>"#
            }
            MediaGlyph::Download => r#"<path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/>"#,
            MediaGlyph::Captions => {
                r#"<path d="M19 4H5c-1.11 0-2 .9-2 2v12c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm-8 7H9.5v-.5h-2v3h2V13H11v1c0 .55-.45 1-1 1H7c-.55 0-1-.45-1-1v-4c0-.55.45-1 1-1h3c.55 0 1 .45 1 1v1zm7 0h-1.5v-.5h-2v3h2V13H18v1c0 .55-.45 1-1 1h-3c-.55 0-1-.45-1-1v-4c0-.55.45-1 1-1h3c.55 0 1 .45 1 1v1z"/>"#
            }
            MediaGlyph::Speed => {
                r#"<path d="M13.05 9.79L10 7.5v9l3.05-2.29L16 12zM11 4.07V2.05c-2.01.2-3.84 1-5.32 2.21L7.1 5.69c1.11-.86 2.44-1.44 3.9-1.62zM5.69 7.1L4.26 5.68C3.05 7.16 2.25 8.99 2.05 11h2.02c.18-1.46.76-2.79 1.62-3.9zM4.07 13H2.05c.2 2.01 1 3.84 2.21 5.32l1.43-1.43c-.86-1.1-1.44-2.43-1.62-3.89zm1.61 6.74C7.16 20.95 9 21.75 11 21.95v-2.02c-1.46-.18-2.79-.76-3.9-1.62l-1.42 1.43zM22 12c0 5.16-3.92 9.42-8.95 9.95v-2.02C16.97 19.41 20 16.05 20 12s-3.03-7.41-6.95-7.93V2.05C18.08 2.58 22 6.84 22 12z"/>"#
            }
            MediaGlyph::PictureInPicture => {
                r#"<path d="M19 11h-8v6h8v-6zm4 8V4.98C23 3.88 22.1 3 21 3H3c-1.1 0-2 .88-2 1.98V19c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2zm-2 .02H3V4.97h18v14.05z"/>"#
            }
            MediaGlyph::Tick => r#"<path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"/>"#,
            MediaGlyph::Back => {
                r#"<path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>"#
            }
            // Pill glyphs in a 20 pt centre: an 8.5 × 11 triangle, two 4 × 14 bars, a speaker
            // with waves of radius 3.5 and 7 spanning ±50°, or a cross, in 1.6-wide lines; three
            // dots of 3, 5.5 apart.
            MediaGlyph::PillPlay => r#"<path d="M9.75 7.875L16.125 12L9.75 16.125z"/>"#,
            MediaGlyph::PillPause => r#"<path d="M7.5 6.75h3v10.5h-3zM13.5 6.75h3v10.5h-3z"/>"#,
            MediaGlyph::PillSound => concat!(
                r#"<path d="M6 9.75h3l3.75-3v10.5L9 14.25H6z"/>"#,
                r##"<path d="M14.43732 9.98913A2.625 2.625 0 0 1 14.43732 14.01087M16.12463 7.97827A5.25 5.25 0 0 1 16.12463 16.02173" fill="none" stroke="#000" stroke-width="1.2"/>"##
            ),
            MediaGlyph::PillMuted => concat!(
                r#"<path d="M6 9.75h3l3.75-3v10.5L9 14.25H6z"/>"#,
                r##"<path d="M14.625 9.75L18.75 14.25M18.75 9.75L14.625 14.25" fill="none" stroke="#000" stroke-width="1.2"/>"##
            ),
            MediaGlyph::PillMore => {
                r#"<circle cx="12" cy="7.875" r="1.125"/><circle cx="12" cy="12" r="1.125"/><circle cx="12" cy="16.125" r="1.125"/>"#
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_has_one_name_and_its_shapes() {
        for glyph in Glyph::ALL {
            assert_eq!(Glyph::named(glyph.name()), Some(glyph));
            let shapes = glyph.shapes();
            assert!(
                shapes.starts_with('<') && shapes.ends_with("/>"),
                "{}",
                glyph.name()
            );
        }
        assert_eq!(Glyph::named("no-such-glyph"), None);
    }

    #[test]
    fn every_glyph_takes_its_weight_from_the_drawing() {
        // No shape sets its own stroke, and filled ones fill black.
        for glyph in Glyph::ALL {
            let shapes = glyph.shapes();
            assert!(!shapes.contains("stroke"), "{}", glyph.name());
            assert_eq!(
                shapes.matches("fill=").count(),
                shapes.matches(r##"fill="#000""##).count(),
                "{}",
                glyph.name()
            );
            let svg = glyph.svg();
            assert!(svg.contains(&format!(r#"stroke-width="{GLYPH_STROKE}""#)));
            assert!(svg.contains(&format!(r#"viewBox="0 0 {GLYPH_BOX} {GLYPH_BOX}""#)));
        }
    }

    #[test]
    fn every_media_glyph_has_one_name_and_its_shapes() {
        for glyph in MediaGlyph::ALL {
            assert_eq!(MediaGlyph::named(glyph.name()), Some(glyph));
            assert_eq!(
                Glyph::named(glyph.name()),
                None,
                "{} is a glyph's name too",
                glyph.name()
            );
            assert!(
                glyph.shapes().starts_with('<') && glyph.shapes().ends_with("/>"),
                "{}",
                glyph.name()
            );
        }
        assert_eq!(MediaGlyph::More.shapes().matches("<circle").count(), 3);
        assert_eq!(MediaGlyph::PillMore.shapes().matches("<circle").count(), 3);
        // The speaker's waves and cross are lines, not fills.
        for glyph in [MediaGlyph::PillSound, MediaGlyph::PillMuted] {
            assert!(
                glyph.shapes().contains(r##"fill="none" stroke="#000""##),
                "{}",
                glyph.name()
            );
        }
    }
}
