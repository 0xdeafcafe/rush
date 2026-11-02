//! Times as efficiency counts them: Go's time.Time in its files
//! (rush_state::Time), Unix seconds and hours to bucket them, and the
//! local day and the few local formats its words use.

use rush_state::Time;

/// Unix seconds at t, floored; Go's zero time is far in the past.
pub fn secs(t: Time) -> i64 {
    t.unix_nanos().div_euclid(1_000_000_000) as i64
}

/// The Unix hour t falls in (scan.go hourOf).
pub fn hour_of(t: Time) -> i64 {
    secs(t).div_euclid(3600)
}

/// The instant s Unix seconds name.
pub fn at_secs(s: i64) -> Time {
    Time::from_unix(s, 0)
}

/// t moved by s seconds, either way.
pub fn add_secs(t: Time, s: i64) -> Time {
    t.add_nanos(s.saturating_mul(1_000_000_000))
}

/// Seconds from a to b.
pub fn since(b: Time, a: Time) -> i64 {
    (b.sub_nanos(a) / 1_000_000_000) as i64
}

pub const HOUR: i64 = 3600;
pub const DAY: i64 = 24 * HOUR;

fn local_tm(s: i64) -> Option<libc::tm> {
    let t = s as libc::time_t;
    // SAFETY: localtime_r writes into the tm we own.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            None
        } else {
            Some(tm)
        }
    }
}

/// The start of t's local day (time.Date(y, m, d, 0, 0, 0, 0, Local)).
pub fn local_day_start(t: Time) -> Time {
    let Some(mut tm) = local_tm(secs(t)) else {
        return at_secs(secs(t).div_euclid(DAY) * DAY);
    };
    tm.tm_hour = 0;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    // SAFETY: mktime reads and normalises the tm we own.
    let s = unsafe { libc::mktime(&mut tm) };
    at_secs(s as i64)
}

/// The local day after the one starting at t: a calendar day, whatever
/// the clocks did (t.AddDate(0, 0, 1)).
pub fn next_local_day(t: Time) -> Time {
    let Some(mut tm) = local_tm(secs(t)) else {
        return add_secs(t, DAY);
    };
    tm.tm_mday += 1;
    tm.tm_isdst = -1;
    // SAFETY: as above.
    let s = unsafe { libc::mktime(&mut tm) };
    at_secs(s as i64)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// t in local time, in one of Go's layouts this crate uses: "2 Jan",
/// "2 Jan 15:04", "15:04", "2006-01-02", "Mon 2 Jan", "2006-01-02T150405".
pub fn local(t: Time, layout: &str) -> String {
    let Some(tm) = local_tm(secs(t)) else {
        return String::new();
    };
    let (d, mon, y) = (
        tm.tm_mday,
        MONTHS[tm.tm_mon.clamp(0, 11) as usize],
        tm.tm_year + 1900,
    );
    let hm = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    match layout {
        "2 Jan" => format!("{d} {mon}"),
        "2 Jan 15:04" => format!("{d} {mon} {hm}"),
        "15:04" => hm,
        "2006-01-02" => format!("{y:04}-{:02}-{d:02}", tm.tm_mon + 1),
        "Mon 2 Jan" => format!("{} {d} {mon}", DAYS[tm.tm_wday.clamp(0, 6) as usize]),
        "2006-01-02T150405" => format!(
            "{y:04}-{:02}-{d:02}T{:02}{:02}{:02}",
            tm.tm_mon + 1,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        ),
        _ => t.format(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_and_days() {
        let t = Time::parse("2026-09-20T10:30:00Z").unwrap();
        assert_eq!(hour_of(t) * 3600, secs(t) - 1800);
        let d = local_day_start(t);
        assert!(d <= t && since(t, d) < DAY);
        let n = next_local_day(d);
        assert!(since(n, d) >= 23 * HOUR && since(n, d) <= 25 * HOUR);
        assert_eq!(local_day_start(n), n);
        assert!(hour_of(Time::default()) < 0, "Go's zero time is long ago");
    }
}
