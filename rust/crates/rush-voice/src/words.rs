//! Turning what the speech model hears into what the window shows. New in
//! the Rust port (no Go original).

/// Splits what the speech model has heard so far into the words it's sure
/// of and the ones it may still change. Apple's partial results don't say
/// which is which, so all but the last two words count as sure; once the
/// result is the last one (last), every word is.
pub fn split(text: &str, last: bool) -> (String, String) {
    if last {
        return (
            text.split_whitespace().collect::<Vec<_>>().join(" "),
            String::new(),
        );
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let sure = words.len().saturating_sub(2);
    (words[..sure].join(" "), words[sure..].join(" "))
}

/// How loud some sound is, 0 to 100: -55 dB is silent and -15 dB loud
/// (yap's Listener.loudness).
pub fn level(samples: &[f32]) -> i64 {
    if samples.is_empty() {
        return 0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum / samples.len() as f32).sqrt().max(1e-6);
    let db = 20.0 * rms.log10();
    let loud = ((db + 55.0) / 40.0).clamp(0.0, 1.0);
    (loud * 100.0).round() as i64
}
