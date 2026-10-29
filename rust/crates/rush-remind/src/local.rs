//! Times in the machine's own zone, as Go's time.Local reads and writes
//! them: a date and clock made into an instant (time.Date), and an
//! instant written as its date and clock (Format).

use rush_state::Time;

/// An instant's date and clock in the local zone.
pub(crate) struct Civil {
    pub year: i64,
    pub month: i64, // 1-12
    pub day: i64,
    pub hour: i64,
    pub min: i64,
    pub wday: usize, // 0 is Sunday
    pub zone: String,
    pub offset: i32,
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn secs(t: Time) -> i64 {
    t.unix_nanos().div_euclid(1_000_000_000) as i64
}

/// t's date and clock here.
pub(crate) fn civil(t: Time) -> Civil {
    let at = secs(t) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes only tm, which lives on this stack.
    unsafe { libc::localtime_r(&at, &mut tm) };
    let zone = if tm.tm_zone.is_null() {
        String::new()
    } else {
        // SAFETY: tm_zone names a NUL-terminated string the C library keeps.
        unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }
            .to_string_lossy()
            .into_owned()
    };
    Civil {
        year: tm.tm_year as i64 + 1900,
        month: tm.tm_mon as i64 + 1,
        day: tm.tm_mday as i64,
        hour: tm.tm_hour as i64,
        min: tm.tm_min as i64,
        wday: tm.tm_wday as usize % 7,
        zone,
        offset: tm.tm_gmtoff as i32,
    }
}

/// t written in the local zone, so its own format says local time.
pub(crate) fn here(t: Time) -> Time {
    t.with_offset(civil(t).offset)
}

/// The instant of a date and clock here, as Go's time.Date with
/// time.Local: a day or hour past its end runs into the next.
pub(crate) fn date(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> Time {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (year - 1900) as libc::c_int;
    tm.tm_mon = (month - 1) as libc::c_int;
    tm.tm_mday = day as libc::c_int;
    tm.tm_hour = hour as libc::c_int;
    tm.tm_min = min as libc::c_int;
    tm.tm_sec = sec as libc::c_int;
    tm.tm_isdst = -1;
    // SAFETY: mktime reads and normalises only tm.
    let at = unsafe { libc::mktime(&mut tm) };
    here(Time::from_unix(at as i64, 0))
}

/// Go's time.DateTime: "2006-01-02 15:04:05", in t's own zone.
pub(crate) fn date_time(t: Time) -> String {
    let s = t.format();
    s.get(..19).unwrap_or(&s).replacen('T', " ", 1)
}

/// "Mon 2 Jan 15:04", here.
pub(crate) fn short(t: Time) -> String {
    let c = civil(t);
    format!(
        "{} {} {} {:02}:{:02}",
        DAYS[c.wday],
        c.day,
        MONTHS[(c.month - 1).clamp(0, 11) as usize],
        c.hour,
        c.min
    )
}

/// "Mon 2 Jan 15:04 MST", here.
pub(crate) fn short_zoned(t: Time) -> String {
    format!("{} {}", short(t), civil(t).zone)
}

/// Go's time.RFC3339, here: to the second.
pub(crate) fn rfc3339(t: Time) -> String {
    here(Time::from_unix(secs(t), 0)).format()
}
