//! Local times as the cards and sheets write them: Go's "15:04" and
//! "20060102-150405.000" and "2006-01-02 15:04".

use rush_core::time::Ms;

/// The local date and time of ms: year, month, day, hour, minute, second.
pub fn civil(ms: Ms) -> (i64, u32, u32, u32, u32, u32) {
    let secs = ms.div_euclid(1000) + crate::model::local_zone(ms);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (
        y,
        mo,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    )
}

/// "15:04", local.
pub fn hhmm(ms: Ms) -> String {
    let (_, _, _, h, mi, _) = civil(ms);
    format!("{h:02}:{mi:02}")
}

/// "2006-01-02 15:04", local.
pub fn date_time(ms: Ms) -> String {
    let (y, mo, d, h, mi, _) = civil(ms);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

/// "20060102-150405.000", local: a file's name.
pub fn stamp(ms: Ms) -> String {
    let (y, mo, d, h, mi, s) = civil(ms);
    format!(
        "{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.{:03}",
        ms.rem_euclid(1000)
    )
}

/// d rounded to the minute, as Go's Duration.Round(time.Minute) prints:
/// "1h5m0s", "3m0s", "0s".
pub fn round_min(d: Ms) -> String {
    let m = (d + 30_000).div_euclid(60_000).max(0);
    match (m / 60, m % 60) {
        (0, 0) => "0s".into(),
        (0, mi) => format!("{mi}m0s"),
        (h, mi) => format!("{h}h{mi}m0s"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rounds_as_go() {
        for (d, want) in [
            (0, "0s"),
            (29_000, "0s"),
            (90_000, "2m0s"),
            (3_900_000, "1h5m0s"),
        ] {
            assert_eq!(super::round_min(d), want, "{d}");
        }
    }
}
