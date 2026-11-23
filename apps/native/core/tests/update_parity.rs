//! Checks the updater against a recording (`fixtures/update.json`): the controller step by
//! step, and electron-updater's channel file parsing, version order, rollout and digest
//! encoding.

use northroom_core::update::controller::{Controller, Effect, Found, Origin};
use northroom_core::update::{feed, sha512};
use serde_json::Value;

fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/update.json")).expect("update fixtures")
}

/// Maps recorded text to the app's where they differ on purpose: the unsupported message's
/// second sentence is about the Linux AppImage, which does not apply on a Mac (ADR-0010).
fn as_native(line: &str) -> String {
    line.replace(
        "Updates require an installed Northroom release. On Linux, use the AppImage build.",
        "Updates require an installed Northroom release.",
    )
}

fn written(effect: Effect) -> String {
    match effect {
        Effect::Status(status) => format!("status {}", status.name()),
        Effect::Fetch => "fetch".into(),
        Effect::Download => "download".into(),
        Effect::Show(dialog) => format!(
            "show {}|{}|{}|{}|{}|{}",
            if dialog.error { "error" } else { "info" },
            dialog.message,
            dialog.detail,
            dialog
                .buttons
                .iter()
                .map(|button| northroom_core::strings::text("en", *button))
                .collect::<Vec<_>>()
                .join("/"),
            dialog
                .default_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            dialog
                .cancel_id
                .map(|id| id.to_string())
                .unwrap_or_default()
        ),
        Effect::Relaunch => "install silent=false relaunch=true".into(),
        Effect::Log(reason) => format!("error {reason}"),
    }
}

/// Replays one recorded step (`verb words`) on the controller.
fn play(controller: &mut Controller, step: &str) {
    let (verb, words) = step.split_once(' ').unwrap_or((step, ""));
    match verb {
        "request" => controller.request(if words == "manual" {
            Origin::Manual
        } else {
            Origin::Background
        }),
        "found" => {
            let (available, version) = words.split_once(' ').unwrap();
            controller.fetched(Ok(Found {
                available: available == "true",
                version: version.into(),
            }));
        }
        "check-fails" => controller.fetched(Err(words.into())),
        "downloaded" => controller.downloaded_result(Ok(())),
        "download-fails" => controller.downloaded_result(Err(words.into())),
        "answer" => controller.answered(words.parse().unwrap()),
        other => panic!("unknown step {other}"),
    }
}

#[test]
fn the_controller_does_what_was_recorded() {
    let fixtures = fixtures();
    let runs = fixtures["controller"].as_array().unwrap();
    assert!(runs.len() >= 16);
    for run in runs {
        let name = run["name"].as_str().unwrap();
        let mut controller = Controller::new("0.2.55", run["supported"].as_bool().unwrap());
        let mut log = Vec::new();
        for step in run["steps"].as_array().unwrap() {
            let step = step.as_str().unwrap();
            play(&mut controller, step);
            while let Some(effect) = controller.next_effect() {
                log.push(written(effect));
            }
            log.push(format!("> {step}"));
        }
        let recorded: Vec<String> = run["log"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| as_native(line.as_str().unwrap()))
            .collect();
        assert_eq!(log, recorded, "{name}");
    }
}

#[test]
fn channel_files_read_as_electron_updater_reads_them() {
    let fixtures = fixtures();
    let base = feed::base_url("https://api.northroom.com/api/download/update");
    for case in fixtures["feeds"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let info = feed::parse_update_info(
            text,
            "latest-mac.yml",
            "https://api.northroom.com/api/download/update/latest-mac.yml",
        )
        .unwrap();
        assert_eq!(info.version, case["version"].as_str().unwrap(), "{text}");
        let files: Vec<(String, String, Option<u64>)> = case["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                (
                    file["url"].as_str().unwrap().into(),
                    file["sha512"].as_str().unwrap().into(),
                    file["size"].as_u64(),
                )
            })
            .collect();
        let read: Vec<(String, String, Option<u64>)> = info
            .files
            .iter()
            .map(|file| (file.url.clone(), file.sha512.clone(), file.size))
            .collect();
        assert_eq!(read, files, "{text}");
        assert_eq!(
            info.release_notes.as_deref(),
            case["releaseNotes"].as_str(),
            "{text}"
        );
        assert_eq!(
            info.minimum_system_version.as_deref(),
            case["minimumSystemVersion"].as_str(),
            "{text}"
        );
        assert_eq!(
            info.staging_percentage
                .as_deref()
                .map(|value| value.parse::<u64>().unwrap()),
            case["stagingPercentage"].as_u64()
        );
        for (arm64, key) in [(true, "zipArm64"), (false, "zipX64")] {
            let chosen = feed::choose_zip(&info, &base, arm64)
                .map(|(url, _)| url)
                .map_err(|reason| reason.to_string());
            let expected = &case[key];
            match expected["url"].as_str() {
                Some(url) => assert_eq!(chosen, Ok(url.to_owned()), "{key} {text}"),
                None => assert_eq!(
                    chosen,
                    Err(expected["error"].as_str().unwrap().to_owned()),
                    "{key} {text}"
                ),
            }
        }
    }
}

#[test]
fn versions_order_as_semver_orders_them() {
    for case in fixtures()["versions"].as_array().unwrap() {
        let info = feed::UpdateInfo {
            version: case["latest"].as_str().unwrap().into(),
            files: Vec::new(),
            release_notes: None,
            staging_percentage: None,
            minimum_system_version: None,
        };
        let current = case["current"].as_str().unwrap();
        let available = feed::is_update_available(
            &info,
            current,
            "25.0.0",
            "00000000-0000-5000-8000-000000000000",
        );
        assert_eq!(
            available.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{case}"
        );
        if let Ok(available) = available {
            assert_eq!(available, case["available"].as_bool().unwrap(), "{case}");
        }
    }
}

#[test]
fn the_rollout_takes_the_same_computers() {
    for case in fixtures()["staging"].as_array().unwrap() {
        let percentage = case["percentage"].as_u64().unwrap().to_string();
        assert_eq!(
            feed::within_rollout(Some(&percentage), case["id"].as_str().unwrap()),
            case["within"].as_bool().unwrap(),
            "{case}"
        );
    }
    assert!(feed::within_rollout(
        None,
        "00000000-0000-5000-8000-000000000000"
    ));
}

#[test]
fn a_digest_is_read_in_the_same_encoding() {
    for case in fixtures()["digests"].as_array().unwrap() {
        assert_eq!(
            sha512::expected_is_hex(case["expected"].as_str().unwrap()),
            case["hex"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn a_downloaded_file_is_checked_against_its_digest() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("update.zip");
    std::fs::write(&file, b"x").unwrap();
    // `x`'s digest in base64, as electron-builder writes it, and in hex, as some feeds do.
    let digests = fixtures()["digests"].as_array().unwrap().clone();
    let (hex, base64) = (
        digests[0]["expected"].as_str().unwrap(),
        digests[1]["expected"].as_str().unwrap(),
    );
    assert_eq!(sha512::check_file(&file, base64), Ok(()));
    assert_eq!(sha512::check_file(&file, hex), Ok(()));
    std::fs::write(&file, b"y").unwrap();
    let refused = sha512::check_file(&file, base64).unwrap_err();
    assert!(
        refused.starts_with(&format!(
            "sha512 checksum mismatch, expected {base64}, got "
        )),
        "{refused}"
    );
}
