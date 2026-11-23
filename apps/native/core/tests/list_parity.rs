//! Checks the list model against recorded results (`fixtures/list.json`) for the same sizes,
//! dates and names, in America/Chicago and en-US.

use northroom_core::list::{compare_names, format_date, format_size};
use serde_json::Value;

#[test]
fn writes_sizes_dates_and_name_order_as_recorded() {
    // The fixture's dates use this zone. Set it before any time is read.
    // SAFETY: this binary has one test, and no other thread reads the environment.
    unsafe { std::env::set_var("TZ", "America/Chicago") };
    let fixture: Value = serde_json::from_str(include_str!("fixtures/list.json")).expect("fixture");

    for pair in fixture["sizes"].as_array().expect("sizes") {
        let bytes = pair[0].as_f64().expect("bytes") as u64;
        assert_eq!(
            format_size(Some(bytes)),
            pair[1].as_str().expect("text"),
            "{bytes} bytes"
        );
    }
    assert_eq!(format_size(None), "--");

    let now = fixture["now"].as_f64().expect("now") as i64;
    for pair in fixture["dates"].as_array().expect("dates") {
        let seconds = pair[0].as_f64().expect("seconds") as i64;
        assert_eq!(
            format_date(Some(seconds), now),
            pair[1].as_str().expect("text"),
            "{seconds}"
        );
    }
    assert_eq!(format_date(None, now), "--");

    let mut names: Vec<String> = fixture["names"]
        .as_array()
        .expect("names")
        .iter()
        .map(|name| name.as_str().expect("name").to_owned())
        .collect();
    names.sort_by(|left, right| compare_names(left, right));
    let expected: Vec<&str> = fixture["sorted"]
        .as_array()
        .expect("sorted")
        .iter()
        .map(|name| name.as_str().expect("name"))
        .collect();
    assert_eq!(names, expected);
}
