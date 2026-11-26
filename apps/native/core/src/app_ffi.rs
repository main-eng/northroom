//! FFI for the app chrome (ADR-0001): app folders, launch mode, networking, settings, motion,
//! the colour picker, onboarding and live HTML pages. Returned text is borrowed
//! UTF-8, valid until the next call that returns text.

use crate::ffi::{text_of, NrColor, NrText};
use crate::Relock;
use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

fn lend(text: String) -> NrText {
    static LENT: Mutex<String> = Mutex::new(String::new());
    NrText::lend_into(&LENT, text)
}

// MARK: Folders and launch

/// Sets the platform folders for app data (on a Mac `~/Library/Application Support`,
/// `~/Library/Logs`, `~/Library/Caches`). Call this before anything that touches app files.
///
/// # Safety
/// Each argument is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_paths_open(
    application_support: *const c_char,
    logs: *const c_char,
    caches: *const c_char,
) {
    // SAFETY: the caller passes NUL-terminated strings.
    let (application_support, logs, caches) =
        unsafe { (text_of(application_support), text_of(logs), text_of(caches)) };
    crate::paths::open(&crate::paths::Bases {
        application_support: PathBuf::from(application_support),
        logs: PathBuf::from(logs),
        caches: PathBuf::from(caches),
    });
    // Off the launch's thread: the folder held up to 2 GB.
    let support = crate::paths::get().support;
    std::thread::spawn(move || {
        if let Err(error) = crate::paths::remove_change_review_data(&support) {
            eprintln!("what change review left was not removed: {error}");
        }
    });
}

/// An app folder (`paths::Paths`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrPath {
    Support,
    Logs,
    /// Holds downloads.
    Caches,
}

#[no_mangle]
pub extern "C" fn nr_path(which: NrPath) -> NrText {
    let paths = crate::paths::get();
    let path = match which {
        NrPath::Support => paths.support.clone(),
        NrPath::Logs => paths.logs.clone(),
        NrPath::Caches => paths.caches.clone(),
    };
    lend(path.to_string_lossy().into_owned())
}

/// Launch mode (`launch::Launch`).
#[repr(C)]
pub struct NrLaunch {
    /// An automated check: takes no focus, stays out of the Dock and app switcher, and uses a
    /// private clipboard.
    pub unattended: bool,
    /// `NR_TRACE_SCROLL`: log every scroll event's target to stderr.
    pub trace_scroll: bool,
}

#[no_mangle]
pub extern "C" fn nr_launch() -> NrLaunch {
    let launch = crate::launch::get();
    NrLaunch {
        unattended: launch.unattended,
        trace_scroll: launch.trace_scroll,
    }
}

/// Initial and minimum window size.
#[repr(C)]
pub struct NrWindowSize {
    pub width: f64,
    pub height: f64,
    pub min_width: f64,
    pub min_height: f64,
}

#[no_mangle]
pub extern "C" fn nr_window_size() -> NrWindowSize {
    let ((width, height), (min_width, min_height)) =
        (crate::window::WINDOW_SIZE, crate::window::WINDOW_MIN_SIZE);
    NrWindowSize {
        width,
        height,
        min_width,
        min_height,
    }
}

// MARK: The network

/// Registers a callback the core may call from any thread when it has work the shell did not
/// ask for: a request to send or an unsolicited effect. On its main thread, the shell then sends
/// what `transport_next` returns and drains every owner's effects.
#[no_mangle]
pub extern "C" fn nr_transport_open(wake: extern "C" fn()) {
    crate::transport::open(wake);
}

/// The next request for the shell's executor and its id, or `None`. A request carries a delay,
/// an absolute URL and headers, sent exactly as given. The executor keeps no cookies, cache or
/// credentials and streams the reply back (`transport_response`, `nr_transport_data`,
/// `nr_transport_done`).
pub fn transport_next() -> Option<(u64, crate::transport::Request)> {
    crate::transport::next()
}

/// Called on the executor's thread when request `id` gets its HTTP status. Returns whether to
/// read the body; if not, the executor cancels it and reports `nr_transport_done`.
#[no_mangle]
pub extern "C" fn nr_transport_response(id: u64, status: u32) -> bool {
    crate::transport::respond(id, status)
}

/// Called on the executor's thread with the next `length` bytes of request `id`'s body. Returns
/// whether to continue; if not, the executor cancels it.
///
/// # Safety
/// `bytes` points to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn nr_transport_data(id: u64, bytes: *const u8, length: usize) -> bool {
    if length == 0 {
        return crate::transport::receive(id, &[]);
    }
    // SAFETY: as the caller promises.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    crate::transport::receive(id, bytes)
}

/// Called on the executor's thread when request `id` ends. `failure` is the system's error
/// message, or empty on success. The updater is notified before this returns.
///
/// # Safety
/// `failure` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_transport_done(id: u64, failure: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let failure = unsafe { text_of(failure) };
    crate::transport::finished(id, &failure);
}

// MARK: Settings

/// The settings page (`appearance::page`). The shell builds a control for each of
/// `Page::controls` and lays the page out with `settings_window`.
pub fn settings_page() -> crate::settings::page::Page {
    let language = crate::ffi::interface_language();
    crate::ffi::with_appearance(|settings| settings.page(language))
}

/// A native control's size as the shell measured it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NrSize {
    pub width: f64,
    pub height: f64,
}

impl From<NrSize> for crate::dialog::layout::Size {
    fn from(size: NrSize) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }
}

/// Lays out the settings window: sidebar, page column and `page`. `sizes` follows
/// `Page::controls` order: each section's heading, row and folder-button controls, then the
/// footer's.
pub fn settings_window(
    page: &crate::settings::page::Page,
    width: f64,
    height: f64,
    sizes: &[crate::dialog::layout::Size],
) -> crate::settings::page::layout::Window {
    crate::settings::page::layout::window(
        page,
        width,
        height,
        sizes,
        &mut crate::text::typeset::width,
    )
}

/// What the shell must do after a settings control is used.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrSettingsActed {
    Nothing,
    /// The theme changed; redraw every view.
    ThemeChanged,
}

/// Handles use of a settings control: `action` names it and `value` is its new value
/// (`settings::page::Control`). Appearance controls save the choice and apply the theme.
///
/// # Safety
/// `action` and `value` are NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_settings_act(
    action: *const c_char,
    value: *const c_char,
) -> NrSettingsActed {
    // SAFETY: the caller passes NUL-terminated strings.
    let (action, value) = unsafe { (text_of(action), text_of(value)) };
    if action.starts_with("appearance.")
        && crate::ffi::change_appearance(|settings| settings.act(&action, &value))
    {
        return NrSettingsActed::ThemeChanged;
    }
    NrSettingsActed::Nothing
}

/// Called when the system switches between light and dark. Users who follow the system get its
/// theme; a check's `NR_THEME` ignores it. Returns whether the active theme changed, in which
/// case every view redraws.
#[no_mangle]
pub extern "C" fn nr_appearance_system(system_is_dark: bool) -> bool {
    if !crate::launch::get().follows_system_look() {
        return false;
    }
    let before = crate::ffi::nr_appearance_is_dark();
    crate::app().appearance.relock().1 = system_is_dark;
    crate::ffi::apply_appearance();
    crate::ffi::nr_appearance_is_dark() != before
}

// MARK: Motion

/// Whether a check freezes entrance (`entering`) or exit animations, drawing one still frame
/// with no clock running.
#[no_mangle]
pub extern "C" fn nr_motion_held(entering: bool) -> bool {
    crate::launch::get().held_ms(entering).is_some()
}

/// The current pose of a popup or dialog animation, and whether it is done (`motion::frame`).
#[repr(C)]
pub struct NrMotionFrame {
    pub opacity: f64,
    pub scale: f64,
    pub slide: f64,
    pub done: bool,
}

/// Pose at `now` of a `duration` ms entrance or exit (`leaving`) that began at `since`.
#[no_mangle]
pub extern "C" fn nr_motion_frame(
    since: f64,
    now: f64,
    duration: f64,
    leaving: bool,
) -> NrMotionFrame {
    let held = crate::launch::get().held_ms(!leaving);
    let frame = crate::motion::frame(
        crate::motion::elapsed(since, now, held),
        duration,
        leaving,
        held,
    );
    NrMotionFrame {
        opacity: frame.pose.opacity,
        scale: frame.pose.scale,
        slide: frame.pose.slide,
        done: frame.done,
    }
}

static HOVERS: LazyLock<Mutex<crate::motion::HoverFades>> =
    LazyLock::new(|| Mutex::new(crate::motion::HoverFades::default()));

/// Hover highlight level of window button `key` at `now` (platform clock, seconds), with the
/// pointer over it (`on`) or not (`motion::HoverFades`). `began` means a fade started: redraw
/// every `nr_frame_seconds(Move)` until `nr_hover_settled`.
#[repr(C)]
pub struct NrHoverLevel {
    pub level: f64,
    pub began: bool,
}

/// # Safety
/// `key` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_hover_level(key: *const c_char, on: bool, now: f64) -> NrHoverLevel {
    // SAFETY: the caller passes a NUL-terminated string.
    let key = unsafe { text_of(key) };
    let held = crate::launch::get().held_ms(true);
    let level = HOVERS.relock().level(&key, on, now, held);
    NrHoverLevel {
        level: level.level,
        began: level.began && held.is_none(),
    }
}

/// Whether every hover fade has finished at `now`.
#[no_mangle]
pub extern "C" fn nr_hover_settled(now: f64) -> bool {
    let held = crate::launch::get().held_ms(true);
    HOVERS.relock().settled(now, held)
}

/// Kinds of animation, for `nr_frame_seconds`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrFrame {
    /// A popup or dialog transition, or a hover fade.
    Move,
    /// Other animation, such as a spinner or the board.
    Moving,
    /// Player controls tracking playback.
    Follow,
}

/// Redraw interval for an animation kind, in seconds (`motion`).
#[no_mangle]
pub extern "C" fn nr_frame_seconds(which: NrFrame) -> f64 {
    match which {
        NrFrame::Move => crate::motion::MOVE_FRAME_SECONDS,
        NrFrame::Moving => crate::motion::FRAME_SECONDS,
        NrFrame::Follow => crate::motion::FOLLOW_SECONDS,
    }
}

/// Spinner rotation as a fraction of a turn at `seconds` (platform clock). `reduced` is the
/// system's reduce-motion setting (`motion::spin`).
#[no_mangle]
pub extern "C" fn nr_motion_spin(seconds: f64, reduced: bool) -> f64 {
    crate::motion::spin(seconds, reduced, crate::launch::get().held_ms(true))
}

/// Whether the spinner animates and so needs a redraw every frame (`motion::spins`). `reduced`
/// as for `nr_motion_spin`.
#[no_mangle]
pub extern "C" fn nr_motion_spins(reduced: bool) -> bool {
    crate::motion::spins(reduced, crate::launch::get().held_ms(true))
}

// MARK: The colour picker

static PICKER: Mutex<Option<crate::colour_picker::PickerView>> = Mutex::new(None);

fn with_picker<T>(
    default: T,
    action: impl FnOnce(&mut crate::colour_picker::PickerView) -> T,
) -> T {
    PICKER.relock().as_mut().map_or(default, action)
}

/// Converts 8-bit channels to an opaque colour with 0 to 1 channels.
fn color_of(channels: [u8; 3]) -> NrColor {
    let [red, green, blue] = channels.map(|channel| f32::from(channel) / 255.0);
    NrColor {
        red,
        green,
        blue,
        alpha: 1.0,
    }
}

/// Parses `hex` (`#RRGGBB` or six digits). Missing channels are 0.
///
/// # Safety
/// `hex` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_hex_color(hex: *const c_char) -> NrColor {
    // SAFETY: the caller passes a NUL-terminated string.
    color_of(crate::settings::appearance::picker::rgb(&unsafe {
        text_of(hex)
    }))
}

/// Formats a swatch label: `#` plus six upper-case hex digits (`colour_picker::swatch_hex`).
///
/// # Safety
/// `hex` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_swatch_hex(hex: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    lend(crate::colour_picker::swatch_hex(&unsafe { text_of(hex) }))
}

/// Opens the colour picker (`colour_picker::PickerView`) on `hex`, in hex mode.
///
/// # Safety
/// `hex` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_picker_open(hex: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let picker = crate::colour_picker::PickerView::open(&unsafe { text_of(hex) });
    *PICKER.relock() = Some(picker);
}

/// The open picker's colour.
#[repr(C)]
pub struct NrPicker {
    /// Six upper-case hex digits, no `#`.
    pub hex: NrText,
    pub color: NrColor,
    /// The hue at full saturation and brightness, for the square's gradient and the hue knob.
    pub pure: NrColor,
    /// Channels 0 to 255, for the RGB fields.
    pub red: u32,
    pub green: u32,
    pub blue: u32,
}

#[no_mangle]
pub extern "C" fn nr_picker_view() -> NrPicker {
    use crate::settings::appearance::picker::{hex, rgb};
    let (shown, hue) = with_picker((String::new(), 0.0), |view| {
        (view.picker.hex().to_owned(), view.picker.hue())
    });
    let channels = rgb(&shown);
    NrPicker {
        color: color_of(channels),
        pure: color_of(rgb(&hex(hue, 1.0, 1.0))),
        red: u32::from(channels[0]),
        green: u32::from(channels[1]),
        blue: u32::from(channels[2]),
        hex: lend(shown),
    }
}

/// The open picker's layout (`colour_picker::Layout`), or `None` when no picker is open.
pub fn picker_layout() -> Option<crate::colour_picker::Layout> {
    with_picker(None, |view| {
        Some(view.layout(&mut crate::text::typeset::width))
    })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrPickerPress {
    Nothing,
    /// The colour changed; a drag on the square or hue bar may follow.
    Changed,
    /// Run the native eyedropper and report via `nr_picker_act` with `Eyedropper`.
    Eyedropper,
    /// The fields switched between hex and RGB.
    ModeTurned,
}

#[no_mangle]
pub extern "C" fn nr_picker_press(x: f64, y: f64) -> NrPickerPress {
    use crate::colour_picker::Press;
    match with_picker(Press::Nothing, |view| view.press(x, y)) {
        Press::Nothing => NrPickerPress::Nothing,
        Press::Changed => NrPickerPress::Changed,
        Press::Eyedropper => NrPickerPress::Eyedropper,
        Press::ModeTurned => NrPickerPress::ModeTurned,
    }
}

/// Continues a drag. Returns whether the colour changed.
#[no_mangle]
pub extern "C" fn nr_picker_drag(x: f64, y: f64) -> bool {
    with_picker(false, |view| view.drag(x, y))
}

#[no_mangle]
pub extern "C" fn nr_picker_release() {
    with_picker((), crate::colour_picker::PickerView::release);
}

/// Returns whether the hovered part changed.
#[no_mangle]
pub extern "C" fn nr_picker_hover(x: f64, y: f64) -> bool {
    with_picker(false, |view| view.hover(Some((x, y))))
}

/// Clears hover. Returns whether the hovered part changed.
#[no_mangle]
pub extern "C" fn nr_picker_leave() -> bool {
    with_picker(false, |view| view.hover(None))
}

/// An edit from the picker's fields or eyedropper.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrPickerAct {
    /// `text` typed into channel field `x` (0 red, 1 green, 2 blue).
    Typed,
    /// The hex field lost focus holding `text`.
    LeftHex,
    /// The eyedropper sampled colour `x`, `y`, `z` (channels 0 to 1).
    Eyedropper,
}

/// Applies a field edit or eyedropper sample. Returns whether the colour changed.
///
/// # Safety
/// `text` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_picker_act(
    action: NrPickerAct,
    x: f64,
    y: f64,
    z: f64,
    text: *const c_char,
) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let text = unsafe { text_of(text) };
    with_picker(false, |view| match action {
        NrPickerAct::Typed => view.picker.type_channel(x as usize, &text),
        NrPickerAct::LeftHex => view.picker.commit_hex(&text),
        NrPickerAct::Eyedropper => {
            view.picker.sampled(x, y, z);
            true
        }
    })
}

/// What the hex field should display while the user types `typed`.
///
/// # Safety
/// `typed` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_picker_typed_hex(typed: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    lend(crate::settings::appearance::picker::typed_hex(&unsafe {
        text_of(typed)
    }))
}

/// The picker's frame for `swatch` in a window with content rect `window` (screen coordinates,
/// y up). It opens left of the swatch with tops aligned, stays inside the window
/// (`menus::keys::place_left`) and snaps to whole pixels at `scale`.
#[no_mangle]
pub extern "C" fn nr_picker_place(
    swatch: crate::ffi::NrRect,
    window: crate::ffi::NrRect,
    scale: f64,
) -> crate::ffi::NrRect {
    use crate::colour_picker::{HEIGHT, WIDTH};
    use crate::menus::keys::Rect;
    use crate::settings::appearance::picker::{GAP, MARGIN};
    let anchor = Rect {
        x: swatch.x,
        y: swatch.y,
        width: swatch.width,
        height: swatch.height,
    };
    let bounds = Rect {
        x: window.x + MARGIN,
        y: window.y + MARGIN,
        width: window.width - 2.0 * MARGIN,
        height: window.height - 2.0 * MARGIN,
    };
    let (x, y) = crate::menus::keys::place_left(anchor, WIDTH, HEIGHT, bounds, GAP, scale);
    crate::ffi::NrRect {
        x,
        y,
        width: WIDTH,
        height: HEIGHT,
    }
}

/// The contrast value at `share` (0 to 1) along the appearance slider.
#[no_mangle]
pub extern "C" fn nr_slider_value(share: f64) -> f64 {
    crate::settings::appearance::contrast_at(share)
}

/// The slider's value label, as a whole number.
#[no_mangle]
pub extern "C" fn nr_slider_words(value: f64) -> NrText {
    lend(crate::settings::appearance::contrast_words(value))
}

/// Theme token for a switch thumb, on or off, in the active theme.
#[no_mangle]
pub extern "C" fn nr_switch_thumb(on: bool) -> NrText {
    lend(
        crate::theme::switch_thumb(crate::ffi::nr_appearance_is_dark(), on)
            .name()
            .to_owned(),
    )
}

// MARK: Onboarding

/// Whether a drive is open. False until a usable folder is chosen (`nr_drive_start`,
/// `nr_selection_choose`); until then the window shows onboarding.
#[no_mangle]
pub extern "C" fn nr_drive_ready() -> bool {
    crate::ffi::with_columns(|_| ()).is_some()
}

/// Onboarding layout for a `width` × `height` view (`screen_layout::Onboarding`). `refusal` is
/// why the last picked folder was rejected, or empty (`selection::onboarding::note`).
pub fn onboarding_layout(
    width: f64,
    height: f64,
    refusal: &str,
) -> crate::window::screen_layout::Onboarding {
    let selection = crate::drive::selection::read(&crate::paths::get().settings_file);
    let language = crate::ffi::interface_language();
    let note = crate::drive::selection::onboarding::note(&selection, refusal, language);
    crate::window::screen_layout::onboarding(
        width,
        height,
        note,
        language,
        &mut crate::text::typeset::width,
    )
}

/// Onboarding's Start button: creates `~/Northroom` (`paths::Paths::start_folder`) if needed
/// and opens it as the drive like `nr_selection_choose`. Returns the refusal, or empty.
#[no_mangle]
pub extern "C" fn nr_selection_start() -> NrText {
    let paths = crate::paths::get();
    let language = crate::ffi::interface_language();
    if let Err(refusal) = crate::drive::selection::make_start_folder(&paths.start_folder, language)
    {
        return lend(refusal);
    }
    let text = |path: &Path| std::ffi::CString::new(path.to_string_lossy().into_owned());
    let (Ok(support), Ok(folder)) = (text(&paths.support), text(&paths.start_folder)) else {
        return lend(
            crate::strings::text(language, crate::strings::key::ONBOARDING_START_REFUSED)
                .to_owned(),
        );
    };
    // SAFETY: both are NUL-terminated strings that outlive the call.
    unsafe { crate::ffi::nr_selection_choose(support.as_ptr(), folder.as_ptr()) }
}

/// Confirm-button label for the native folder picker opened from onboarding and Settings.
#[no_mangle]
pub extern "C" fn nr_folder_picker_button() -> NrText {
    lend(
        crate::strings::text(
            crate::ffi::interface_language(),
            crate::strings::key::ONBOARDING_PICKER_BUTTON,
        )
        .to_owned(),
    )
}

// MARK: Page `/` menu and drag chip

/// Scrolls the page's `/` menu by `delta` (down is positive) in `unit` (0 points, 3 clicks),
/// clamped to its rows (`menus::slash::scroll`). `ffi::page_slash` then places them.
#[no_mangle]
pub extern "C" fn nr_page_slash_scroll(delta: f64, unit: u32) {
    crate::ffi::scroll_page_slash(delta, unit);
}

/// Size and text of the chip that follows the pointer during a file drag (`nr_drag_label`).
pub fn drag_chip(label: &str) -> crate::window::screen_layout::DragChip {
    crate::window::screen_layout::drag_chip(label, &mut crate::text::typeset::width)
}

/// The pointer's offset from the drag chip's top-left, on both axes.
#[no_mangle]
pub extern "C" fn nr_drag_chip_pointer() -> f64 {
    crate::window::screen_layout::CHIP_POINTER
}

/// Settings' heading above the reason a folder could not become the drive.
#[no_mangle]
pub extern "C" fn nr_choose_refused() -> NrText {
    lend(
        crate::strings::text(
            crate::ffi::interface_language(),
            crate::strings::key::ONBOARDING_CHOOSE_REFUSED,
        )
        .to_owned(),
    )
}

/// Placeholder text for an empty folder's column.
#[no_mangle]
pub extern "C" fn nr_column_empty_text() -> NrText {
    lend(
        crate::columns::Placeholder::Empty
            .words(crate::ffi::interface_language())
            .to_owned(),
    )
}

// MARK: Details pane

/// Details pane layout for `width` × `height` (`details::Layout`), or `None` when nothing is
/// loaded (`nr_details_load`). `media` is what is known so far about the picture or video;
/// `fills` means the file was opened to fill the tab.
pub fn details_layout(
    width: f64,
    height: f64,
    media: crate::preview::details::Media,
    fills: bool,
) -> Option<crate::preview::details::Layout> {
    crate::ffi::details_layout(width, height, media, fills)
}

/// Whether the platform can read a picture's size from its file header when the file index
/// lacks it, for `extension` (lowercase, no dot).
///
/// # Safety
/// `extension` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_reads_picture_size(extension: *const c_char) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    crate::workspace::reads_picture_size(&unsafe { text_of(extension) })
}

/// Opens an HTML file as a live page (`page::watch`). Returns the id for later calls.
///
/// # Safety
/// `file` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_html_page_open(file: *const c_char) -> u64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let file = unsafe { text_of(file) };
    let id = crate::page::watch::with(|pages| pages.open(Path::new(&file)));
    crate::ffi::watch_open_columns();
    id
}

/// Records that page `id` loaded, reading `served` (folder paths, one per line).
///
/// # Safety
/// `served` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_html_page_loaded(id: u64, served: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let served = unsafe { text_of(served) };
    let served: Vec<String> = served
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    crate::page::watch::with(|pages| pages.loaded(id, &served));
    crate::ffi::watch_open_columns();
}

/// Whether page `id` should reload because its file or a file it read changed on disk.
/// Reports each change once. Also starts watching the folder of any database the page has
/// queried since the last call.
#[no_mangle]
pub extern "C" fn nr_html_page_changed(id: u64) -> bool {
    if crate::page::watch::with(|pages| pages.take_asked_joined()) {
        crate::ffi::watch_open_columns();
    }
    crate::page::watch::with(|pages| pages.changed(id))
}

/// Stops tracking page `id` once it leaves the screen.
#[no_mangle]
pub extern "C" fn nr_html_page_close(id: u64) {
    crate::page::watch::with(|pages| pages.close(id));
    crate::ffi::watch_open_columns();
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_platform_reads_the_size_of_pictures_it_decodes() {
        for extension in ["png", "jpg", "tiff", "bmp"] {
            assert!(
                crate::workspace::reads_picture_size(extension),
                "{extension}"
            );
        }
        // Image I/O reads HEIF and AVIF, as does WIC with Windows' extensions installed; the
        // `image` crate on Linux does not.
        for extension in ["heic", "heif", "avif"] {
            assert_eq!(
                crate::workspace::reads_picture_size(extension),
                crate::workspace::phone_still_decodes_here(extension),
                "{extension}"
            );
        }
        for extension in ["svg", "mp4", "txt", "PNG"] {
            assert!(
                !crate::workspace::reads_picture_size(extension),
                "{extension}"
            );
        }
    }
}
