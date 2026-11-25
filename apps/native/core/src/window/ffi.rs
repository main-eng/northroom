//! The window's boundary (ADR-0001): which views it shows and where (`window`), the title bar
//! (`title_bar`), and row selection in the list and columns (`Columns::table_selection`).
//! Returned text is borrowed UTF-8 and rows are borrowed numbers, each valid until the next
//! call here that returns the same kind.

use crate::board::Frame;
use crate::columns::Table;
use crate::ffi::{
    file_kind, text_of, watch_open_columns, with_columns, NrKey, NrKeyPress, NrRect, NrText,
};
use crate::window::OpenedView;
use crate::Relock;
use std::ffi::c_char;
use std::sync::{LazyLock, Mutex};

fn lend(text: String) -> NrText {
    static LENT: Mutex<String> = Mutex::new(String::new());
    NrText::lend_into(&LENT, text)
}

/// Copies text lent by another boundary call before anything lends again.
fn owned(text: NrText) -> String {
    // SAFETY: the text was just lent and nothing has lent since.
    unsafe { text.to_owned_string() }
}

fn rect(frame: Option<Frame>) -> NrRect {
    let frame = frame.unwrap_or(Frame {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    });
    NrRect {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
    }
}

// MARK: What the window shows

/// How a folder is shown; the core's `#[repr(C)]` type.
pub use crate::columns::View as NrFolderView;

/// Which drive view the window shows.
#[repr(C)]
pub struct NrShellView {
    /// The active tab is the Settings tab, shown in the folder's place.
    pub settings: bool,
    /// The active tab is a Changes tab, shown in the folder's place (`changes::ffi`).
    pub changes: bool,
    pub folder: NrFolderView,
    /// How many columns are drawn: none for a board or list, or while a file opened from them
    /// fills the tab.
    pub columns: usize,
}

#[no_mangle]
pub extern "C" fn nr_shell_view() -> NrShellView {
    NrShellView {
        settings: crate::ffi::nr_settings_tab() >= 0,
        changes: crate::changes::ffi::nr_changes_shown(),
        folder: crate::ffi::folder_view(),
        columns: with_columns(|columns| columns.drawn_columns()).unwrap_or(0),
    }
}

/// A key for the open file as shown. When it changes, the shell rebuilds the view (another
/// file, another tab, or the same file moving from beside its column to filling the tab). Empty
/// when no file is open. It uses the file id, so two `index.html` files in different folders
/// differ.
#[no_mangle]
pub extern "C" fn nr_opened_identity() -> NrText {
    lend(
        with_columns(|columns| opened_identity(columns))
            .flatten()
            .unwrap_or_default(),
    )
}

fn opened_identity(columns: &crate::columns::Columns) -> Option<String> {
    let opened = columns.opened()?;
    let together: Vec<&str> = opened.together.iter().map(|(id, _)| id.as_str()).collect();
    Some(format!(
        "{}:{}:{}:{}:{}",
        file_kind(opened.kind) as u32,
        columns.active_tab(),
        opened.id,
        columns.opened_fills_tab(),
        together.join(",")
    ))
}

/// The view an open file is drawn in (`window::OpenedView`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrOpenedViewKind {
    /// No file is open.
    None,
    Terminal,
    /// A live HTML page.
    Html,
    Page,
    Table,
    /// A 3D model.
    Model,
    Source,
    /// The file's preview and details.
    Details,
    /// A presentation's slides.
    Deck,
}

fn view_kind(view: Option<OpenedView>) -> NrOpenedViewKind {
    match view {
        None => NrOpenedViewKind::None,
        Some(OpenedView::Terminal) => NrOpenedViewKind::Terminal,
        Some(OpenedView::Html) => NrOpenedViewKind::Html,
        Some(OpenedView::Page) => NrOpenedViewKind::Page,
        Some(OpenedView::Table) => NrOpenedViewKind::Table,
        Some(OpenedView::Deck) => NrOpenedViewKind::Deck,
        Some(OpenedView::Model) => NrOpenedViewKind::Model,
        Some(OpenedView::Source) => NrOpenedViewKind::Source,
        Some(OpenedView::Details) => NrOpenedViewKind::Details,
    }
}

fn view_of(kind: NrOpenedViewKind) -> Option<OpenedView> {
    Some(match kind {
        NrOpenedViewKind::None => return None,
        NrOpenedViewKind::Terminal => OpenedView::Terminal,
        NrOpenedViewKind::Html => OpenedView::Html,
        NrOpenedViewKind::Page => OpenedView::Page,
        NrOpenedViewKind::Table => OpenedView::Table,
        NrOpenedViewKind::Deck => OpenedView::Deck,
        NrOpenedViewKind::Model => OpenedView::Model,
        NrOpenedViewKind::Source => OpenedView::Source,
        NrOpenedViewKind::Details => OpenedView::Details,
    })
}

/// The view the open file is drawn in (`window::OpenedView::of`).
#[repr(C)]
pub struct NrOpenedView {
    pub view: NrOpenedViewKind,
    /// The view takes the keyboard when created.
    pub takes_keys: bool,
    /// The file the live page runs; empty for other views.
    pub path: NrText,
}

#[no_mangle]
pub extern "C" fn nr_opened_view() -> NrOpenedView {
    use crate::ffi::{
        nr_opened_model_path, opened_database_path, opened_html_path, opened_text_form,
        opened_workbook_path,
    };
    use crate::workspace::TextForm;
    let opened = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        Some((opened.kind, columns.opened_fills_tab()))
    })
    .flatten();
    let Some((kind, fills)) = opened else {
        return NrOpenedView {
            view: NrOpenedViewKind::None,
            takes_keys: false,
            path: lend(String::new()),
        };
    };
    let html = opened_html_path();
    let form = std::cell::OnceCell::new();
    let text_form = || *form.get_or_init(opened_text_form);
    let view = OpenedView::of(kind, |view| match view {
        OpenedView::Html => !html.is_empty(),
        OpenedView::Page => {
            kind == crate::workspace::EntryKind::Page || text_form() == Some(TextForm::PlainPage)
        }
        OpenedView::Table => {
            text_form() == Some(TextForm::Table)
                || opened_workbook_path().is_some()
                || opened_database_path().is_some()
        }
        OpenedView::Deck => {
            crate::deck::ffi::opened_deck().is_some() || crate::deck::ffi::opened_locked().is_some()
        }
        OpenedView::Model => !owned(nr_opened_model_path()).is_empty(),
        OpenedView::Source => text_form() == Some(TextForm::Source),
        OpenedView::Terminal | OpenedView::Details => false,
    });
    let runs = match view {
        OpenedView::Html => html,
        _ => String::new(),
    };
    NrOpenedView {
        view: view_kind(Some(view)),
        takes_keys: view.takes_keys(fills),
        path: lend(runs),
    }
}

/// Window frames (`window::layout`) in points from the top-left; the columns and a file beside
/// them are in the columns' content coordinates.
#[repr(C)]
pub struct NrWindowLayout {
    pub title_bar: NrRect,
    /// Under the title bar: the list, the board, settings and onboarding.
    pub content: NrRect,
    pub columns_scroll: NrRect,
    pub columns_content: NrRect,
    /// Column `n` sits `n × column_width` into the columns' content and is this wide.
    pub column_width: f64,
    pub opened: NrRect,
    /// The open file sits beside the columns, in their content coordinates.
    pub beside: bool,
    /// The list and the board are hidden.
    pub folder_hidden: bool,
    pub columns_hidden: bool,
    pub opened_hidden: bool,
    /// The details pane fills the tab.
    pub details_fills: bool,
    /// Whether to scroll `reveal_start..reveal_end` into view after opening a column or file;
    /// false with no columns.
    pub reveals: bool,
    pub reveal_start: f64,
    pub reveal_end: f64,
}

/// Lays out a `width` × `height` window with `columns` columns, the open file in view `opened`
/// (as `NrOpenedView::view`), settings when shown, and `changes` when the active tab is a
/// Changes tab.
#[no_mangle]
pub extern "C" fn nr_window_layout(
    width: f64,
    height: f64,
    columns: usize,
    opened: NrOpenedViewKind,
    settings: bool,
    changes: bool,
) -> NrWindowLayout {
    let laid = crate::window::layout(
        width,
        height,
        crate::window::Holds {
            columns,
            opened: view_of(opened),
            settings,
            changes,
        },
    );
    let (reveal_start, reveal_end) = laid.reveal.unwrap_or((0.0, 0.0));
    NrWindowLayout {
        title_bar: rect(Some(laid.title_bar)),
        content: rect(Some(laid.content)),
        columns_scroll: rect(Some(laid.columns_scroll)),
        columns_content: rect(Some(laid.columns_content)),
        column_width: laid.column_width,
        opened: rect(Some(laid.opened)),
        beside: laid.beside,
        folder_hidden: laid.folder_hidden,
        columns_hidden: laid.columns_hidden,
        opened_hidden: laid.opened_hidden,
        details_fills: laid.details_fills,
        reveals: laid.reveal.is_some(),
        reveal_start,
        reveal_end,
    }
}

// MARK: A table file's view

/// Layout of the open table's view (`ffi::opened_table`) in `width` × `height`, with `sheet`
/// open (`nr_table_sheet`) and the pointer over tab `hovered` (`sheets::layout::View`, or `None`
/// with no table open): the background, any message replacing the grid, the grid's scroll area
/// and full size, and the sheet bar with its tabs.
pub fn table_view(
    sheet: usize,
    width: f64,
    height: f64,
    hovered: Option<usize>,
) -> Option<crate::sheets::layout::View> {
    crate::ffi::with_opened_table(|table| {
        crate::sheets::layout::view(
            table,
            sheet,
            width,
            height,
            hovered,
            &mut crate::text::typeset::width,
        )
    })
}

/// The visible cells of `sheet` in the grid region from (`x`, `y`), `width` × `height`
/// (`sheets::layout::Cell`s in draw order): each with its frame relative to the region
/// (`sheets::layout::from_view`), whether it is a heading on the muted background, and its text.
/// `None` with no table open. A database's visible rows are read from its file here
/// (`database::rows`), which caches the rows around them.
pub fn table_grid(
    sheet: usize,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Vec<crate::sheets::layout::Cell> {
    use crate::sheets::layout::{from_view, grid, rows_in_view, windowed, Table};
    let visible = crate::geometry::Rect::new(x, y, width, height);
    let of_database = crate::ffi::with_opened_table(|table| match table {
        Table::Database(workbook) => workbook.sheets.get(sheet).cloned(),
        Table::Sheets(_) | Table::Unreadable(_) => None,
    })
    .flatten();
    if let Some(shown) = of_database {
        let rows = rows_in_view(visible, shown.row_count);
        let held = crate::ffi::opened_database_path().map_or_else(Vec::new, |path| {
            crate::sheets::database::rows(&path, sheet, rows.clone())
        });
        let cells = grid(
            &windowed(&shown, rows.start, held),
            visible,
            &mut crate::text::typeset::width,
        );
        return from_view(cells, visible);
    }
    crate::ffi::with_opened_table(|table| match table {
        Table::Sheets(workbook) => workbook.sheets.get(sheet).map_or_else(Vec::new, |shown| {
            from_view(
                grid(shown, visible, &mut crate::text::typeset::width),
                visible,
            )
        }),
        Table::Database(_) | Table::Unreadable(_) => Vec::new(),
    })
    .unwrap_or_default()
}

/// The sheet tab at (`x`, `y`) in the open table's sheet bar, with `sheet` open; -1 for none.
#[no_mangle]
pub extern "C" fn nr_table_tab_at(sheet: usize, x: f64, y: f64) -> i64 {
    crate::ffi::with_opened_table(|table| {
        crate::sheets::layout::tab_at(table, sheet, x, y, &mut crate::text::typeset::width)
    })
    .flatten()
    .map_or(-1, |tab| tab as i64)
}

// MARK: The title bar

static TITLE_BAR: LazyLock<Mutex<crate::window::title_bar::TitleBar>> =
    LazyLock::new(Mutex::default);

fn with_title_bar<T>(action: impl FnOnce(&mut crate::window::title_bar::TitleBar) -> T) -> T {
    action(&mut TITLE_BAR.relock())
}

/// Title bar control frames (`title_bar::Layout`); a hidden control is all zero.
#[repr(C)]
pub struct NrTitleBar {
    pub back: NrRect,
    pub forward: NrRect,
    /// The visible part of the tab row.
    pub tabs: NrRect,
    /// The tabs overflow, so they are clipped and scroll.
    pub clipped: bool,
    pub plus: NrRect,
    pub update: NrRect,
    /// The sessions button, its glyph, and where its text starts as a pill.
    pub sessions: NrRect,
    pub sessions_glyph: NrRect,
    pub sessions_words_x: f64,
    /// The review button, left of the sessions button.
    pub review: NrRect,
    /// How far the tabs are scrolled.
    pub offset: f64,
    /// How many tabs `nr_title_bar_tab` returns.
    pub tab_count: usize,
    /// The dragged tab, drawn over the others; -1 for none.
    pub carried: i64,
}

fn bar_of(layout: &crate::window::title_bar::Layout, carried: Option<usize>) -> NrTitleBar {
    NrTitleBar {
        back: rect(layout.back),
        forward: rect(layout.forward),
        tabs: rect(layout.tabs_view),
        clipped: layout.clipped,
        plus: rect(layout.plus),
        update: rect(layout.update),
        sessions: rect(layout.sessions),
        sessions_glyph: rect(layout.sessions_glyph),
        sessions_words_x: layout.sessions_words_x,
        review: rect(layout.review),
        offset: layout.offset,
        tab_count: layout.tabs.len(),
        carried: carried.map_or(-1, |tab| tab as i64),
    }
}

/// `count` widths at `widths`, or an empty slice when `count` is 0.
///
/// # Safety
/// `widths` must point to `count` values, or be null when `count` is 0.
unsafe fn widths_of<'a>(widths: *const f64, count: usize) -> &'a [f64] {
    if count == 0 {
        &[]
    } else {
        // SAFETY: the caller passes `count` values.
        unsafe { std::slice::from_raw_parts(widths, count) }
    }
}

/// Lays out the title bar for a window `width` wide. `titles` holds `count` tab name widths in
/// the tab font. `update` shows the update button. `sessions_words` is the width of the sessions
/// button's text (`sessions::ffi::sessions_button`) in the pill font, 0 for none, and
/// `sessions_grown` is how far the pill has grown, 0 to 1. `pointer` is the pointer position, or
/// null when off the bar. `window_buttons` says whether the window buttons sit at the bar's left
/// end (they are hidden in full screen).
///
/// # Safety
/// `titles` must point to `count` values, or be null when `count` is 0. `pointer` must point
/// to one point, or be null.
#[no_mangle]
pub unsafe extern "C" fn nr_title_bar_lay_out(
    width: f64,
    titles: *const f64,
    count: usize,
    update: bool,
    sessions_words: f64,
    sessions_grown: f64,
    pointer: *const crate::board::ffi::NrPoint,
    window_buttons: crate::window::title_bar::WindowButtons,
) -> NrTitleBar {
    // SAFETY: the caller passes `count` values.
    let titles = unsafe { widths_of(titles, count) };
    let (closes, active, active_id) = with_columns(|columns| {
        (
            (0..count)
                .map(|tab| columns.can_close_tab(tab))
                .collect::<Vec<bool>>(),
            columns.active_tab(),
            Some(columns.active_tab_id()),
        )
    })
    .unwrap_or_else(|| (vec![false; count], 0, None));
    // SAFETY: the caller passes one point, or null.
    let pointer = unsafe { pointer.as_ref() }.map(|at| (at.x, at.y));
    with_title_bar(|bar| {
        bar.lay_out(&crate::window::title_bar::Shown {
            width,
            titles,
            closes: &closes,
            pointer,
            active,
            active_id,
            update,
            sessions: crate::window::title_bar::SessionsShown {
                words: sessions_words,
                grown: sessions_grown,
            },
            window_buttons,
        });
        bar_of(bar.layout(), bar.carried())
    })
}

/// One tab from the last title bar layout.
#[repr(C)]
pub struct NrTitleBarTab {
    pub frame: NrRect,
    /// The glyph for the tab's content (`nr_tab_glyph`).
    pub glyph: NrRect,
    /// Where its name starts.
    pub label_x: f64,
    /// The name does not fit: it is opaque up to `fade_start` and fully faded at `fade_end`.
    pub fades: bool,
    pub fade_start: f64,
    pub fade_end: f64,
    /// Its cross, drawn over the end of the name on hover (`close_shown`).
    pub close: NrRect,
    pub close_shown: bool,
}

/// # Safety
/// `out` must point to writable storage for one `NrTitleBarTab`.
#[no_mangle]
pub unsafe extern "C" fn nr_title_bar_tab(index: usize, out: *mut NrTitleBarTab) -> bool {
    with_title_bar(|bar| {
        let Some(tab) = bar.layout().tabs.get(index) else {
            return false;
        };
        let fade = tab.fade.unwrap_or(crate::window::title_bar::Fade {
            start: 0.0,
            end: 0.0,
        });
        let placed = NrTitleBarTab {
            frame: rect(Some(tab.frame)),
            glyph: rect(Some(tab.glyph)),
            label_x: tab.label_x,
            fades: tab.fade.is_some(),
            fade_start: fade.start,
            fade_end: fade.end,
            close: rect(tab.close.map(|close| close.frame)),
            close_shown: tab.close.is_some_and(|close| close.shown),
        };
        // SAFETY: the caller passes storage for one tab.
        unsafe { out.write(placed) };
        true
    })
}

/// The result of a wheel event on the title bar.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrBarScroll {
    /// Not over the tabs or not sideways, so it passes on.
    PassOn,
    /// The tabs consume it without moving.
    Kept,
    /// The tabs moved and need a redraw.
    Moved,
}

/// Handles sideways wheel travel `across` (positive is right) in `unit` (0 points, 3 clicks) at
/// (`x`, `y`) on the bar.
#[no_mangle]
pub extern "C" fn nr_title_bar_scroll(x: f64, y: f64, across: f64, unit: u32) -> NrBarScroll {
    // Only a scroll over the tabs moves them.
    with_title_bar(|bar| match bar.over_tabs(x, y) && across != 0.0 {
        false => NrBarScroll::PassOn,
        true if bar.scroll(across, unit) => NrBarScroll::Moved,
        true => NrBarScroll::Kept,
    })
}

/// A control of the title bar (`title_bar::Control`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrBarControl {
    /// The bar itself, which moves the window.
    Bar,
    Back,
    Forward,
    /// The update button.
    Update,
    /// The sessions button.
    Sessions,
    /// The review button, which opens the Changes tab (`nr_changes_open_current`).
    Review,
    /// A tab's cross.
    Close,
    /// "+".
    Plus,
    Tab,
}

/// The control at a point on the title bar.
#[repr(C)]
pub struct NrTitleBarHit {
    pub control: NrBarControl,
    /// The tab, for `Close` and `Tab`; -1 otherwise.
    pub tab: i64,
}

/// The tab under (`x`, `y`) in the last title bar layout, cross included.
pub(crate) fn title_bar_tab_at(x: f64, y: f64) -> Option<usize> {
    with_title_bar(|bar| bar.tab_at(x, y))
}

/// Handles a middle-button release at (`x`, `y`): closes the tab under it unless it is the
/// last. Returns whether one closed.
#[no_mangle]
pub extern "C" fn nr_title_bar_middle_up(x: f64, y: f64) -> bool {
    match title_bar_tab_at(x, y) {
        Some(tab) if crate::ffi::nr_tab_closable(tab) => {
            crate::ffi::nr_tab_close(tab);
            true
        }
        _ => false,
    }
}

/// Holds tab `tab`, pressed at (`x`, `y`). It is dragged once the pointer moves
/// (`nr_title_bar_carry`).
#[no_mangle]
pub extern "C" fn nr_title_bar_hold(tab: usize, x: f64, y: f64) {
    with_title_bar(|bar| bar.hold(tab, x, y));
}

/// The result of dragging the held tab (`title_bar::Carry`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrBarCarry {
    /// No tab is dragged.
    Still,
    /// The tab follows the pointer; redraw the bar.
    Carried,
    /// The tab order changed; reload the tabs.
    Moved,
}

/// Moves the pressed pointer to (`x`, `y`) on a screen of `scale` pixels per point. The held tab
/// follows and the tabs reorder around it.
#[no_mangle]
pub extern "C" fn nr_title_bar_carry(x: f64, y: f64, scale: f64) -> NrBarCarry {
    match with_title_bar(|bar| bar.carry(x, y, scale)) {
        crate::window::title_bar::Carry::Still => NrBarCarry::Still,
        crate::window::title_bar::Carry::Carried => NrBarCarry::Carried,
        crate::window::title_bar::Carry::Moved(from, to) => {
            with_columns(|columns| columns.move_tab(from, to));
            NrBarCarry::Moved
        }
    }
}

/// Releases the held tab in place. Returns whether it was dragged.
#[no_mangle]
pub extern "C" fn nr_title_bar_let_go() -> bool {
    with_title_bar(crate::window::title_bar::TitleBar::let_go)
}

/// The top-left of window button `index` (0 close, 1 minimise, 2 zoom), from the window's
/// top-left (`screen_layout::window_button`).
#[no_mangle]
pub extern "C" fn nr_window_button_place(index: u32) -> crate::board::ffi::NrPoint {
    let (x, y) = crate::window::screen_layout::window_button(index);
    crate::board::ffi::NrPoint { x, y }
}

/// Positions of the list header and rows in a `width` × `height` list view
/// (`list::header::place`).
#[repr(C)]
pub struct NrListPlace {
    pub header: NrRect,
    pub rows: NrRect,
}

#[no_mangle]
pub extern "C" fn nr_list_place(width: f64, height: f64) -> NrListPlace {
    let (header, rows) = crate::list::header::place(width, height);
    NrListPlace {
        header: rect(Some(header)),
        rows: rect(Some(rows)),
    }
}

#[no_mangle]
pub extern "C" fn nr_title_bar_hit(x: f64, y: f64) -> NrTitleBarHit {
    use crate::window::title_bar::Control;
    let (control, tab) = match with_title_bar(|bar| bar.hit(x, y)) {
        Control::Bar => (NrBarControl::Bar, -1),
        Control::Back => (NrBarControl::Back, -1),
        Control::Forward => (NrBarControl::Forward, -1),
        Control::Update => (NrBarControl::Update, -1),
        Control::Sessions => (NrBarControl::Sessions, -1),
        Control::Review => (NrBarControl::Review, -1),
        Control::Close(tab) => (NrBarControl::Close, tab as i64),
        Control::Plus => (NrBarControl::Plus, -1),
        Control::Tab(tab) => (NrBarControl::Tab, tab as i64),
    };
    NrTitleBarHit { control, tab }
}

/// The tooltip for the control at (`x`, `y`), with its `frame`; empty for none. `update` is the
/// update button's current label.
///
/// # Safety
/// `update` must point to a NUL-terminated string; `frame` to writable storage for one rect.
#[no_mangle]
pub unsafe extern "C" fn nr_title_bar_tooltip(
    x: f64,
    y: f64,
    update: *const c_char,
    frame: *mut NrRect,
) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let update = unsafe { text_of(update) };
    let (back, forward) = (
        crate::ffi::nr_history_can_go_back(),
        crate::ffi::nr_history_can_go_forward(),
    );
    let sessions = crate::sessions::ffi::sessions_summary();
    let review = crate::changes::ffi::review_title();
    let tips = crate::window::title_bar::Tips {
        can_go_back: back,
        can_go_forward: forward,
        update: &update,
        sessions: &sessions,
        review: &review,
    };
    let said = with_title_bar(|bar| bar.tooltip(x, y, &tips));
    let (place, words) = said.map_or((None, String::new()), |(place, words)| (Some(place), words));
    // SAFETY: the caller passes storage for one rect.
    unsafe { frame.write(rect(place)) };
    lend(words)
}

// MARK: The rows of the list and of the columns

/// A table's rows, top first, borrowed until the next call here that returns rows.
#[repr(C)]
pub struct NrRows {
    pub rows: *const usize,
    pub count: usize,
    /// For a selection, the focus row keys act from; -1 for none.
    pub focus: i64,
}

fn lend_rows(rows: Vec<usize>, focus: Option<usize>) -> NrRows {
    static LENT: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let mut lent = LENT.relock();
    *lent = rows;
    NrRows {
        rows: lent.as_ptr(),
        count: lent.len(),
        focus: focus.map_or(-1, |row| row as i64),
    }
}

/// Maps a shell table number to a table: -1 for the list, otherwise a column number.
pub(crate) fn table(table: i64) -> Table {
    usize::try_from(table).map_or(Table::List, Table::Column)
}

/// Runs a selection change; in a column it may open or close the columns after it.
fn changing<T: Default>(
    table: Table,
    change: impl FnOnce(&mut crate::columns::Columns) -> crate::workspace::DriveResult<T>,
) -> T {
    let changed = with_columns(|columns| {
        change(columns).unwrap_or_else(|error| {
            eprintln!("the selection did not open: {error}");
            T::default()
        })
    });
    if matches!(table, Table::Column(_)) {
        watch_open_columns();
    }
    changed.unwrap_or_default()
}

/// The table's selected rows and focus row.
#[no_mangle]
pub extern "C" fn nr_rows_selected(table_number: i64) -> NrRows {
    let selected =
        with_columns(|columns| columns.table_selection(table(table_number))).unwrap_or_default();
    lend_rows(selected.rows, selected.focus)
}

/// Tab width in spaces for source files.
#[no_mangle]
pub extern "C" fn nr_source_tab_spaces() -> u32 {
    crate::window::SOURCE_TAB_SPACES
}

/// Where a file drag lands.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrDropPlace {
    Nowhere,
    /// Into the folder at the row.
    Folder,
    /// To the running terminal at the row.
    Terminal,
    /// Into the shown folder, or the column's own.
    Shown,
}

/// Where a drag of the list's own rows `ids` (one per line) lands over `row` of the last loaded
/// list (-1 for empty space or between rows), per `list::moves::drop_place`. An "Empty folder"
/// placeholder row counts as empty space.
///
/// # Safety
/// `ids` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_list_drop_place(ids: *const c_char, row: i64) -> NrDropPlace {
    use crate::list::moves::{drop_place, Drop, Over};
    // SAFETY: the caller passes a NUL-terminated string.
    let ids = crate::ffi::ids_of(&unsafe { text_of(ids) });
    let takes = crate::ffi::takes_references_now();
    let over = usize::try_from(row).ok().and_then(|row| {
        let rows = crate::app().list_rows.relock();
        let shown = rows.get(row).filter(|shown| shown.placeholder.is_none())?;
        Some((
            row,
            shown.entry.id.clone(),
            shown.entry.kind == crate::workspace::EntryKind::Folder,
            takes(&shown.entry),
        ))
    });
    let over = over
        .as_ref()
        .map(|(row, id, folder, takes_references)| Over {
            row: *row,
            id,
            folder: *folder,
            takes_references: *takes_references,
        });
    let place = with_columns(|columns| {
        // Both branches read the same columns, but only one runs.
        let columns = std::cell::RefCell::new(columns);
        drop_place(
            &ids,
            over,
            |target| columns.borrow_mut().list_drop_takes(&ids, Some(target)),
            || columns.borrow_mut().list_drop_takes(&ids, None),
        )
    });
    match place {
        Some(Drop::Folder(_)) => NrDropPlace::Folder,
        Some(Drop::Terminal(_)) => NrDropPlace::Terminal,
        Some(Drop::Shown) => NrDropPlace::Shown,
        Some(Drop::Nowhere) | None => NrDropPlace::Nowhere,
    }
}

/// Handles a click released on `row` (-1 for the space below the rows) with `held` modifiers
/// (`menus::COMMAND`…); ⇧, ⌘ and Control choose how it selects (`row_pointer`). The shell then
/// shows the selection; in a column it may have opened a folder or file.
#[no_mangle]
pub extern "C" fn nr_rows_press(table_number: i64, row: i64, held: u32) {
    use crate::menus::{COMMAND, CONTROL, SHIFT};
    let table = table(table_number);
    let mode =
        crate::list::row_pointer::mode(held & SHIFT != 0, held & COMMAND != 0, held & CONTROL != 0);
    changing(table, |columns| {
        columns.press_row(table, usize::try_from(row).ok(), mode)
    });
}

/// Selects `count` rows directly (`rows`), as a check does.
///
/// # Safety
/// `rows` must point to `count` values, or be null when `count` is 0.
#[no_mangle]
pub unsafe extern "C" fn nr_rows_set(table_number: i64, rows: *const usize, count: usize) {
    let rows: Vec<usize> = if count == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller passes `count` values.
        unsafe { std::slice::from_raw_parts(rows, count) }.to_vec()
    };
    let table = table(table_number);
    changing(table, |columns| columns.set_rows(table, &rows));
}

/// The items a drag starting on `row` carries (`Columns::pick_up_rows`); none when nothing may
/// be dragged. The shell then shows the selection, which pick-up may have changed.
#[no_mangle]
pub extern "C" fn nr_rows_pick_up(table_number: i64, row: usize) -> NrRows {
    let table = table(table_number);
    lend_rows(
        with_columns(|columns| columns.pick_up_rows(table, row)).unwrap_or_default(),
        None,
    )
}

/// Handles a right click on `row` (-1 for empty space): updates the selection and returns the
/// rows the menu applies to, none for empty space (`Columns::context_rows`).
#[no_mangle]
pub extern "C" fn nr_rows_context(table_number: i64, row: i64) -> NrRows {
    let table = table(table_number);
    let rows = changing(table, |columns| {
        columns.context_rows(table, usize::try_from(row).ok())
    });
    lend_rows(rows, None)
}

/// The column list commands act in: the shown folder, or a folder expanded in place that a menu
/// or key targeted (`Columns::list_command_column`).
#[no_mangle]
pub extern "C" fn nr_list_command_column() -> usize {
    with_columns(|columns| columns.list_command_column()).unwrap_or(0)
}

/// What a key in a table asks the shell to do (`NrRowKey::intent`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrRowIntent {
    /// The rows ignore the key, so it passes on.
    PassOn,
    /// Show the core's selection, scrolling `row` into view unless it is -1.
    Selection,
    /// A folder expanded or collapsed in place; reload the list.
    Reload,
    /// Open `row`.
    Open,
    /// Show the parent of the shown folder.
    OpenParent,
    /// Rename `row`.
    Rename,
    /// ⌘⌫ or Delete: the shell trashes the selected files immediately, without confirmation,
    /// via `menus::ffi::nr_drive_edit(NrDriveEdit::Trash, table)`, and shows the result.
    Trash,
    /// Column `row` takes the keyboard.
    Column,
}

/// The result of a key in a table (`row_keys`). A `row` of -1 with `Open`, `Rename` or `Column`
/// does nothing.
#[repr(C)]
pub struct NrRowKey {
    pub intent: NrRowIntent,
    pub row: i64,
}

fn key_of(press: NrKeyPress) -> crate::list::row_keys::Key {
    use crate::list::row_keys::Key;
    match press.key {
        NrKey::Right => Key::Right,
        NrKey::Left => Key::Left,
        NrKey::Down => Key::Down,
        NrKey::Up => Key::Up,
        NrKey::Return => Key::Return,
        NrKey::F2 => Key::F2,
        NrKey::Space => Key::Space,
        NrKey::Delete => Key::Delete,
        NrKey::Backspace => Key::Backspace,
        NrKey::Escape => Key::Escape,
        NrKey::Home => Key::Home,
        NrKey::End => Key::End,
        _ => press.typed().map_or(Key::Other, Key::Character),
    }
}

/// Handles a key in a table: the list (-1) or a column. Selection changes happen here; what the
/// shell must do is returned.
#[no_mangle]
pub extern "C" fn nr_rows_key(table_number: i64, press: NrKeyPress) -> NrRowKey {
    use crate::list::row_keys::{column_intent, list_intent, Intent};
    let table = table(table_number);
    let held = crate::page::keys::held(press.held);
    let key = key_of(press);
    let intent = match table {
        Table::List => list_intent(key, held),
        Table::Column(_) => column_intent(key, held),
    };
    let answer = NrRowKey::answer;
    let focus = || with_columns(|columns| columns.table_selection(table).focus).flatten();
    match intent {
        None => answer(NrRowIntent::PassOn, None),
        Some(Intent::Move(direction, reach)) => {
            let to = changing(table, |columns| {
                columns.step_rows(
                    table,
                    direction,
                    reach == crate::list::row_keys::Reach::Range,
                )
            });
            answer(NrRowIntent::Selection, to)
        }
        Some(Intent::Clear) => {
            changing(table, |columns| columns.clear_rows(table));
            answer(NrRowIntent::Selection, None)
        }
        Some(Intent::SelectAll) => {
            changing(table, |columns| columns.select_all_rows(table));
            answer(NrRowIntent::Selection, None)
        }
        Some(Intent::Expand | Intent::Collapse) if table != Table::List => {
            answer(NrRowIntent::Selection, None)
        }
        Some(step @ (Intent::Expand | Intent::Collapse)) => match focus() {
            Some(row) => step_list_row(table, row, step),
            None => answer(NrRowIntent::Selection, None),
        },
        Some(Intent::Open) => answer(NrRowIntent::Open, focus()),
        Some(Intent::OpenParent) if table == Table::List => answer(NrRowIntent::OpenParent, None),
        Some(Intent::OpenParent) => answer(NrRowIntent::Selection, None),
        Some(Intent::Rename) => answer(
            NrRowIntent::Rename,
            with_columns(|columns| columns.rename_row(table)).flatten(),
        ),
        Some(Intent::Delete) => answer(NrRowIntent::Trash, None),
        Some(Intent::TypeAhead(letter)) => answer(
            NrRowIntent::Selection,
            changing(table, |columns| {
                columns.type_ahead(table, letter, std::time::Instant::now())
            }),
        ),
        Some(step @ (Intent::PreviousColumn | Intent::NextColumn)) => match table {
            Table::Column(column) => answer(
                NrRowIntent::Column,
                changing(table, |columns| {
                    columns.step_column(column, step == Intent::PreviousColumn)
                }),
            ),
            Table::List => answer(NrRowIntent::Selection, None),
        },
    }
}

impl NrRowKey {
    fn answer(intent: NrRowIntent, row: Option<usize>) -> Self {
        Self {
            intent,
            row: row.map_or(-1, |row| row as i64),
        }
    }
}

/// → or ← (`step`) on the list's focused `row`: expands or collapses a folder, or moves focus
/// to its child or parent.
fn step_list_row(table: Table, row: usize, step: crate::list::row_keys::Intent) -> NrRowKey {
    use crate::columns::ListStep;
    let decided = {
        let rows = crate::app().list_rows.relock();
        if step == crate::list::row_keys::Intent::Expand {
            crate::columns::list_expand_step(&rows, row)
        } else {
            crate::columns::list_collapse_step(&rows, row)
        }
    };
    match decided {
        ListStep::Toggle(id) => {
            with_columns(|columns| columns.toggle_expanded(&id));
            NrRowKey::answer(NrRowIntent::Reload, None)
        }
        ListStep::Focus(row) => {
            changing(table, |columns| columns.focus_row(table, row));
            NrRowKey::answer(NrRowIntent::Selection, Some(row))
        }
        ListStep::Stay => NrRowKey::answer(NrRowIntent::Selection, None),
    }
}

// MARK: A scroller the app draws

/// A scroller the app draws itself, as the GPUI shell's list and columns do
/// (`scroller::Scroller`): a clamped offset and an overlay scrollbar. Times are seconds on the
/// platform's monotonic clock; frames are in window points. Free it with `nr_scroller_free`.
pub struct NrScroller(crate::scroll::scroller::Scroller);

/// One wheel or trackpad event over a scroller (`scroller::Travel`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrScrollerTravel {
    /// A wheel that moves in clicks.
    Clicks,
    /// Trackpad points: finger travel, or system momentum after lift-off.
    Points,
}

/// How to draw a scroller's overlay scrollbar now (`scroller::Bar`): the fully rounded knob and
/// the track behind it, each with an opacity (0 hides it).
#[repr(C)]
pub struct NrScrollerBar {
    pub knob: NrRect,
    pub knob_opacity: f64,
    pub track: NrRect,
    pub track_opacity: f64,
}

fn frame_of(rect: NrRect) -> Frame {
    Frame {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// A new scroller at offset 0 with no extent.
#[no_mangle]
pub extern "C" fn nr_scroller_new() -> *mut NrScroller {
    Box::into_raw(Box::new(NrScroller(
        crate::scroll::scroller::Scroller::default(),
    )))
}

/// # Safety
/// `scroller` must be null or a scroller from `nr_scroller_new`; it is not used again.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_free(scroller: *mut NrScroller) {
    if !scroller.is_null() {
        // SAFETY: the caller hands over a scroller from `nr_scroller_new`.
        drop(unsafe { Box::from_raw(scroller) });
    }
}

/// Sets the extent to `shown` points of `length`, clamping the offset.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_set_extent(
    scroller: *mut NrScroller,
    shown: f64,
    length: f64,
) {
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }.0.set_extent(shown, length);
}

/// How far the scroller is scrolled.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_offset(scroller: *const NrScroller) -> f64 {
    // SAFETY: the caller passes a live scroller.
    unsafe { &*scroller }.0.offset()
}

/// Jumps to `offset`, clamped to the ends, as a programmatic scroll.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_scroll_to(scroller: *mut NrScroller, offset: f64) {
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }.0.scroll_to(offset);
}

/// Scrolls just far enough to show `start..end` (`scroll::route::reveal`).
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_reveal(scroller: *mut NrScroller, start: f64, end: f64) {
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }.0.reveal(start, end);
}

/// Applies a wheel or trackpad event at `now`, moving `down` clicks or points. True when it
/// moved.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_wheel(
    scroller: *mut NrScroller,
    travel: NrScrollerTravel,
    down: f64,
    now: f64,
) -> bool {
    use crate::scroll::scroller::Travel;
    let travel = match travel {
        NrScrollerTravel::Clicks => Travel::Clicks,
        NrScrollerTravel::Points => Travel::Points,
    };
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }.0.wheel(travel, down, now)
}

/// True while the scrollbar is still visible at `now`, so the shell redraws next frame.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_tick(scroller: *const NrScroller, now: f64) -> bool {
    // SAFETY: the caller passes a live scroller.
    unsafe { &*scroller }.0.tick(now)
}

/// The overlay scrollbar over the scroller's frame `view` at `now`.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_bar(
    scroller: *const NrScroller,
    view: NrRect,
    now: f64,
) -> NrScrollerBar {
    // SAFETY: the caller passes a live scroller.
    let bar = unsafe { &*scroller }.0.bar(frame_of(view), now);
    NrScrollerBar {
        knob: rect(Some(bar.knob)),
        knob_opacity: bar.knob_opacity,
        track: rect(Some(bar.track)),
        track_opacity: bar.track_opacity,
    }
}

/// Handles the pointer moving to (`x`, `y`) over frame `view` at `now`: a held knob follows it,
/// and the scrollbar widens over the track. True when the scrollbar changed.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_pointer(
    scroller: *mut NrScroller,
    x: f64,
    y: f64,
    view: NrRect,
    now: f64,
) -> bool {
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }
        .0
        .pointer(x, y, frame_of(view), now)
}

/// Handles a press at `y` over frame `view` at `now`: grabs the knob, or jumps it there on the
/// track. True when the scrollbar took the press.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_press(
    scroller: *mut NrScroller,
    y: f64,
    view: NrRect,
    now: f64,
) -> bool {
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }.0.press(y, frame_of(view), now)
}

/// Handles button release at `now`, freeing a held knob. True when it was held.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_release(scroller: *mut NrScroller, now: f64) -> bool {
    // SAFETY: the caller passes a live scroller.
    unsafe { &mut *scroller }.0.release(now)
}

/// Whether the scroller's knob is held.
///
/// # Safety
/// `scroller` must be a scroller from `nr_scroller_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_scroller_held(scroller: *const NrScroller) -> bool {
    // SAFETY: the caller passes a live scroller.
    unsafe { &*scroller }.0.is_held()
}

#[cfg(test)]
mod tests {
    use super::opened_identity;
    use crate::columns::Columns;
    use crate::workspace::Workspace;
    use std::fs;

    #[test]
    fn two_files_of_one_name_are_shown_as_two_files() {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        for folder in ["First", "Second"] {
            fs::create_dir_all(root.path().join(folder)).expect("folder");
            fs::write(root.path().join(folder).join("index.html"), b"<p>x</p>").expect("page");
        }
        let workspace = Workspace::open(root.path(), metadata.path()).expect("open");
        let mut columns = Columns::open(workspace).expect("columns");
        columns.select(0, 0).expect("First");
        columns.select(1, 0).expect("its index.html");
        let first = opened_identity(&columns).expect("a file is open");
        let first_title = columns.opened().expect("open").title.clone();
        columns.select(0, 1).expect("Second");
        columns.select(1, 0).expect("its index.html");
        let second = opened_identity(&columns).expect("a file is open");
        assert_eq!(columns.opened().expect("open").title, first_title);
        assert_ne!(first, second);
    }
}
