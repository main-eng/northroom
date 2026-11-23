//! What kind of run this is, read once from the launch environment: a person using the app, or
//! an automated check that sets its own theme and can freeze animation. Shells ask here instead
//! of reading the environment, so all of them treat checks the same way.
//!
//! An empty variable counts as unset.

use std::sync::OnceLock;

/// A check's theme (`NR_THEME`): `dark` or `light`, optionally followed by background, text,
/// accent and call-to-action colours and the contrast, comma-separated.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckTheme {
    /// The Settings appearance the check's reference images used: `dark` or `light`. `None` for
    /// any other first word.
    pub dark: Option<bool>,
    /// The colours, when all five are given.
    pub colours: Option<CheckColours>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckColours {
    /// The first word is `dark`.
    pub dark: bool,
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub call_to_action: String,
    pub contrast: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    /// An automated check, set by `NR_DO` or `NR_SNAPSHOT_DIR`. It takes no focus, stays out of
    /// the Dock and app switcher, and never touches the system trash or clipboard.
    pub unattended: bool,
    /// A check whose steps include real events (`NR_DO=…ev:…`). Its window acts as key and every
    /// view accepts clicks, because the system won't focus a scripted window while a person
    /// works in another app.
    pub scripted: bool,
    /// False under `NR_NO_TERMINAL`, which leaves the terminal out for comparison runs.
    pub terminal: bool,
    /// `NR_PREVIEW_SILENT`: mute all playback.
    pub silent: bool,
    /// `NR_THEME`. While set, the saved appearance is ignored.
    pub theme: Option<CheckTheme>,
    /// `NR_MOTION_AT=<ms>`: freezes every entrance animation at this time, so a snapshot can
    /// catch it mid-move.
    pub held_entrance_ms: Option<f64>,
    /// `NR_MOTION_EXIT_AT=<ms>`: the same for exits.
    pub held_exit_ms: Option<f64>,
    /// `NR_TRACE_SCROLL`: logs where each scroll event went to stderr.
    pub trace_scroll: bool,
    /// `NR_PAGE_TRACE`: logs the open page's current line, as stored and as shown, to stderr
    /// after every step.
    pub trace_page: bool,
    /// `NR_POSTERS`: draw film posters (`film::poster`) in a check. Unattended runs skip them
    /// by default so snapshots don't depend on render timing.
    pub posters: bool,
}

impl Launch {
    /// Reads the run from `variable`, which looks up an environment variable.
    pub fn read(variable: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| variable(name).filter(|value| !value.is_empty());
        let milliseconds = |name: &str| set(name).and_then(|value| value.parse::<f64>().ok());
        Self {
            unattended: set("NR_DO").is_some() || set("NR_SNAPSHOT_DIR").is_some(),
            scripted: set("NR_DO").is_some_and(|steps| steps.contains("ev:")),
            terminal: set("NR_NO_TERMINAL").is_none(),
            silent: set("NR_PREVIEW_SILENT").is_some(),
            theme: set("NR_THEME").map(|text| check_theme(&text)),
            held_entrance_ms: milliseconds("NR_MOTION_AT"),
            held_exit_ms: milliseconds("NR_MOTION_EXIT_AT"),
            trace_scroll: set("NR_TRACE_SCROLL").is_some(),
            trace_page: set("NR_PAGE_TRACE").is_some(),
            posters: set("NR_POSTERS").is_some(),
        }
    }

    /// Whether playback is muted: by `NR_PREVIEW_SILENT`, or because a check has no listener.
    /// Every player's output follows this (`sound_output`).
    pub fn quiet(&self) -> bool {
        self.silent || self.unattended
    }

    /// Whether the theme may follow the system appearance. A check with `NR_THEME` never does.
    pub fn follows_system_look(&self) -> bool {
        self.theme.is_none()
    }

    /// The time at which a check freezes entrances (`entering`) or exits.
    pub fn held_ms(&self, entering: bool) -> Option<f64> {
        if entering {
            self.held_entrance_ms
        } else {
            self.held_exit_ms
        }
    }
}

fn check_theme(text: &str) -> CheckTheme {
    let parts: Vec<&str> = text.split(',').collect();
    let dark = match parts[0] {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    };
    let colours = match parts.as_slice() {
        [look, background, foreground, accent, call_to_action, contrast] => {
            contrast.parse::<f64>().ok().map(|contrast| CheckColours {
                dark: *look == "dark",
                background: (*background).to_owned(),
                foreground: (*foreground).to_owned(),
                accent: (*accent).to_owned(),
                call_to_action: (*call_to_action).to_owned(),
                contrast,
            })
        }
        _ => None,
    };
    CheckTheme { dark, colours }
}

/// This process's run.
pub fn get() -> &'static Launch {
    static LAUNCH: OnceLock<Launch> = OnceLock::new();
    LAUNCH.get_or_init(|| Launch::read(|name| std::env::var(name).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pairs: &[(&str, &str)]) -> Launch {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Launch::read(move |name| {
            pairs
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| value.clone())
        })
    }

    #[test]
    fn a_persons_app_is_no_check() {
        assert_eq!(
            run(&[]),
            Launch {
                unattended: false,
                scripted: false,
                terminal: true,
                silent: false,
                theme: None,
                held_entrance_ms: None,
                held_exit_ms: None,
                trace_scroll: false,
                trace_page: false,
                posters: false,
            }
        );
        assert!(
            !run(&[("NR_DO", "")]).unattended,
            "set to nothing is not set"
        );
    }

    #[test]
    fn a_check_with_events_is_scripted() {
        assert!(run(&[("NR_DO", "board,ev:click:1:2")]).scripted);
        assert!(!run(&[("NR_DO", "board,film:play")]).scripted);
        let quiet = run(&[("NR_NO_TERMINAL", "1"), ("NR_PREVIEW_SILENT", "1")]);
        assert!(!quiet.terminal && quiet.silent);
    }

    #[test]
    fn a_check_or_the_ask_plays_nothing_aloud() {
        assert!(!run(&[]).quiet());
        assert!(run(&[("NR_PREVIEW_SILENT", "1")]).quiet());
        assert!(
            run(&[("NR_SNAPSHOT_DIR", "/tmp/shots")]).quiet(),
            "an unattended run is quiet unasked"
        );
    }

    #[test]
    fn steps_or_pictures_make_a_check() {
        assert!(run(&[("NR_DO", "settings:2")]).unattended);
        assert!(run(&[("NR_SNAPSHOT_DIR", "/tmp/shots")]).unattended);
    }

    #[test]
    fn a_checks_theme_names_its_look_and_maybe_its_colours() {
        let look = run(&[("NR_THEME", "light")]).theme.expect("a theme");
        assert_eq!(
            look,
            CheckTheme {
                dark: Some(false),
                colours: None
            }
        );
        let full = run(&[("NR_THEME", "dark,#111111,#EEEEF0,#4382F2,#EEEEF0,50")])
            .theme
            .expect("a theme");
        assert_eq!(full.dark, Some(true));
        assert_eq!(
            full.colours,
            Some(CheckColours {
                dark: true,
                background: "#111111".into(),
                foreground: "#EEEEF0".into(),
                accent: "#4382F2".into(),
                call_to_action: "#EEEEF0".into(),
                contrast: 50.0,
            })
        );
        // Another first word picks no appearance; a non-numeric contrast gives no colours.
        assert_eq!(
            run(&[("NR_THEME", "sepia")]).theme,
            Some(CheckTheme {
                dark: None,
                colours: None
            })
        );
        assert_eq!(
            run(&[("NR_THEME", "light,#fff,#000,#00f,#000,high")])
                .theme
                .and_then(|theme| theme.colours),
            None
        );
        assert_eq!(
            run(&[("NR_THEME", "light,#fff,#000")])
                .theme
                .and_then(|theme| theme.colours),
            None
        );
    }

    #[test]
    fn a_check_may_trace_its_scrolling() {
        assert!(run(&[("NR_TRACE_SCROLL", "1")]).trace_scroll);
        assert!(
            !run(&[("NR_TRACE_SCROLL", "")]).trace_scroll,
            "set to nothing is not set"
        );
    }

    #[test]
    fn a_check_may_trace_the_line_typed_in() {
        assert!(run(&[("NR_PAGE_TRACE", "1")]).trace_page);
        assert!(
            !run(&[("NR_PAGE_TRACE", "")]).trace_page,
            "set to nothing is not set"
        );
        assert!(!run(&[]).trace_page);
    }

    #[test]
    fn only_a_run_with_no_look_of_its_own_follows_the_systems() {
        assert!(run(&[]).follows_system_look());
        assert!(!run(&[("NR_THEME", "light")]).follows_system_look());
        assert!(
            !run(&[("NR_THEME", "dark,#111111,#EEEEF0,#4382F2,#EEEEF0,50")]).follows_system_look()
        );
    }

    #[test]
    fn a_check_holds_entrances_and_exits_apart() {
        let held = run(&[("NR_MOTION_AT", "75"), ("NR_MOTION_EXIT_AT", "40.5")]);
        assert_eq!(
            (held.held_ms(true), held.held_ms(false)),
            (Some(75.0), Some(40.5))
        );
        assert_eq!(run(&[("NR_MOTION_AT", "soon")]).held_ms(true), None);
    }
}
