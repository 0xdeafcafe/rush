//! Ollama's API: where it listens, what it says of its models, loading
//! one, what a model reads, and the models offered to choose from.
//!
//! Ported from Go's internal/adapters/ollama/api.go:1-257.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use rush_core::adapter::{Choice, Media, Result};

use crate::{Local, err, lock, tune};

/// What Ollama says of one of its models: /api/show, and /api/ps when it's
/// loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    pub name: String,
    pub capabilities: Vec<String>,
    pub family: String,
    /// "31.1B"
    pub params: String,
    /// "Q4_K_M"
    pub quant: String,
    /// What the model was trained to.
    pub max_context: i64,
    /// What the server loaded it with; 0 when not loaded.
    pub context: i64,
    pub vram: i64,
}

impl Model {
    /// A model of this name, with these capabilities.
    pub fn named(name: &str, caps: &[&str]) -> Model {
        Model {
            name: name.into(),
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            ..Model::default()
        }
    }

    /// Whether the model has capability c: tools, thinking, vision.
    pub fn can(&self, c: &str) -> bool {
        self.capabilities.iter().any(|x| x == c)
    }

    /// Excludes decision classifiers whose base model may still advertise
    /// tools, although the fine-tune only returns choice letters.
    pub fn can_code(&self) -> bool {
        self.can("tools") && !self.can("decision")
    }
}

/// Where Ollama listens, given OLLAMA_HOST as Ollama itself reads it, else
/// its own default.
pub fn server_of(host: &str) -> String {
    let mut h = host.trim().to_string();
    if h.is_empty() {
        return "http://127.0.0.1:11434".into();
    }
    if !h.contains("://") {
        h = format!("http://{h}");
    }
    if let Some(rest) = h.strip_prefix("http://0.0.0.0") {
        h = format!("http://127.0.0.1{rest}");
    }
    if h.matches(':').count() < 2 {
        // no port
        h.push_str(":11434");
    }
    h.trim_end_matches('/').to_string()
}

/// How a request went wrong, before Ollama answered or as it answered.
pub(crate) enum Fail {
    /// It took longer than it was given.
    Slow,
    /// Nothing answered.
    Down,
}

impl Local {
    /// Where this server listens.
    pub fn server(&self) -> String {
        match &self.host {
            Some(h) => server_of(h),
            None => server_of(&std::env::var("OLLAMA_HOST").unwrap_or_default()),
        }
    }

    pub(crate) fn not_running(&self) -> String {
        format!(
            "Ollama isn't running at {}: start it with `ollama serve`, or open the Ollama app",
            self.server()
        )
    }

    /// method path with body as JSON, taking at most timeout (None: as
    /// long as it takes; loading a model does).
    pub(crate) fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> std::result::Result<ureq::http::Response<ureq::Body>, Fail> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(timeout)
            .http_status_as_error(false)
            .build()
            .into();
        let url = format!("{}{path}", self.server());
        let b = body.map(|v| serde_json::to_vec(v).unwrap_or_default());
        let r = match (method, b) {
            ("GET", _) => agent.get(&url).call(),
            ("DELETE", Some(b)) => agent
                .delete(&url)
                .header("Content-Type", "application/json")
                .force_send_body()
                .send(&b[..]),
            ("DELETE", None) => agent.delete(&url).call(),
            (_, b) => agent
                .post(&url)
                .header("Content-Type", "application/json")
                .send(&b.unwrap_or_default()[..]),
        };
        match r {
            Ok(res) => Ok(res),
            Err(ureq::Error::Timeout(_)) => Err(Fail::Slow),
            Err(_) => Err(Fail::Down),
        }
    }

    /// Asks Ollama, and reads its answer; an error says what Ollama said,
    /// or that it isn't there.
    pub(crate) fn call_raw(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<Vec<u8>> {
        let mut res = match self.send(method, path, body, timeout) {
            Ok(r) => r,
            Err(Fail::Slow) => {
                return Err(err(format!(
                    "ollama: {path} took too long: context deadline exceeded"
                )));
            }
            Err(Fail::Down) => return Err(err(self.not_running())),
        };
        let status = res.status();
        let b = res
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::Timeout(_) => err(format!(
                    "ollama: {path} took too long: context deadline exceeded"
                )),
                e => err(e.to_string()),
            })?;
        if status.as_u16() != 200 {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct E {
                error: String,
            }
            let mut e: E = serde_json::from_slice(&b).unwrap_or_default();
            if e.error.is_empty() {
                e.error = match status.canonical_reason() {
                    Some(r) => format!("{} {r}", status.as_u16()),
                    None => status.as_u16().to_string(),
                };
            }
            return Err(err(format!("ollama: {}", e.error)));
        }
        Ok(b)
    }

    /// call_raw, its answer read as T.
    pub(crate) fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let b = self.call_raw(method, path, body, timeout)?;
        Ok(serde_json::from_slice(&b)?)
    }

    /// The models the server has.
    pub(crate) fn names(&self, timeout: Option<Duration>) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Tags {
            #[serde(default)]
            models: Vec<Named>,
        }
        #[derive(Deserialize)]
        struct Named {
            #[serde(default)]
            name: String,
        }
        let t: Tags = self.call("GET", "/api/tags", None, timeout)?;
        Ok(t.models.into_iter().map(|m| m.name).collect())
    }

    /// The models in memory now, with the context each was loaded with.
    pub(crate) fn loaded(&self, timeout: Option<Duration>) -> Result<HashMap<String, Model>> {
        #[derive(Deserialize)]
        struct Ps {
            #[serde(default)]
            models: Vec<P>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P {
            name: String,
            size_vram: i64,
            context_length: i64,
        }
        let ps: Ps = self.call("GET", "/api/ps", None, timeout)?;
        Ok(ps
            .models
            .into_iter()
            .map(|m| {
                let model = Model {
                    name: m.name.clone(),
                    context: m.context_length,
                    vram: m.size_vram,
                    ..Model::default()
                };
                (m.name, model)
            })
            .collect())
    }

    /// What the server knows of model name.
    pub(crate) fn show(&self, name: &str, timeout: Option<Duration>) -> Result<Model> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Show {
            capabilities: Vec<String>,
            details: Details,
            model_info: HashMap<String, Value>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Details {
            family: String,
            parameter_size: String,
            quantization_level: String,
        }
        let s: Show = self.call("POST", "/api/show", Some(&json!({"model": name})), timeout)?;
        let mut m = Model {
            name: name.into(),
            capabilities: s.capabilities,
            family: s.details.family,
            params: s.details.parameter_size,
            quant: s.details.quantization_level,
            ..Model::default()
        };
        lock(&self.shown).insert(name.into(), m.can("vision"));
        for (k, v) in &s.model_info {
            if k.ends_with(".context_length")
                && let Some(f) = v.as_f64()
            {
                m.max_context = f as i64;
            }
        }
        Ok(m)
    }

    /// Puts model name in memory, so the first turn doesn't wait on it and
    /// the context it's loaded with can be read back.
    pub(crate) fn load(&self, name: &str, timeout: Option<Duration>) -> Result<()> {
        self.call_raw(
            "POST",
            "/api/generate",
            Some(&json!({"model": name, "stream": false})),
            timeout,
        )?;
        Ok(())
    }

    /// What model reads, as /api/show says: images when it has vision, and
    /// nothing else. A model not asked of yet is asked of in the
    /// background, and not known until the answer comes, so nothing waits
    /// on the network.
    // ponytail: the first send to a model not yet shown in this process
    // goes unchecked; ask when a session's model is first seen if that
    // matters.
    pub fn reads(self: &Arc<Self>, model: &str) -> Option<Media> {
        let model = model
            .strip_prefix(&format!("{}/", crate::pi::PI_PROVIDER))
            .unwrap_or(model);
        if model.is_empty() {
            return None;
        }
        if let Some(&v) = lock(&self.shown).get(model) {
            return Some(if v { Media::IMAGE } else { Media::default() });
        }
        if lock(&self.asking).insert(model.to_string()) {
            let me = self.clone();
            let model = model.to_string();
            std::thread::spawn(move || {
                let _ = me.show(&model, Some(Duration::from_secs(10)));
                lock(&me.asking).remove(&model);
            });
        }
        None
    }

    /// The models this machine's Ollama has that can call tools, the
    /// likeliest first, each with its size and whether it's loaded. It
    /// asks the server, so never on the UI; it's kept for a minute.
    pub fn list_models(&self) -> Vec<Choice> {
        let mut listed = lock(&self.listed);
        if let Some(at) = listed.0
            && at.elapsed() < Duration::from_secs(60)
        {
            return listed.1.clone();
        }
        let Ok(mut models) = self.installed_models(Some(Duration::from_secs(30))) else {
            return Vec::new();
        };
        models.retain(Model::can_code);
        models.sort_by_key(|m| std::cmp::Reverse(tune::rank(m)));
        let out: Vec<Choice> = models
            .iter()
            .map(|m| {
                let mut note = [&m.family, &m.params, &m.quant]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if m.can("vision") {
                    note.push_str(", reads images");
                }
                if m.vram > 0 {
                    note.push_str(", loaded");
                }
                Choice {
                    id: m.name.clone(),
                    note,
                    context: tune::window(m),
                }
            })
            .collect();
        *listed = (Some(Instant::now()), out.clone());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::Registry;

    #[test]
    fn server() {
        for (input, want) in [
            ("", "http://127.0.0.1:11434"),
            ("0.0.0.0", "http://127.0.0.1:11434"),
            ("0.0.0.0:9000", "http://127.0.0.1:9000"),
            ("https://gpu.box:80/", "https://gpu.box:80"),
            ("box", "http://box:11434"),
        ] {
            assert_eq!(server_of(input), want, "OLLAMA_HOST={input:?}");
        }
    }

    // What a model reads is what /api/show said of it, under Pi's name for
    // it too; one not shown yet isn't known.
    #[test]
    fn reads() {
        let l = Local::with(Registry::new(), Some("http://127.0.0.1:1"), None, None);
        lock(&l.shown).insert("seer:4b".into(), true);
        lock(&l.shown).insert("blind:4b".into(), false);
        lock(&l.asking).insert("unasked:4b".into()); // as if a show were on its way
        for (model, want) in [
            ("seer:4b", Media::IMAGE),
            ("ollama/seer:4b", Media::IMAGE),
            ("blind:4b", Media::default()),
        ] {
            assert_eq!(l.reads(model), Some(want), "{model}");
        }
        assert_eq!(
            l.reads("unasked:4b"),
            None,
            "a model not shown yet is known"
        );
    }
}
