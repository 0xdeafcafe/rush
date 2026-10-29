//! Claude Code's own record of an account's use over time, as it keeps it
//! for /stats in stats-cache.json. Port of
//! internal/adapters/claude/claude/stats.go.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::time::Duration;

use rush_core::adapter::{ModelTotal, Stats, StatsDay};
use rush_core::time::{Ms, parse_rfc3339};
use serde::Deserialize;

use crate::account::Account;
use crate::json::nd;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    #[serde(rename = "lastComputedDate", deserialize_with = "nd")]
    last_computed_date: String,
    #[serde(rename = "dailyActivity", deserialize_with = "nd")]
    daily_activity: Vec<Activity>,
    #[serde(rename = "dailyModelTokens", deserialize_with = "nd")]
    daily_model_tokens: Vec<DayTokens>,
    #[serde(rename = "modelUsage", deserialize_with = "nd")]
    model_usage: HashMap<String, ModelUse>,
    #[serde(rename = "totalSessions", deserialize_with = "nd")]
    total_sessions: i64,
    #[serde(rename = "totalMessages", deserialize_with = "nd")]
    total_messages: i64,
    #[serde(rename = "firstSessionDate", deserialize_with = "nd")]
    first_session: String,
    #[serde(rename = "longestSession", deserialize_with = "nd")]
    longest: Longest,
    #[serde(rename = "hourCounts", deserialize_with = "nd")]
    hour_counts: HashMap<String, i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Activity {
    #[serde(deserialize_with = "nd")]
    date: String,
    #[serde(rename = "messageCount", deserialize_with = "nd")]
    messages: i64,
    #[serde(rename = "sessionCount", deserialize_with = "nd")]
    sessions: i64,
    #[serde(rename = "toolCallCount", deserialize_with = "nd")]
    tool_calls: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DayTokens {
    #[serde(deserialize_with = "nd")]
    date: String,
    #[serde(rename = "tokensByModel", deserialize_with = "nd")]
    tokens: BTreeMap<String, i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ModelUse {
    #[serde(rename = "inputTokens", deserialize_with = "nd")]
    input: i64,
    #[serde(rename = "outputTokens", deserialize_with = "nd")]
    output: i64,
    #[serde(rename = "cacheReadInputTokens", deserialize_with = "nd")]
    cache_read: i64,
    #[serde(rename = "cacheCreationInputTokens", deserialize_with = "nd")]
    cache_write: i64,
    #[serde(rename = "costUSD", deserialize_with = "nd")]
    cost_usd: f64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Longest {
    #[serde(rename = "sessionId", deserialize_with = "nd")]
    session_id: String,
    #[serde(deserialize_with = "nd")]
    timestamp: String,
    #[serde(deserialize_with = "nd")]
    duration: i64,
    #[serde(rename = "messageCount", deserialize_with = "nd")]
    messages: i64,
}

/// Reads the account's stats-cache.json.
pub fn load_stats(acct: &Account) -> io::Result<Stats> {
    let b = std::fs::read(acct.config_dir.join("stats-cache.json"))?;
    parse_stats(&b)
}

/// Reads a stats-cache.json already read.
pub fn parse_stats(b: &[u8]) -> io::Result<Stats> {
    let raw: Raw = serde_json::from_slice(b).map_err(io::Error::other)?;
    let mut s = Stats {
        computed: raw.last_computed_date,
        sessions: raw.total_sessions,
        messages: raw.total_messages,
        longest: Duration::from_millis(raw.longest.duration.max(0) as u64),
        longest_msgs: raw.longest.messages,
        longest_id: raw.longest.session_id,
        first: parse_rfc3339(&raw.first_session),
        longest_at: parse_rfc3339(&raw.longest.timestamp),
        ..Stats::default()
    };
    let mut days: BTreeMap<String, StatsDay> = BTreeMap::new();
    fn day<'a>(days: &'a mut BTreeMap<String, StatsDay>, d: &str) -> &'a mut StatsDay {
        days.entry(d.to_string()).or_insert_with(|| StatsDay {
            date: local_day(d),
            ..StatsDay::default()
        })
    }
    for a in &raw.daily_activity {
        let d = day(&mut days, &a.date);
        d.messages = a.messages;
        d.sessions = a.sessions;
        d.tool_calls = a.tool_calls;
    }
    for t in raw.daily_model_tokens {
        day(&mut days, &t.date).tokens = t.tokens;
    }
    s.days = days.into_values().collect();
    s.days.sort_by_key(|d| d.date);
    for (name, u) in raw.model_usage {
        s.models.push(ModelTotal {
            model: name,
            input: u.input,
            output: u.output,
            cache_read: u.cache_read,
            cache_write: u.cache_write,
            cost_usd: u.cost_usd,
        });
    }
    s.models
        .sort_by(|a, b| b.total().cmp(&a.total()).then(a.model.cmp(&b.model)));
    for (h, n) in raw.hour_counts {
        if let Ok(i) = h.parse::<usize>()
            && i < 24
        {
            s.hours[i] = n;
        }
    }
    Ok(s)
}

/// Local midnight of a 2006-01-02 date, as Go's ParseInLocation in Local;
/// 0 when it isn't one.
pub(crate) fn local_day(d: &str) -> Ms {
    let mut it = d.splitn(3, '-').map(|p| p.parse::<i32>().ok());
    let (Some(Some(y)), Some(Some(m)), Some(Some(dd))) = (it.next(), it.next(), it.next()) else {
        return 0;
    };
    if d.len() != 10 {
        return 0;
    }
    // SAFETY: mktime only reads and normalises the tm we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = y - 1900;
    tm.tm_mon = m - 1;
    tm.tm_mday = dd;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    if t == -1 { 0 } else { t as Ms * 1000 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_cache() {
        let b = br#"{"lastComputedDate":"2026-10-02","dailyActivity":[{"date":"2026-10-02","messageCount":5,"sessionCount":1,"toolCallCount":3},{"date":"2026-10-01","messageCount":2,"sessionCount":1,"toolCallCount":0}],
"dailyModelTokens":[{"date":"2026-10-02","tokensByModel":{"claude-opus-5-5":100,"claude-haiku-4-5":20}}],
"modelUsage":{"claude-haiku-4-5":{"inputTokens":1,"outputTokens":1},"claude-opus-5-5":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":30,"cacheCreationInputTokens":40,"costUSD":1.5}},
"totalSessions":2,"totalMessages":7,"firstSessionDate":"2026-10-01T09:00:00.000Z",
"longestSession":{"sessionId":"s1","timestamp":"2026-10-02T10:00:00Z","duration":60000,"messageCount":5},"hourCounts":{"9":1,"10":1,"24":3}}"#;
        let s = parse_stats(b).unwrap();
        assert_eq!(s.computed, "2026-10-02");
        assert_eq!((s.sessions, s.messages), (2, 7));
        assert_eq!(s.days.len(), 2);
        assert!(s.days[0].date < s.days[1].date, "oldest first");
        assert_eq!(s.days[1].total_tokens(), 120);
        assert_eq!(s.days[1].tool_calls, 3);
        assert_eq!(s.models[0].model, "claude-opus-5-5");
        assert_eq!(s.models[0].total(), 100);
        assert_eq!(s.longest, Duration::from_secs(60));
        assert_eq!(s.longest_id, "s1");
        assert_eq!(s.hours[9], 1);
        assert_eq!(s.hours.iter().sum::<i64>(), 2, "an hour past 23 is left out");
        assert_eq!(s.first, parse_rfc3339("2026-10-01T09:00:00Z"));
    }
}
