//! Decision models (tev1, nimble) answer typed questions over
//! /v1/systemone with a probability instead of writing text. rush asks
//! them which tool steps a conversation still needs, and keeps those word
//! for word: compaction by deleting, not summarising. And pulling and
//! removing models.
//!
//! Ported from Go's internal/adapters/ollama/decide.go:1-192.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use rush_core::adapter::Result;

use crate::api::Fail;
use crate::{Local, err, lock};

// tev1 reads at most 2050 tokens a prompt, and a request's every question
// is read with the state and all the others: so one step a request, the
// work in short as its state, a few at once.
// shortcut: bytes stand in for tokens at 2 a token, the worst code and
// JSON come to; read the limit off Ollama's error if a model's is smaller.
const DECIDE_ITEM: usize = 1200;
const DECIDE_GOAL: usize = 2000;
/// About 3× one at a time on an M-series Mac.
pub const DECIDE_PARALLEL: usize = 4;

/// How long one decision is given.
const DECIDE_FOR: Duration = Duration::from_secs(5 * 60);

impl Local {
    /// The installed models that answer decisions.
    pub fn decision_models(&self) -> Result<Vec<String>> {
        Ok(self
            .installed_models(None)?
            .into_iter()
            .filter(|m| m.can("decision"))
            .map(|m| m.name)
            .collect())
    }

    /// The first decision model the last discovery found, or ""; no I/O.
    pub fn cached_decision_model(&self) -> String {
        self.cached_installed_models()
            .iter()
            .find(|m| m.can("decision"))
            .map(|m| m.name.clone())
            .unwrap_or_default()
    }

    /// Scores each item for whether the work in goal still needs it: the
    /// probability, 0 to 1, that it does, in the order given. A few are
    /// asked at once; the first to fail stops the rest.
    pub fn keep(&self, model: &str, goal: &str, items: &[String]) -> Result<Vec<f64>> {
        self.ensure_running()?;
        let state = format!("Current work:\n{}", clip_bytes(goal, DECIDE_GOAL));
        let out = Mutex::new(vec![0.0; items.len()]);
        let failed: Mutex<Option<String>> = Mutex::new(None);
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..DECIDE_PARALLEL.min(items.len()) {
                s.spawn(|| {
                    loop {
                        if lock(&failed).is_some() {
                            return;
                        }
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(it) = items.get(i) else { return };
                        match self.decide(model, &state, it, i) {
                            Ok(p) => lock(&out)[i] = p,
                            Err(e) => {
                                lock(&failed).get_or_insert(e.to_string());
                                return;
                            }
                        }
                    }
                });
            }
        });
        if let Some(e) = failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
            return Err(err(e));
        }
        Ok(out.into_inner().unwrap_or_else(|e| e.into_inner()))
    }

    /// Asks model whether the work still needs step i, item.
    fn decide(&self, model: &str, state: &str, item: &str, i: usize) -> Result<f64> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Resp {
            answers: HashMap<String, Answer>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            noul: Option<f64>,
        }
        let req = json!({"model": model, "state": state, "questions": {"keep": {
            "type": "noul",
            "instructions": format!("An earlier tool step:\n{}\n\nDoes the current work still need this step's exact input or output to carry on?", clip_bytes(item, DECIDE_ITEM)),
            "criteria": {
                "false": "Finished, superseded, or easily redone: it can be dropped.",
                "true": "Holds a path, error, value or decision the work still relies on.",
            },
        }}});
        let r: Resp = self.call("POST", "/v1/systemone", Some(&req), Some(DECIDE_FOR))?;
        r.answers
            .get("keep")
            .and_then(|a| a.noul)
            .ok_or_else(|| err(format!("{model} didn't answer for step {}", i + 1)))
    }

    /// Downloads name, telling progress the bytes done of the total as they
    /// come. A model pulled again is mended: its missing files fetched.
    pub fn pull(&self, name: &str, mut progress: Option<&mut dyn FnMut(i64, i64)>) -> Result<()> {
        self.ensure_running()?;
        let res = match self.send(
            "POST",
            "/api/pull",
            Some(&json!({"model": name, "stream": true})),
            None,
        ) {
            Ok(r) => r,
            Err(Fail::Slow | Fail::Down) => return Err(err(self.not_running())),
        };
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Line {
            status: String,
            error: String,
            total: i64,
            completed: i64,
        }
        let mut r = BufReader::new(res.into_body().into_reader());
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if r.by_ref().take(1 << 20).read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            let Ok(line) = serde_json::from_slice::<Line>(buf.trim_ascii()) else {
                continue;
            };
            if !line.error.is_empty() {
                return Err(err(format!("ollama: {}", line.error)));
            }
            if let Some(p) = progress.as_mut()
                && line.total > 0
            {
                p(line.completed, line.total);
            }
            if line.status == "success" {
                lock(&self.broken).remove(name);
                return Ok(());
            }
        }
        Err(err("ollama: pull ended before it finished"))
    }

    /// Deletes model name from Ollama.
    pub fn remove(&self, name: &str) -> Result<()> {
        self.call_raw(
            "DELETE",
            "/api/delete",
            Some(&json!({"model": name})),
            Some(Duration::from_secs(60)),
        )?;
        lock(&self.broken).remove(name);
        let mut c = lock(&self.catalog);
        if let Some(all) = c.as_ref() {
            let next: Vec<_> = all.iter().filter(|m| m.name != name).cloned().collect();
            *c = Some(next.into());
        }
        Ok(())
    }

    /// The installed models the last discovery couldn't read, each with
    /// why; no I/O.
    pub fn broken(&self) -> HashMap<String, String> {
        lock(&self.broken).clone()
    }
}

/// s cut to at most n bytes, at a character's edge, marked as cut.
pub fn clip_bytes(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = n;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} …", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::serve;
    use rush_core::adapter::Registry;
    use std::sync::{Arc, Condvar};

    fn local(url: &str) -> (std::sync::Arc<Local>, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(
            Registry::new(),
            Some(url),
            Some(d.path().to_path_buf()),
            None,
        );
        (l, d)
    }

    #[test]
    fn keep_asks_each_step_alone_in_order() {
        // busy now, most at once; a request waits (at most a while) for a
        // second to be in flight, so asking one at a time shows.
        let gate = Arc::new((Mutex::new((0usize, 0usize)), Condvar::new()));
        let g = gate.clone();
        let url = serve(move |r| {
            if r.path != "/v1/systemone" {
                return (200, "{}".into());
            }
            let v = r.json();
            let qs = v["questions"].as_object().unwrap();
            let ins = qs["keep"]["instructions"].as_str().unwrap();
            // tev1 reads the state and every question as one prompt of at
            // most 2050 tokens; at 2 bytes a token, one step's must fit.
            let state = v["state"].as_str().unwrap();
            assert!(
                qs.len() == 1 && state.len() + ins.len() <= 4000,
                "{} bytes",
                state.len() + ins.len()
            );
            let step = ins.strip_prefix("An earlier tool step:\n").unwrap();
            let id: i64 = step.split_whitespace().next().unwrap().parse().unwrap();
            if id == 999 {
                return (
                    400,
                    r#"{"error":"prompt 0 has 2400 tokens; expected 1–2050"}"#.into(),
                );
            }
            let (m, cv) = &*g;
            let mut b = lock(m);
            b.0 += 1;
            b.1 = b.1.max(b.0);
            cv.notify_all();
            let (mut b, _) = cv
                .wait_timeout_while(b, Duration::from_secs(5), |b| b.1 < 2)
                .unwrap();
            b.0 -= 1;
            (
                200,
                format!(
                    r#"{{"answers":{{"keep":{{"noul":{}}}}}}}"#,
                    id as f64 / 1000.0
                ),
            )
        });
        let (l, _d) = local(&url);
        let mut items: Vec<String> = (0..40)
            .map(|i| format!("{i} {}", "\"<\n".repeat(3000)))
            .collect();
        let got = l
            .keep("tev1", &"fix the build ".repeat(1000), &items)
            .unwrap();
        for (i, p) in got.iter().enumerate() {
            assert_eq!(*p, i as f64 / 1000.0, "score {i}: answers out of order");
        }
        let most = lock(&gate.0).1;
        assert!((2..=DECIDE_PARALLEL).contains(&most), "{most} at once");
        items[7] = "999 too long".into();
        let e = l.keep("tev1", "x", &items).unwrap_err();
        assert!(e.to_string().contains("2050"), "{e}");
    }

    #[test]
    fn pull_reports_progress_and_ollama_errors() {
        let url = serve(|r| {
            match (r.path.as_str(), r.json()["model"].as_str()) {
            ("/api/pull", Some("nope")) => (
                200,
                "{\"status\":\"pulling\",\"total\":100,\"completed\":40}\n{\"error\":\"pull model manifest: file does not exist\"}\n".into(),
            ),
            ("/api/pull", _) => (
                200,
                "{\"status\":\"pulling\",\"total\":100,\"completed\":40}\n{\"status\":\"success\"}\n".into(),
            ),
            _ => (200, "{}".into()),
        }
        });
        let (l, _d) = local(&url);
        lock(&l.broken).insert("tev1".into(), "missing blob".into());
        let mut done = 0;
        l.pull("tev1", Some(&mut |d, _| done = d)).unwrap();
        assert_eq!(done, 40);
        assert!(
            !l.broken().contains_key("tev1"),
            "a pulled model is still broken"
        );
        let e = l.pull("nope", None).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
    }

    // A model removed is gone from what was found, and mended of nothing.
    #[test]
    fn remove_forgets_a_model() {
        let url = serve(|r| {
            assert_eq!(
                (r.method.as_str(), r.path.as_str()),
                ("DELETE", "/api/delete")
            );
            assert_eq!(r.json()["model"], "old");
            (200, String::new())
        });
        let (l, _d) = local(&url);
        *lock(&l.catalog) = Some(
            vec![
                crate::Model::named("old", &[]),
                crate::Model::named("new", &[]),
            ]
            .into(),
        );
        lock(&l.broken).insert("old".into(), "missing blob".into());
        l.remove("old").unwrap();
        assert_eq!(l.cached_installed_models().len(), 1);
        assert!(l.broken().is_empty());
    }

    #[test]
    fn clips_at_a_characters_edge() {
        assert_eq!(clip_bytes("short", 10), "short");
        assert_eq!(clip_bytes("重要", 4), "重 …");
    }
}
