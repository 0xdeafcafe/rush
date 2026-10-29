//! Summing a conversation up on a local model: which models can, and
//! summing up long input part by part so no middle of it is dropped.
//!
//! Ported from Go's internal/adapters/ollama/summary.go:1-148.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use rush_core::adapter::Result;

use crate::api::Model;
use crate::{Local, err};

/// How long a summary is given, each part.
const SUMMARY_FOR: Duration = Duration::from_secs(10 * 60);

/// Why m can't sum a conversation up; "" when it can.
fn summary_unsupported(m: &Model) -> &'static str {
    if m.can("decision") {
        return "a decision classifier; it selects an option, not a conversation summary";
    }
    if !m.can("completion") {
        return "not a text-completion model";
    }
    ""
}

impl Local {
    /// The installed models that can sum up: not classifiers, even when
    /// their base architecture advertises completion (a decision letter
    /// can't keep a conversation).
    pub fn summary_models(&self) -> Result<Vec<String>> {
        Ok(self
            .installed_models(None)?
            .into_iter()
            .filter(|m| summary_unsupported(m).is_empty())
            .map(|m| m.name)
            .collect())
    }

    /// Why each local model that can't sum up can't, without stopping
    /// callers such as automatic titling from using those that can; no I/O.
    pub fn summary_exclusions(&self) -> Vec<String> {
        self.cached_installed_models()
            .iter()
            .filter_map(|m| {
                let why = summary_unsupported(m);
                (!why.is_empty()).then(|| format!("{}: {why}", m.name))
            })
            .collect()
    }

    /// Sums text up with model, in a bounded context: long input has every
    /// part summed up before the results are combined. No middle of the
    /// conversation is silently dropped to fit the local model.
    pub fn summarize(&self, model: &str, system: &str, text: &str) -> Result<String> {
        self.ensure_running()?;
        let m = self.show(model, Some(SUMMARY_FOR))?;
        let why = summary_unsupported(&m);
        if !why.is_empty() {
            return Err(err(format!("{model} is {why}")));
        }
        let mut n = self.context_pref(model);
        if n == 0 {
            n = 32768;
        }
        if m.max_context > 0 {
            n = n.min(m.max_context);
        }
        let output = 4096.min(n / 4);
        // A byte can need its own token: reserve by bytes rather than a
        // prose-only characters-a-token guess that can cut code or CJK.
        let budget = n - output - system.len() as i64 - 1024;
        if budget < 1024 {
            return Err(err("context is too small to summarize safely"));
        }
        let budget = budget as usize;
        let mut text = text.to_string();
        while text.len() > budget {
            let parts = summary_chunks(&text, budget);
            let mut merged = String::new();
            let sys = format!(
                "{system}\nThis is one ordered part of a larger input. Preserve facts, constraints, and unfinished work from this part for a later combined summary."
            );
            for (i, part) in parts.iter().enumerate() {
                let summary = self.summarize_part(model, &sys, part, n, output)?;
                merged.push_str(&format!(
                    "\nPart {} of {}:\n{summary}\n",
                    i + 1,
                    parts.len()
                ));
            }
            if merged.len() >= text.len() {
                return Err(err(
                    "local model did not reduce the input; conversation left unchanged",
                ));
            }
            text = merged;
        }
        self.summarize_part(model, system, &text, n, output)
    }

    pub(crate) fn summarize_part(
        &self,
        model: &str,
        system: &str,
        text: &str,
        window: i64,
        output: i64,
    ) -> Result<String> {
        let req = json!({
            "model": model, "stream": false, "think": false,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": text}],
            "options": {"num_ctx": window, "num_predict": output, "temperature": 0},
        });
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Resp {
            message: Msg,
            done_reason: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Msg {
            content: String,
        }
        let r: Resp = self.call("POST", "/api/chat", Some(&req), Some(SUMMARY_FOR))?;
        if r.done_reason == "length" {
            return Err(err(
                "local summary reached its output limit; conversation left unchanged",
            ));
        }
        let mut out = r.message.content.trim();
        if let Some((_, after)) = out.split_once("</think>") {
            out = after.trim();
        }
        if out.is_empty() {
            return Err(err(format!("{model} wrote nothing")));
        }
        Ok(out.to_string())
    }
}

/// text in parts of at most most bytes, each cut at a character's edge,
/// at a line's end where one is in its second half.
pub fn summary_chunks(text: &str, most: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut text = text;
    while text.len() > most {
        let mut cut = most;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(at) = text[..cut].rfind('\n')
            && at > cut / 2
        {
            cut = at + 1;
        }
        out.push(text[..cut].to_string());
        text = &text[cut..];
    }
    if !text.is_empty() {
        out.push(text.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::serve;
    use rush_core::adapter::Registry;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn local(url: &str) -> (Arc<Local>, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(Registry::new(), Some(url), Some(d.path().into()), None);
        (l, d)
    }

    #[test]
    fn summary_models_leave_out_classifiers_even_with_completion() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let url = serve(move |r| {
            let out = match r.path.as_str() {
                "/api/version" => "{}",
                "/api/tags" => {
                    r#"{"models":[{"name":"tev1:4b"},{"name":"writer"},{"name":"embed"}]}"#
                }
                "/api/ps" => r#"{"models":[]}"#,
                "/api/show" => match r.json()["model"].as_str() {
                    Some("tev1:4b") => r#"{"capabilities":["decision","tools","completion"]}"#,
                    Some("writer") => r#"{"capabilities":["completion"]}"#,
                    _ => r#"{"capabilities":["embedding"]}"#,
                },
                "/api/chat" => {
                    c.fetch_add(1, Ordering::SeqCst);
                    r#"{"message":{"content":"A"}}"#
                }
                _ => "{}",
            };
            (200, out.to_string())
        });
        let (l, _d) = local(&url);
        assert_eq!(l.summary_models().unwrap(), ["writer"]);
        let why = l.summary_exclusions().join(" ");
        assert!(
            why.contains("tev1:4b") && why.contains("decision classifier"),
            "{why}"
        );
        let e = l.summarize("tev1:4b", "summarize", "work").unwrap_err();
        assert!(e.to_string().contains("decision classifier"), "{e}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "classifier invoked as summarizer"
        );
    }

    #[test]
    fn chunks_keep_every_byte_and_character() {
        let input = format!("{}last task", "重要な修正\n".repeat(120));
        let chunks = summary_chunks(&input, 43);
        assert_eq!(chunks.concat(), input, "input was lost");
        for c in &chunks {
            assert!(c.len() <= 43, "bad chunk {c:?}");
        }
    }

    #[test]
    fn summarize_bounds_the_context_and_keeps_the_middle() {
        let original = format!(
            "{}IMPORTANT MIDDLE TASK{}",
            "first work. ".repeat(2500),
            "last work. ".repeat(2500)
        );
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = sent.clone();
        let url = serve(move |r| {
            let out = match r.path.as_str() {
                "/api/version" => "{}",
                "/api/show" => {
                    r#"{"capabilities":["completion"],"model_info":{"test.context_length":262144}}"#
                }
                "/api/chat" => {
                    let v = r.json();
                    assert_eq!(v["options"]["num_ctx"], 32768);
                    assert_eq!(v["options"]["num_predict"], 4096);
                    assert_eq!(v["think"], false);
                    s.lock()
                        .unwrap()
                        .push(v["messages"][1]["content"].as_str().unwrap().into());
                    r#"{"message":{"content":"Preserved the important pending work, all decisions and constraints."},"done_reason":"stop"}"#
                }
                _ => "{}",
            };
            (200, out.to_string())
        });
        let (l, _d) = local(&url);
        let out = l.summarize("writer", "summarize", &original).unwrap();
        assert!(!out.is_empty());
        let sent = sent.lock().unwrap();
        assert!(
            sent.len() >= 3,
            "expected chunks and a combined summary, got {}",
            sent.len()
        );
        assert_eq!(
            sent[..sent.len() - 1].concat(),
            original,
            "not all the input reached the model"
        );
    }

    #[test]
    fn an_output_limit_leaves_the_conversation_unchanged() {
        let url = serve(|_| {
            (
                200,
                r#"{"message":{"content":"partial summary"},"done_reason":"length"}"#.into(),
            )
        });
        let (l, _d) = local(&url);
        assert!(
            l.summarize_part("writer", "summary", "work", 8192, 2048)
                .is_err()
        );
    }
}
