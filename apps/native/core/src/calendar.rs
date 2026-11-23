//! Converts between days since 1970-01-01 and Gregorian dates, for any year, using Howard
//! Hinnant's `civil_from_days` and `days_from_civil`.

/// The date `days` after 1970-01-01 (negative is before), as year, month (1 = January) and day.
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// Days from 1970-01-01 to a date (negative before it). `month` is 1 for January.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each direction undoes the other, across leap days, centuries and 1970 itself.
    #[test]
    fn days_and_dates_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        for days in -800_000..800_000 {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }
}
