//! A saver's own account of what it saved. Port of
//! internal/efficiency/gain.go.

use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::detect::look_path;

/// A day of a saver's own account of what it saved. It's the saver's
/// estimate (rtk counts bytes of shell output over four), shown beside
/// what rush measured, never instead of it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Gain {
    #[serde(rename = "date")]
    pub day: String,
    pub commands: i64,
    #[serde(rename = "saved_tokens")]
    pub saved: i64,
    #[serde(rename = "savings_pct")]
    pub pct: f64,
}

/// rtk's daily record, from `rtk gain -d -f json`; none when rtk isn't
/// installed.
pub fn rtk_gains() -> Result<Vec<Gain>, String> {
    let bin = look_path("rtk");
    if bin.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new(bin)
        .args(["gain", "-d", "-f", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Read as it's written, so a long record can't fill the pipe and stall
    // it while it's waited for.
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut b);
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed().as_secs() >= 10 => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("rtk gain didn't answer in 10s".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(status.to_string());
    }
    parse(&out)
}

fn parse(b: &[u8]) -> Result<Vec<Gain>, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        daily: Vec<Gain>,
    }
    serde_json::from_slice::<R>(b)
        .map(|r| r.daily)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_rtk() {
        let g = super::parse(
            br#"{"daily":[{"date":"2026-09-20","commands":12,"saved_tokens":3400,"savings_pct":61.5}]}"#,
        )
        .unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(
            (g[0].day.as_str(), g[0].saved, g[0].pct),
            ("2026-09-20", 3400, 61.5)
        );
    }
}
