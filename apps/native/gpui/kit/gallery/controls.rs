//! Form control scenes (`form-*`). Each draws kit's controls where the core lays them out on an
//! app screen (the settings page, the colour picker), in the app theme, and saves the window as
//! `<folder>/<scene>.png`. `form-states` covers states the settings page doesn't show.
//!
//! Usage: kit-gallery <folder> form-<scene> ...
//! Environment: KIT_THEME=dark|light (default dark).

use gpui::{
    AnyView, App, AppContext, AsyncApp, Bounds, Context, Entity, Hsla, IntoElement, ParentElement,
    Pixels, Point, Render, Styled, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowOptions, canvas, div, point, px, size,
};
use kit::controls::button::{Button, Look, Words};
use kit::controls::input::{Field, Placeholder};
use kit::controls::picker::{Picker, Swatch};
use kit::controls::select::{Choice, Select, Swatch as Dot};
use kit::controls::slider::Slider;
use kit::controls::theme_choice::ThemeChoice;
use kit::controls::{Drawn, Frame};
use kit::controls::{Line, Rect, spinner};
use kit::core::{c, frame, inset};
use kit::draw::{fill, rounded, stroke};
use kit::icons::Glyph;
use kit::nr::*;
use kit::theme::{self, Token, color};
use northroom_core::app_ffi::{settings_page, settings_window};
use northroom_core::dialog::layout::Size;
use northroom_core::settings::page::Control;
use northroom_core::settings::page::layout::PageLayout;
use std::path::{Path, PathBuf};
use std::time::Duration;

static INTER: &[u8] = include_bytes!("../../../core/assets/fonts/Inter-latin.woff2");
static GEIST_MONO: &[u8] = include_bytes!("../../../core/assets/fonts/GeistMono-latin.woff2");

/// The window the app's scripted runs are in.
const WINDOW: (f32, f32) = (1280.0, 800.0);

const SCENES: [&str; 4] = [
    "form-settings-appearance",
    "form-settings-picker",
    "form-settings-picker-rgb",
    "form-states",
];

/// Whether a scene name is one of these.
pub fn owns(name: &str) -> bool {
    name.starts_with("form-")
}

/// Runs `scenes` (every one when none is named), saving their pictures in `folder`, then quits.
pub fn run(folder: PathBuf, scenes: Vec<String>) {
    let theme = std::env::var("KIT_THEME").unwrap_or_else(|_| "dark".to_owned());
    let colours = theme_colours(&theme);
    std::fs::create_dir_all(folder.join("drive/workspace")).expect("the gallery's folder is made");
    let place = |name: &str| folder.join(name).to_string_lossy().into_owned();
    // Run as a scripted app with its own theme and folders.
    let settings = [
        ("NR_THEME", colours.to_owned()),
        ("NR_NO_TERMINAL", "1".to_owned()),
        ("NR_PREVIEW_SILENT", "1".to_owned()),
        ("NR_WORKSPACE", place("drive/workspace")),
        ("NR_METADATA", place("metadata")),
        ("NR_SUPPORT", place("support")),
        ("NR_TRASH", place("trash")),
    ];
    for (name, value) in settings {
        // SAFETY: set before the core or GPUI starts a thread; threads only read them.
        unsafe { std::env::set_var(name, value) };
    }
    let chosen: Vec<&'static str> = SCENES
        .into_iter()
        .filter(|scene| scenes.is_empty() || scenes.iter().any(|name| name == scene))
        .collect();
    // Match the app (`shell/src/main.rs`). Windows needs no change.
    #[cfg(target_os = "macos")]
    kit::platform::draw_glyphs_unsmoothed();
    // SAFETY: called before the app starts any thread.
    #[cfg(target_os = "linux")]
    unsafe {
        kit::platform::draw_glyphs_unsmoothed()
    };
    nr_app_start();
    let (support, logs, caches) = (c(&place("support")), c(&place("logs")), c(&place("caches")));
    // SAFETY: the paths are NUL-terminated.
    unsafe { nr_paths_open(support.as_ptr(), logs.as_ptr(), caches.as_ptr()) };
    let _ = nr_launch();
    gpui_platform::application().run(move |cx: &mut App| {
        cx.text_system()
            .add_fonts(kit::fonts::faces(INTER, GEIST_MONO))
            .expect("the app's own faces load");
        kit::text::install(cx.text_system().clone());
        nr_appearance_start(theme == "dark");
        nr_set_fallback_measure(kit::text::measure_fallback);
        let _ = nr_drive_start(NrFileSystem::NONE);
        cx.bind_keys(kit::controls::input::bindings());
        cx.spawn(async move |cx| {
            for scene in chosen {
                show(scene, &folder, cx).await;
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

/// The `NR_THEME` value for `theme`: name, background, ink colours and contrast.
fn theme_colours(theme: &str) -> &'static str {
    match theme {
        "light" => "light,#fafafb,#111113,#111113,#111113,50",
        _ => "dark,#111111,#eeeef0,#eeeef0,#eeeef0,50",
    }
}

/// Opens the window for scene `name`, saves it once drawn, and closes it.
async fn show(name: &str, folder: &Path, cx: &mut AsyncApp) {
    if shows_settings(name) {
        nr_settings_open();
    }
    let options = window_options();
    let handle: WindowHandle<Stage> = cx
        .update(|cx| {
            cx.open_window(options, |window, cx| {
                cx.new(|cx| Stage::for_scene(name, window, cx))
            })
        })
        .expect("the window opens");
    #[cfg(target_os = "macos")]
    let _ = cx.update_window(handle.into(), |_, window, _| {
        kit::platform::put_check_window_off_screen(window, WINDOW.0.into(), WINDOW.1.into())
    });
    // Wait a few frames so the fields and picker parts are placed and drawn.
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    let path = folder.join(format!("{name}.png"));
    let _ = cx.update_window(handle.into(), |_, window, cx| {
        save_window_picture(window, cx, &path);
    });
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

/// Whether a scene shows settings: all but the states do.
fn shows_settings(name: &str) -> bool {
    name != "form-states"
}

/// Window options for a scripted run: sized, transparent title bar, offscreen and unfocused.
fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(WINDOW.0), px(WINDOW.1)),
        })),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: None,
        }),
        focus: false,
        show: false,
        inactive_frame_interval: None,
        use_display_p3: kit::display::DISPLAY_P3,
        ..Default::default()
    }
}

/// Draws the window and saves it to `path`, logging when that fails.
fn save_window_picture(window: &mut Window, cx: &mut App, path: &Path) {
    window.draw(cx).clear(cx);
    match window.render_to_image() {
        Ok(image) => {
            if let Err(error) = kit::display::save(&image, &path.to_string_lossy()) {
                eprintln!(
                    "gallery\tthe picture {} was not saved: {error}",
                    path.display()
                );
            }
        }
        Err(error) => eprintln!("gallery\tno picture of the window: {error}"),
    }
}

// MARK: The core's descriptions

/// A settings control as a kit view (none for words), with the size it takes.
fn build(control: &Control, cx: &mut Context<Stage>) -> ((f32, f32), Option<AnyView>) {
    {
        match control {
            Control::Words { text } => (
                (kit::text::width(text, theme::Face::sans(14.0, 400.0)), 20.0),
                None,
            ),
            Control::Select {
                enabled,
                choices,
                selected,
                width,
                ..
            } => {
                let choices = choices
                    .iter()
                    .map(|choice| Choice {
                        id: choice.id.clone(),
                        name: choice.name.clone(),
                        swatch: choice.swatch.as_deref().map(|swatch| Dot {
                            color: theme::named(swatch),
                        }),
                    })
                    .collect();
                let select = cx.new(|_| {
                    let mut select = Select::new(choices, *selected);
                    select.enabled = *enabled;
                    select
                });
                (
                    (*width as f32, kit::controls::select::HEIGHT),
                    Some(select.into()),
                )
            }
            Control::Theme {
                labels, selected, ..
            } => {
                let choice = ThemeChoice::new(labels.clone(), *selected);
                let size = choice.size();
                (size, Some(cx.new(|_| choice).into()))
            }
            Control::Color { hex, .. } => {
                let swatch = Swatch::new(hex);
                let size = swatch.size();
                (size, Some(cx.new(|_| swatch).into()))
            }
            Control::Slider { value, .. } => (
                kit::controls::slider::SIZE,
                Some(cx.new(|_| Slider::new(*value)).into()),
            ),
        }
    }
}

// MARK: The stage

/// What a scene draws under its controls.
#[derive(Clone)]
enum Under {
    Fill(Bounds<Pixels>, Hsla, f32),
    Edge(Bounds<Pixels>, Hsla, f32),
    Words(Line),
    /// The spinner in a theme colour, frozen at a moment.
    Spinner(Bounds<Pixels>, Token, f64),
}

struct Stage {
    ground: Vec<Under>,
    controls: Vec<AnyView>,
}

impl Stage {
    fn new() -> Self {
        Self {
            ground: vec![Under::Fill(
                frame(0.0, 0.0, WINDOW.0, WINDOW.1),
                color(Token::Background),
                0.0,
            )],
            controls: Vec::new(),
        }
    }

    fn add<T: Render>(&mut self, view: Entity<T>) {
        self.controls.push(view.into());
    }

    /// What scene `name` draws.
    fn for_scene(name: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        match name {
            "form-states" => Self::states(window, cx),
            "form-settings-picker" => Self::picker(false, window, cx),
            "form-settings-picker-rgb" => Self::picker(true, window, cx),
            _ => Self::settings(cx),
        }
    }

    /// The settings page as the core lays it out, with kit controls.
    fn settings(cx: &mut Context<Self>) -> Self {
        let mut stage = Self::new();
        stage.settings_page(cx);
        stage
    }

    /// Lays out the settings page, adds its controls, and returns them with their frames.
    fn settings_page(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<(Control, Bounds<Pixels>, Option<AnyView>)> {
        let page = settings_page();
        let controls: Vec<Control> = page.controls().cloned().collect();
        // Each control as a kit view, with the size it takes.
        let mut built: Vec<(Control, (f32, f32), Option<AnyView>)> = Vec::new();
        for control in controls {
            let (size, view) = build(&control, cx);
            built.push((control, size, view));
        }
        let sizes: Vec<Size> = built
            .iter()
            .map(|(_, (width, height), _)| Size {
                width: f64::from(*width),
                height: f64::from(*height),
            })
            .collect();
        let content = nr_window_layout(
            f64::from(WINDOW.0),
            f64::from(WINDOW.1),
            0,
            NrOpenedViewKind::None,
            true,
        )
        .content;
        let laid = settings_window(&page, content.width, content.height, &sizes);
        let column = laid.column.bounds();
        let pane = point(
            px(content.x as f32)
                + column.origin.x
                + px(laid.document.x as f32)
                + px(laid.page_frame.x as f32),
            px(content.y as f32)
                + column.origin.y
                + px(laid.document.y as f32)
                + px(laid.page_frame.y as f32),
        );
        let PageLayout {
            boxes,
            dividers,
            lines,
            controls: placements,
            ..
        } = laid.page;
        self.add_page_ground(&boxes, &dividers, &lines, pane);
        let mut placed_controls = Vec::new();
        for ((control, _, view), placed) in built.into_iter().zip(placements) {
            let at = placed.frame.at(pane);
            self.place_control(&control, view.as_ref(), at, cx);
            placed_controls.push((control, at, view));
        }
        placed_controls
    }

    /// Draws a settings page's boxes, dividers and text with its origin at `pane`.
    fn add_page_ground(
        &mut self,
        boxes: &[Rect],
        dividers: &[Rect],
        lines: &[Line],
        pane: Point<Pixels>,
    ) {
        for placed in boxes {
            let at = placed.at(pane);
            self.ground.push(Under::Fill(at, color(Token::Card), 6.0));
            self.ground
                .push(Under::Edge(inset(at, 0.5, 0.5), color(Token::Border), 5.5));
        }
        for placed in dividers {
            self.ground
                .push(Under::Fill(placed.at(pane), color(Token::Border), 0.0));
        }
        for line in lines {
            let at = line.frame.at(pane);
            self.ground.push(Under::Words(Line {
                text: line.text.clone(),
                frame: Rect {
                    x: f64::from(f32::from(at.origin.x)),
                    y: f64::from(f32::from(at.origin.y)),
                    width: line.frame.width,
                    height: line.frame.height,
                },
                font: line.font,
                color: line.color,
                opacity: line.opacity,
            }));
        }
    }

    /// Places a page control at `at`: text drawn on the background, or its view.
    fn place_control(
        &mut self,
        control: &Control,
        view: Option<&AnyView>,
        at: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Control::Words { text } = control {
            self.ground.push(Under::Words(Line {
                text: text.clone(),
                frame: Rect {
                    x: f64::from(f32::from(at.origin.x)),
                    y: f64::from(f32::from(at.origin.y)),
                    width: 10_000.0,
                    height: 20.0,
                },
                font: kit::controls::Font {
                    size: 14.0,
                    weight: 400.0,
                    mono: false,
                    slanted: false,
                    tabular: false,
                },
                color: Token::MutedForeground.to_owned(),
                opacity: 1.0,
            }));
        }
        let Some(view) = view else {
            return;
        };
        self.place(view, at, cx);
        self.controls.push(view.clone());
    }

    /// Puts a control view at `at`.
    fn place(&self, view: &AnyView, at: Bounds<Pixels>, cx: &mut Context<Self>) {
        if let Ok(control) = view.clone().downcast::<Select>() {
            control.update(cx, |select, cx| select.place(at, cx));
        } else if let Ok(control) = view.clone().downcast::<Button>() {
            control.update(cx, |button, cx| button.place(at, cx));
        } else if let Ok(control) = view.clone().downcast::<ThemeChoice>() {
            control.update(cx, |choice, cx| choice.place(at, cx));
        } else if let Ok(control) = view.clone().downcast::<Swatch>() {
            control.update(cx, |swatch, cx| swatch.place(at, cx));
        } else if let Ok(control) = view.clone().downcast::<Slider>() {
            control.update(cx, |slider, cx| slider.place(at, cx));
        }
    }

    /// Appearance with the light theme's accent swatch pressed and the picker open beside it,
    /// showing channel fields when `channels`.
    fn picker(channels: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut stage = Self::new();
        let controls = stage.settings_page(cx);
        let (hex, swatch) = controls
            .iter()
            .find_map(|(control, _, view)| match control {
                Control::Color { hex, .. } => view
                    .clone()
                    .and_then(|view| view.downcast::<Swatch>().ok())
                    .map(|swatch| (hex.clone(), swatch.read(cx).button())),
                _ => None,
            })
            .expect("Appearance has the accent's swatch");
        let picker = cx.new(|cx| Picker::new(&hex, window, cx));
        // The core's position beside the swatch, in screen coordinates (y up) for this window.
        let up = |at: Bounds<Pixels>| NrRect {
            x: f64::from(f32::from(at.origin.x)),
            y: f64::from(WINDOW.1 - f32::from(at.origin.y + at.size.height)),
            width: f64::from(f32::from(at.size.width)),
            height: f64::from(f32::from(at.size.height)),
        };
        let placed = nr_picker_place(up(swatch), up(frame(0.0, 0.0, WINDOW.0, WINDOW.1)), 2.0);
        let at = frame(
            placed.x as f32,
            WINDOW.1 - (placed.y + placed.height) as f32,
            placed.width as f32,
            placed.height as f32,
        );
        picker.update(cx, |picker, cx| {
            picker.place(at, cx);
            if channels {
                let mode = picker.layout().mode.at(at.origin);
                picker.press(mode.center(), window, cx);
                kit::nr::nr_picker_release();
                kit::nr::nr_picker_leave();
            }
        });
        stage.add(picker);
        stage
    }

    /// States the settings page doesn't show: every button look hovered, pressed, disabled and
    /// focused, plus selects, edited fields, sliders, a hovered theme choice and the spinner.
    fn states(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut stage = Self::new();
        let card = frame(40.0, 40.0, 1200.0, 720.0);
        stage
            .ground
            .push(Under::Fill(card, color(Token::Card), 6.0));
        stage.ground.push(Under::Edge(
            inset(card, 0.5, 0.5),
            color(Token::Border),
            5.5,
        ));
        let mut y = 64.0;
        for look in [
            Look::Primary,
            Look::Outline,
            Look::Destructive,
            Look::Bordered,
        ] {
            stage.add_button_states(look, y, cx);
            y += 44.0;
        }
        stage.add_selects(y, cx);
        y += 52.0;
        stage.add_field_states(y, window, cx);
        y += 52.0;
        stage.add_sliders(y, cx);
        y += 52.0;
        stage.add_theme_choice_and_swatches(y, cx);
        y += 60.0;
        stage.add_spinners(y);
        stage
    }

    /// A row of `look` buttons at `y`: idle, hovered, pressed, disabled, focused.
    fn add_button_states(&mut self, look: Look, y: f32, cx: &mut Context<Self>) {
        let mut x = 64.0;
        for (hovered, pressed, enabled, ring) in [
            (false, false, true, false),
            (true, false, true, false),
            (true, true, true, false),
            (false, false, false, false),
            (false, false, true, true),
        ] {
            let icon = (look == Look::Outline).then_some(Glyph::FolderOpen);
            let mut button = Button::new(
                look,
                Words::Title {
                    title: "Save changes".to_owned(),
                    icon,
                    inset: 10.0,
                },
            );
            (button.hovered, button.pressed, button.enabled, button.ring) =
                (hovered, pressed, enabled, ring);
            let (w, h) = button.size();
            button.frame = frame(x, y, w, h);
            x += w + 24.0;
            self.add(cx.new(|_| button));
        }
    }

    /// Selects: idle, hovered, disabled, focused, with a swatch.
    fn add_selects(&mut self, y: f32, cx: &mut Context<Self>) {
        let mut x = 64.0;
        let choices = |swatch: Option<Dot>| {
            vec![Choice {
                id: "one".to_owned(),
                name: "Monochrome".to_owned(),
                swatch,
            }]
        };
        for (swatch, hovered, enabled, ring) in [
            (None, false, true, false),
            (None, true, true, false),
            (None, false, false, false),
            (None, false, true, true),
            (
                Some(Dot {
                    color: color(Token::Primary),
                }),
                false,
                true,
                false,
            ),
        ] {
            let mut select = Select::new(choices(swatch), 0);
            (select.hovered, select.enabled, select.ring) = (hovered, enabled, ring);
            select.frame = frame(x, y, 140.0, 26.0);
            self.add(cx.new(|_| select));
            x += 156.0;
        }
    }

    /// Fields: empty, filled, focused with a selection, disabled.
    fn add_field_states(&mut self, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        let mut x = 64.0;
        for (state, words) in [
            ("empty", ""),
            ("words", "parity-agent@northroom.test"),
            ("selected", "Selected words"),
            ("unusable", "Cannot change"),
        ] {
            let at = frame(x, y, 200.0, 26.0);
            let entity = cx.new(|cx| {
                let mut field = Field::new("Field", words, window, cx);
                field.placeholder = Placeholder::Words("Display name".to_owned());
                field.enabled = state != "unusable";
                field.blinks = false;
                field.place(at, Field::settings(at), cx);
                if state == "selected" {
                    field.editing.select_all();
                    field.focus(window, cx);
                }
                field
            });
            self.add(entity);
            x += 220.0;
        }
    }

    /// Sliders at 0, 50, 100.
    fn add_sliders(&mut self, y: f32, cx: &mut Context<Self>) {
        let mut x = 64.0;
        for value in [0.0, 50.0, 100.0] {
            let mut slider = Slider::new(value);
            slider.frame = frame(x, y + 2.0, 256.0, 20.0);
            self.add(cx.new(|_| slider));
            x += 280.0;
        }
    }

    /// The theme choice with Light hovered, and colour swatches idle and hovered.
    fn add_theme_choice_and_swatches(&mut self, y: f32, cx: &mut Context<Self>) {
        let mut choice = ThemeChoice::new(
            vec!["Light".to_owned(), "Dark".to_owned(), "System".to_owned()],
            1,
        );
        choice.hovered = Some(0);
        let (w, h) = choice.size();
        choice.frame = frame(64.0, y, w, h);
        self.add(cx.new(|_| choice));
        let mut x = 64.0 + w + 32.0;
        for hovered in [false, true] {
            let mut swatch = Swatch::new("#3b82f6");
            swatch.hovered = hovered;
            let (w, h) = swatch.size();
            swatch.frame = frame(x, y + 4.0, w, h);
            self.add(cx.new(|_| swatch));
            x += w + 24.0;
        }
    }

    /// The spinner at four moments.
    fn add_spinners(&mut self, y: f32) {
        for (index, seconds) in [0.0, 0.15, 0.3, 0.45].into_iter().enumerate() {
            self.ground.push(Under::Spinner(
                frame(64.0 + index as f32 * 40.0, y + 8.0, 16.0, 16.0),
                Token::Foreground,
                seconds,
            ));
        }
    }
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        div()
            .size_full()
            .relative()
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| {
                        let ground: Vec<Under> = this.read(cx).ground.clone();
                        for part in ground {
                            match part {
                                Under::Fill(at, shade, radius) => {
                                    if radius == 0.0 {
                                        fill(window, at, shade);
                                    } else {
                                        rounded(window, at, shade, radius);
                                    }
                                }
                                Under::Edge(at, shade, radius) => {
                                    stroke(window, at, shade, radius, 1.0)
                                }
                                Under::Words(line) => {
                                    line.draw(window, cx, point(px(0.0), px(0.0)), None)
                                }
                                Under::Spinner(at, ink, seconds) => {
                                    spinner::paint(window, cx, at, ink, seconds, false)
                                }
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(self.controls.iter().cloned())
    }
}
