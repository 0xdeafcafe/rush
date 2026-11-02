//! A view as an agent sends it, read the way it was meant. Agents mostly
//! write views right, but a slip in one field used to throw the whole view
//! away with serde's words ("invalid type: string \"up\", expected a
//! sequence"), which say neither where nor what to write instead. So a
//! number written as a string, a string as a number, one item where a list
//! goes, a word for a tone ("success", "error") or a kind ("stat") is read
//! as meant; what can't be (a trend of "up") is left out and the agent is
//! told; and what is still wrong names the field and shows the shape.
//!
//! New in the Rust port (Go read views with encoding/json alone).

use serde_json::{Map, Value};

use crate::views::View;

type Obj = Map<String, Value>;

/// The shape of a whole view, for an error about one.
pub const VIEW_LIKE: &str = r#"{"title": "Tests", "summary": "412 passed", "stats": [{"value": "412", "label": "passed", "tone": "good"}], "blocks": [{"kind": "list", "items": [{"text": "TestRetry", "meta": "timed out", "tone": "bad"}]}]}"#;

/// The smallest right block of each kind, for an error about one.
pub fn block_like(kind: &str) -> &'static str {
    match kind {
        "table" => {
            r#"{"kind": "table", "columns": ["test", "time"], "items": [{"cells": ["TestRetry", "4s"], "tone": "bad"}]}"#
        }
        "kv" => r#"{"kind": "kv", "items": [{"text": "branch", "meta": "main"}]}"#,
        "checklist" => {
            r#"{"kind": "checklist", "items": [{"text": "freeze main", "done": true}, {"text": "sign"}]}"#
        }
        "log" => {
            r#"{"kind": "log", "items": [{"time": "14:05", "text": "ECONNRESET", "tone": "bad"}]}"#
        }
        "bars" => r#"{"kind": "bars", "items": [{"text": "e2e", "value": 9, "meta": "9m"}]}"#,
        "chart" => r#"{"kind": "chart", "values": [2, 3, 9, 4], "labels": ["14:00", "14:30"]}"#,
        "heatmap" => {
            r#"{"kind": "heatmap", "columns": ["M", "T"], "items": [{"text": "ui", "values": [0, 3]}]}"#
        }
        "compare" => {
            r#"{"kind": "compare", "items": [{"text": "bundle", "from": "412 kB", "to": "328 kB", "tone": "good"}]}"#
        }
        "stats" => {
            r#"{"kind": "stats", "stats": [{"value": "8", "label": "ready", "tone": "good", "trend": [2, 5, 8]}]}"#
        }
        "progress" => {
            r#"{"kind": "progress", "progress": {"done": 38, "total": 52, "label": "moved"}}"#
        }
        "steps" => {
            r#"{"kind": "steps", "items": [{"text": "build", "tone": "good"}, {"text": "deploy", "tone": "busy"}]}"#
        }
        "timeline" => {
            r#"{"kind": "timeline", "items": [{"time": "14:05", "text": "502s begin", "tone": "bad"}]}"#
        }
        "board" => {
            r#"{"kind": "board", "items": [{"text": "Doing", "items": [{"text": "TestRetry"}]}]}"#
        }
        "tree" => {
            r#"{"kind": "tree", "items": [{"text": "invoices", "items": [{"text": "create.ts"}]}]}"#
        }
        "hero" => {
            r#"{"kind": "hero", "value": "412", "text": "tests passing", "trend": [380, 412]}"#
        }
        "callout" => r#"{"kind": "callout", "tone": "warn", "text": "Waiting on your review."}"#,
        "code" => r#"{"kind": "code", "text": "max: 64, // was 32"}"#,
        "text" => r#"{"kind": "text", "text": "A sentence of context."}"#,
        _ => {
            r#"{"kind": "list", "items": [{"text": "PR 8947", "meta": "lint fails", "tone": "bad"}]}"#
        }
    }
}

const STAT_LIKE: &str = r#"{"value": "8", "label": "ready", "tone": "good", "trend": [2, 5, 8]}"#;
const PROGRESS_LIKE: &str = r#"{"done": 38, "total": 52, "label": "moved"}"#;
const GROUP_LIKE: &str =
    r#"{"title": "Failing", "rows": [{"text": "PR 8947", "meta": "lint", "tone": "bad"}]}"#;
const ACTION_LIKE: &str = r#"{"label": "rerun", "send": "Rerun the failed tests"}"#;

/// A word an agent may use for a tone, as rush's tone.
fn tone_word(t: &str) -> Option<&'static str> {
    Some(match t.trim().to_ascii_lowercase().as_str() {
        "good" | "ok" | "success" | "succeeded" | "pass" | "passed" | "passing" | "done"
        | "green" | "positive" => "good",
        "bad" | "error" | "err" | "fail" | "failed" | "failing" | "failure" | "danger" | "red"
        | "critical" | "negative" | "broken" => "bad",
        "warn" | "warning" | "yellow" | "caution" | "orange" => "warn",
        "busy" | "running" | "active" | "info" | "blue" | "in-progress" | "in_progress"
        | "progress" | "pending" => "busy",
        "quiet" | "muted" | "grey" | "gray" | "dim" | "neutral" | "todo" | "skipped" | "idle" => {
            "quiet"
        }
        _ => return None,
    })
}

/// A word an agent may use for a block's kind, as rush's kind.
fn kind_word(k: &str) -> String {
    let k = k.trim().to_ascii_lowercase();
    match k.as_str() {
        "stat" | "metrics" | "numbers" | "tiles" => "stats",
        "bar" | "barchart" | "bar-chart" => "bars",
        "kanban" => "board",
        "key-value" | "keyvalue" | "props" | "properties" => "kv",
        "todo" | "todos" | "tasks" => "checklist",
        "pipeline" | "stages" => "steps",
        "diff" | "before-after" => "compare",
        "note" | "alert" | "warning" => "callout",
        "sparkline" | "line" | "linechart" => "chart",
        "rows" | "items" => "list",
        _ => return k,
    }
    .to_string()
}

/// What reading one view found: what was left out, for the agent to hear,
/// or what is wrong with it.
struct Fix {
    notes: Vec<String>,
}

type Res = Result<(), String>;

fn wrong(path: &str, what: &str, like: &str) -> String {
    format!("{path}: {what}. Like: {like}")
}

/// v's number: a JSON number, or a string that starts with one ("38",
/// "1,204", "9m", "45%"); and whether it was a percent.
fn number(v: &Value) -> Option<(f64, bool)> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| (f, false)),
        Value::String(s) => {
            let s = s.trim().replace(',', "");
            let end = s
                .char_indices()
                .find(|&(i, c)| {
                    !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0))
                })
                .map_or(s.len(), |(i, _)| i);
            let f: f64 = s[..end].parse().ok()?;
            Some((f, s[end..].trim_start().starts_with('%')))
        }
        _ => None,
    }
}

impl Fix {
    /// A text field: numbers and yes/no read as their words.
    fn text(&mut self, o: &mut Obj, key: &str, path: &str, like: &str) -> Res {
        match o.get(key) {
            None | Some(Value::String(_)) => Ok(()),
            Some(Value::Null) => {
                o.remove(key);
                Ok(())
            }
            Some(Value::Number(n)) => {
                let s = n.to_string();
                o.insert(key.into(), Value::String(s));
                Ok(())
            }
            Some(Value::Bool(b)) => {
                let s = if *b { "yes" } else { "no" };
                o.insert(key.into(), Value::String(s.into()));
                Ok(())
            }
            Some(_) => Err(wrong(&format!("{path}.{key}"), "is words, a string", like)),
        }
    }

    /// A tone: rush's own, or a word that means one; any other is left
    /// for check to take off.
    fn tone(&mut self, o: &mut Obj, path: &str, like: &str) -> Res {
        self.text(o, "tone", path, like)?;
        if let Some(Value::String(t)) = o.get_mut("tone")
            && let Some(w) = tone_word(t)
        {
            *t = w.into();
        }
        Ok(())
    }

    /// A number: "38", "9m" and "1,204" read as theirs, and for a
    /// fraction a percent ("45%", or 45) as its hundredth; words where a
    /// number goes ("lots") are left out, and said.
    fn num(&mut self, o: &mut Obj, key: &str, path: &str, fraction: bool, like: &str) -> Res {
        let Some(v) = o.get(key) else { return Ok(()) };
        let got = match v {
            Value::Null => None,
            Value::String(s) if s.trim().is_empty() => None,
            Value::Number(_) | Value::String(_) => match number(v) {
                Some(n) => Some(n),
                None => {
                    self.notes.push(format!(
                        "{path}.{key} is a number: {} was left out",
                        serde_json::to_string(v).unwrap_or_default()
                    ));
                    None
                }
            },
            _ => {
                let what = if fraction {
                    "is a number from 0 to 1"
                } else {
                    "is a number"
                };
                return Err(wrong(&format!("{path}.{key}"), what, like));
            }
        };
        match got {
            None => {
                o.remove(key);
            }
            Some((f, pct)) => {
                let f = if fraction && (pct || f > 1.0) && f <= 100.0 {
                    f / 100.0
                } else {
                    f
                };
                o.insert(key.into(), serde_json::json!(f));
            }
        }
        Ok(())
    }

    /// Numbers, oldest first: one number is a list of it, a string of
    /// numbers ("2, 5, 8") is read as them, and words ("up") are left out,
    /// and said.
    fn nums(&mut self, o: &mut Obj, key: &str, path: &str, like: &str) -> Res {
        let Some(v) = o.get(key) else { return Ok(()) };
        let out: Vec<Value> = match v {
            Value::Null => {
                o.remove(key);
                return Ok(());
            }
            Value::Number(_) => vec![v.clone()],
            Value::String(s) => {
                let parts: Vec<&str> = s
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|p| !p.is_empty())
                    .collect();
                let ns: Option<Vec<f64>> = parts.iter().map(|p| p.parse().ok()).collect();
                match ns {
                    Some(ns) if !ns.is_empty() => {
                        ns.into_iter().map(|f| serde_json::json!(f)).collect()
                    }
                    _ => {
                        self.notes.push(format!(
                            "{path}.{key} is numbers, oldest first, like [2, 5, 8]: {} was left out",
                            serde_json::to_string(s).unwrap_or_default()
                        ));
                        o.remove(key);
                        return Ok(());
                    }
                }
            }
            Value::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for x in a {
                    match number(x) {
                        Some((f, _)) => out.push(serde_json::json!(f)),
                        None if x.is_null() => out.push(serde_json::json!(0)),
                        None => {
                            return Err(wrong(
                                &format!("{path}.{key}"),
                                "is a list of numbers, oldest first",
                                like,
                            ));
                        }
                    }
                }
                out
            }
            _ => {
                return Err(wrong(
                    &format!("{path}.{key}"),
                    "is a list of numbers, oldest first",
                    like,
                ));
            }
        };
        o.insert(key.into(), Value::Array(out));
        Ok(())
    }

    /// A list of words: one word is a list of it, numbers read as words.
    fn texts(&mut self, o: &mut Obj, key: &str, path: &str, like: &str) -> Res {
        let Some(v) = o.get(key) else { return Ok(()) };
        let out: Vec<Value> = match v {
            Value::Null => {
                o.remove(key);
                return Ok(());
            }
            Value::String(_) => vec![v.clone()],
            Value::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for x in a {
                    out.push(match x {
                        Value::String(_) => x.clone(),
                        Value::Number(n) => Value::String(n.to_string()),
                        Value::Bool(b) => Value::String(if *b { "yes" } else { "no" }.into()),
                        Value::Null => Value::String(String::new()),
                        _ => {
                            return Err(wrong(
                                &format!("{path}.{key}"),
                                "is a list of strings",
                                like,
                            ));
                        }
                    });
                }
                out
            }
            _ => {
                return Err(wrong(
                    &format!("{path}.{key}"),
                    "is a list of strings",
                    like,
                ));
            }
        };
        o.insert(key.into(), Value::Array(out));
        Ok(())
    }

    /// Yes or no: "true", "yes", "done", 1 read as yes.
    fn yes(&mut self, o: &mut Obj, key: &str, path: &str, like: &str) -> Res {
        let b = match o.get(key) {
            None | Some(Value::Bool(_)) => return Ok(()),
            Some(Value::Null) => {
                o.remove(key);
                return Ok(());
            }
            Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "y" | "done" | "1" | "x" | "✓" => true,
                "false" | "no" | "n" | "" | "0" | "todo" => false,
                _ => return Err(wrong(&format!("{path}.{key}"), "is true or false", like)),
            },
            Some(_) => return Err(wrong(&format!("{path}.{key}"), "is true or false", like)),
        };
        o.insert(key.into(), Value::Bool(b));
        Ok(())
    }

    /// A list of objects, each read by each: one object is a list of it,
    /// and a plain word is an object of that text when words: is set.
    fn list(
        &mut self,
        o: &mut Obj,
        key: &str,
        path: &str,
        like: &str,
        words: Option<&str>,
        mut each: impl FnMut(&mut Self, &mut Obj, &str) -> Res,
    ) -> Res {
        let Some(v) = o.get_mut(key) else {
            return Ok(());
        };
        match v {
            Value::Null => {
                o.remove(key);
                return Ok(());
            }
            Value::Object(_) => *v = Value::Array(vec![v.take()]),
            Value::Array(_) => {}
            _ => {
                return Err(wrong(&format!("{path}.{key}"), "is a list", like));
            }
        }
        let Some(Value::Array(a)) = o.get_mut(key) else {
            return Ok(());
        };
        a.retain(|x| !x.is_null());
        for (i, x) in a.iter_mut().enumerate() {
            let at = format!("{path}.{key}[{i}]");
            let plain = match &*x {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            };
            if let (Some(field), Some(s)) = (words, plain) {
                let mut m = Obj::new();
                m.insert(field.into(), Value::String(s));
                *x = Value::Object(m);
            }
            match x {
                Value::Object(m) => each(self, m, &at)?,
                _ => return Err(wrong(&at, "is an object", like)),
            }
        }
        Ok(())
    }

    /// An object field read by each; null is none.
    fn object(
        &mut self,
        o: &mut Obj,
        key: &str,
        path: &str,
        like: &str,
        each: impl FnOnce(&mut Self, &mut Obj, &str) -> Res,
    ) -> Res {
        match o.get_mut(key) {
            None => Ok(()),
            Some(Value::Null) => {
                o.remove(key);
                Ok(())
            }
            Some(Value::Object(m)) => each(self, m, &format!("{path}.{key}")),
            Some(_) => Err(wrong(&format!("{path}.{key}"), "is an object", like)),
        }
    }

    /// A bar of done of total: also a fraction (0.6), a percent ("60%")
    /// or "3/5".
    fn progress(&mut self, o: &mut Obj, path: &str) -> Res {
        let as_obj = |done: f64, total: f64| serde_json::json!({"done": done, "total": total});
        match o.get("progress") {
            Some(Value::Number(n)) => {
                let f = n.as_f64().unwrap_or(0.0);
                let p = if f <= 1.0 {
                    as_obj(f, 1.0)
                } else if f <= 100.0 {
                    as_obj(f, 100.0)
                } else {
                    return Err(wrong(
                        &format!("{path}.progress"),
                        "is done of total",
                        PROGRESS_LIKE,
                    ));
                };
                o.insert("progress".into(), p);
            }
            Some(Value::String(s)) => {
                let p = if let Some((d, t)) = s
                    .split_once(['/', ' '])
                    .filter(|_| s.contains('/') || s.contains(" of "))
                {
                    let t = t.trim().trim_start_matches("of").trim();
                    match (
                        number(&Value::String(d.into())),
                        number(&Value::String(t.into())),
                    ) {
                        (Some((d, _)), Some((t, _))) => as_obj(d, t),
                        _ => {
                            return Err(wrong(
                                &format!("{path}.progress"),
                                "is done of total",
                                PROGRESS_LIKE,
                            ));
                        }
                    }
                } else {
                    match number(&Value::String(s.clone())) {
                        Some((f, true)) => as_obj(f, 100.0),
                        Some((f, false)) if f <= 1.0 => as_obj(f, 1.0),
                        _ => {
                            return Err(wrong(
                                &format!("{path}.progress"),
                                "is done of total",
                                PROGRESS_LIKE,
                            ));
                        }
                    }
                };
                o.insert("progress".into(), p);
            }
            _ => {}
        }
        self.object(o, "progress", path, PROGRESS_LIKE, |f, p, at| {
            f.num(p, "done", at, false, PROGRESS_LIKE)?;
            f.num(p, "total", at, false, PROGRESS_LIKE)?;
            f.text(p, "label", at, PROGRESS_LIKE)?;
            f.tone(p, at, PROGRESS_LIKE)
        })
    }

    /// Stats: also an object of label to value, {"passed": 412}.
    fn stats(&mut self, o: &mut Obj, path: &str) -> Res {
        if let Some(Value::Object(m)) = o.get("stats")
            && !m.contains_key("value")
            && !m.contains_key("label")
        {
            let list: Vec<Value> = m
                .iter()
                .map(|(k, v)| serde_json::json!({"label": k, "value": v}))
                .collect();
            o.insert("stats".into(), Value::Array(list));
        }
        self.list(o, "stats", path, STAT_LIKE, None, |f, s, at| {
            f.text(s, "value", at, STAT_LIKE)?;
            f.text(s, "label", at, STAT_LIKE)?;
            f.tone(s, at, STAT_LIKE)?;
            f.nums(s, "trend", at, STAT_LIKE)
        })
    }

    fn actions(&mut self, o: &mut Obj, path: &str) -> Res {
        self.list(
            o,
            "actions",
            path,
            ACTION_LIKE,
            Some("label"),
            |f, a, at| {
                for k in ["label", "send", "open", "copy"] {
                    f.text(a, k, at, ACTION_LIKE)?;
                }
                f.tone(a, at, ACTION_LIKE)?;
                f.object(a, "set", at, ACTION_LIKE, |f, s, at| {
                    f.yes(s, "done", at, ACTION_LIKE)?;
                    f.text(s, "meta", at, ACTION_LIKE)?;
                    f.tone(s, at, ACTION_LIKE)
                })
            },
        )
    }

    /// One of a block's items, and the items under it.
    fn item(&mut self, it: &mut Obj, path: &str, like: &str, depth: usize) -> Res {
        for k in ["text", "meta", "link", "time", "from", "to", "id", "detail"] {
            self.text(it, k, path, like)?;
        }
        self.tone(it, path, like)?;
        self.num(it, "value", path, false, like)?;
        self.num(it, "progress", path, true, like)?;
        self.yes(it, "done", path, like)?;
        self.texts(it, "cells", path, like)?;
        self.nums(it, "values", path, like)?;
        self.actions(it, path)?;
        if depth < 8 {
            self.list(it, "items", path, like, Some("text"), |f, c, at| {
                f.item(c, at, like, depth + 1)
            })?;
        }
        Ok(())
    }

    fn block(&mut self, b: &mut Obj, path: &str) -> Res {
        self.text(b, "kind", path, block_like(""))?;
        let mut line = false;
        if let Some(Value::String(k)) = b.get_mut("kind") {
            line = matches!(k.trim().to_ascii_lowercase().as_str(), "line" | "sparkline");
            *k = kind_word(k);
        }
        if line && !b.contains_key("style") {
            b.insert("style".into(), Value::String("line".into()));
        }
        let kind = b
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let like = block_like(&kind);
        for k in ["title", "text", "value", "style"] {
            self.text(b, k, path, like)?;
        }
        self.tone(b, path, like)?;
        self.yes(b, "folded", path, like)?;
        self.texts(b, "columns", path, like)?;
        self.texts(b, "labels", path, like)?;
        self.nums(b, "values", path, like)?;
        self.nums(b, "trend", path, like)?;
        // A list's rows called rows, as a group's are.
        if !b.contains_key("items")
            && let Some(rows) = b.remove("rows")
        {
            b.insert("items".into(), rows);
        }
        self.list(b, "items", path, like, Some("text"), |f, it, at| {
            f.item(it, at, like, 0)
        })?;
        self.stats(b, path)?;
        self.progress(b, path)
    }

    fn view(&mut self, v: &mut Obj) -> Res {
        // What only rush writes.
        for k in ["updated", "gone", "yours", "pinned", "by"] {
            v.remove(k);
        }
        let p = "view";
        for k in ["name", "title", "summary", "note", "accent"] {
            self.text(v, k, p, VIEW_LIKE)?;
        }
        if let Some(Value::String(a)) = v.get_mut("accent") {
            *a = a.trim().to_ascii_lowercase();
        }
        self.tone(v, p, VIEW_LIKE)?;
        self.progress(v, p)?;
        self.stats(v, p)?;
        self.list(v, "groups", p, GROUP_LIKE, None, |f, g, at| {
            for k in ["title", "text"] {
                f.text(g, k, at, GROUP_LIKE)?;
            }
            f.tone(g, at, GROUP_LIKE)?;
            f.yes(g, "folded", at, GROUP_LIKE)?;
            if !g.contains_key("rows")
                && let Some(items) = g.remove("items")
            {
                g.insert("rows".into(), items);
            }
            f.list(g, "rows", at, GROUP_LIKE, Some("text"), |f, r, at| {
                for k in ["text", "meta", "link", "id", "detail"] {
                    f.text(r, k, at, GROUP_LIKE)?;
                }
                f.tone(r, at, GROUP_LIKE)?;
                f.num(r, "progress", at, true, GROUP_LIKE)?;
                f.num(r, "value", at, false, GROUP_LIKE)?;
                f.actions(r, at)
            })
        })?;
        self.list(v, "blocks", p, block_like(""), None, |f, b, at| {
            f.block(b, at)
        })?;
        self.actions(v, p)
    }
}

/// The view in raw, read as its agent meant it, and what was left out of
/// it to tell the agent; or what is wrong with it, by its field, with the
/// shape to write.
pub fn parse_view(raw: &[u8]) -> Result<(View, Vec<String>), String> {
    let mut v: Value = serde_json::from_slice(raw)
        .map_err(|e| format!("its JSON: {e}. A view is like: {VIEW_LIKE}"))?;
    // Some clients send the object as a string of its JSON.
    let inner = match &v {
        Value::String(s) => serde_json::from_str::<Value>(s).ok(),
        _ => None,
    };
    if let Some(inner) = inner {
        v = inner;
    }
    let Value::Object(o) = &mut v else {
        return Err(format!("a view is an object, like: {VIEW_LIKE}"));
    };
    let mut f = Fix { notes: Vec::new() };
    f.view(o)?;
    let view: View =
        serde_json::from_value(v).map_err(|e| format!("its view: {e}. Like: {VIEW_LIKE}"))?;
    Ok((view, f.notes))
}

/// What the agent hears of what was left out: a line, or nothing.
pub fn notes_line(notes: &[String]) -> String {
    match notes.len() {
        0 => String::new(),
        _ => format!(" Left out: {}.", notes.join("; ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v with every number a float, as a view writes 9 for 9.0.
    fn floats(v: Value) -> Value {
        match v {
            Value::Number(n) => serde_json::json!(n.as_f64().unwrap_or_default()),
            Value::Array(a) => Value::Array(a.into_iter().map(floats).collect()),
            Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, floats(v))).collect()),
            v => v,
        }
    }

    /// Slips agents make, each read as meant: what the view holds after,
    /// as JSON at a pointer, and the notes the agent is told.
    #[test]
    fn reads_slips_as_meant() {
        struct Case {
            what: &'static str,
            raw: &'static str,
            at: &'static str,
            want: Value,
            notes: usize,
        }
        let cases = [
            Case {
                what: "a stat's trend as a word is left out, and said",
                raw: r#"{"title":"T","stats":[{"value":"516 MB","label":"memory","trend":"up"}]}"#,
                at: "/stats/0/trend",
                want: Value::Null,
                notes: 1,
            },
            Case {
                what: "a trend as a string of numbers",
                raw: r#"{"title":"T","stats":[{"value":"8","label":"ready","trend":"2, 5 8"}]}"#,
                at: "/stats/0/trend",
                want: serde_json::json!([2.0, 5.0, 8.0]),
                notes: 0,
            },
            Case {
                what: "a trend of one number",
                raw: r#"{"title":"T","blocks":[{"kind":"hero","value":"41","trend":41}]}"#,
                at: "/blocks/0/trend",
                want: serde_json::json!([41.0]),
                notes: 0,
            },
            Case {
                what: "a stat's value as a number",
                raw: r#"{"title":"T","stats":[{"value":8,"label":"ready"}]}"#,
                at: "/stats/0/value",
                want: serde_json::json!("8"),
                notes: 0,
            },
            Case {
                what: "stats as an object of label to value",
                raw: r#"{"title":"T","stats":{"passed":412}}"#,
                at: "/stats/0/label",
                want: serde_json::json!("passed"),
                notes: 0,
            },
            Case {
                what: "a hero's value as a number",
                raw: r#"{"title":"T","blocks":[{"kind":"hero","value":412}]}"#,
                at: "/blocks/0/value",
                want: serde_json::json!("412"),
                notes: 0,
            },
            Case {
                what: "a bar's value as a string with its unit",
                raw: r#"{"title":"T","blocks":[{"kind":"bars","items":[{"text":"e2e","value":"9m"}]}]}"#,
                at: "/blocks/0/items/0/value",
                want: serde_json::json!(9.0),
                notes: 0,
            },
            Case {
                what: "a row's progress as a percent",
                raw: r#"{"title":"T","blocks":[{"kind":"list","items":[{"text":"ui","progress":"45%"}]}]}"#,
                at: "/blocks/0/items/0/progress",
                want: serde_json::json!(0.45),
                notes: 0,
            },
            Case {
                what: "progress as done of total in words",
                raw: r#"{"title":"T","progress":"3/5"}"#,
                at: "/progress/total",
                want: serde_json::json!(5.0),
                notes: 0,
            },
            Case {
                what: "progress's numbers as strings",
                raw: r#"{"title":"T","progress":{"done":"38","total":"52"}}"#,
                at: "/progress/done",
                want: serde_json::json!(38.0),
                notes: 0,
            },
            Case {
                what: "a tone's other word",
                raw: r#"{"title":"T","blocks":[{"kind":"list","items":[{"text":"a","tone":"success"},{"text":"b","tone":"error"}]}]}"#,
                at: "/blocks/0/items/1/tone",
                want: serde_json::json!("bad"),
                notes: 0,
            },
            Case {
                what: "a kind's other word",
                raw: r#"{"title":"T","blocks":[{"kind":"Stat","stats":[{"value":"1","label":"x"}]}]}"#,
                at: "/blocks/0/kind",
                want: serde_json::json!("stats"),
                notes: 0,
            },
            Case {
                what: "items as plain words",
                raw: r#"{"title":"T","blocks":[{"kind":"list","items":["one","two"]}]}"#,
                at: "/blocks/0/items/1/text",
                want: serde_json::json!("two"),
                notes: 0,
            },
            Case {
                what: "a table's cells as numbers",
                raw: r#"{"title":"T","blocks":[{"kind":"table","columns":["a","n"],"items":[{"cells":["x",3]}]}]}"#,
                at: "/blocks/0/items/0/cells/1",
                want: serde_json::json!("3"),
                notes: 0,
            },
            Case {
                what: "a checklist's done as a word",
                raw: r#"{"title":"T","blocks":[{"kind":"checklist","items":[{"text":"x","done":"yes"}]}]}"#,
                at: "/blocks/0/items/0/done",
                want: serde_json::json!(true),
                notes: 0,
            },
            Case {
                what: "a list's rows called rows",
                raw: r#"{"title":"T","blocks":[{"kind":"list","rows":[{"text":"x"}]}]}"#,
                at: "/blocks/0/items/0/text",
                want: serde_json::json!("x"),
                notes: 0,
            },
            Case {
                what: "one block where a list goes",
                raw: r#"{"title":"T","blocks":{"kind":"callout","text":"hi"}}"#,
                at: "/blocks/0/text",
                want: serde_json::json!("hi"),
                notes: 0,
            },
            Case {
                what: "the view as a string of its JSON",
                raw: r#""{\"title\":\"T\",\"note\":\"n\"}""#,
                at: "/note",
                want: serde_json::json!("n"),
                notes: 0,
            },
            Case {
                what: "words where a number goes are left out, and said",
                raw: r#"{"title":"T","blocks":[{"kind":"bars","items":[{"text":"e2e","value":"lots"}]}]}"#,
                at: "/blocks/0/items/0/value",
                want: Value::Null,
                notes: 1,
            },
            Case {
                what: "a kind of line is a chart drawn as a line",
                raw: r#"{"title":"T","blocks":[{"kind":"line","values":[1,2]}]}"#,
                at: "/blocks/0/style",
                want: serde_json::json!("line"),
                notes: 0,
            },
            Case {
                what: "nulls are none",
                raw: r#"{"title":"T","summary":null,"stats":null,"blocks":[null,{"kind":"text","text":"t","items":null}]}"#,
                at: "/blocks/0/text",
                want: serde_json::json!("t"),
                notes: 0,
            },
        ];
        for c in cases {
            let (v, notes) =
                parse_view(c.raw.as_bytes()).unwrap_or_else(|e| panic!("{}: {e}", c.what));
            let got = serde_json::to_value(&v).unwrap();
            let at = got.pointer(c.at).cloned().unwrap_or(Value::Null);
            assert_eq!(floats(at), floats(c.want), "{}: {got}", c.what);
            assert_eq!(notes.len(), c.notes, "{}: {notes:?}", c.what);
        }
    }

    /// What can't be read as meant names its field and shows the shape to
    /// write.
    #[test]
    fn says_where_and_what_to_write() {
        let cases = [
            (
                r#"{"title":"T","stats":[{"value":"1","trend":{"a":1}}]}"#,
                "view.stats[0].trend: is a list of numbers",
                r#""trend": [2, 5, 8]"#,
            ),
            (
                r#"{"title":"T","blocks":[{"kind":"table","items":[{"cells":[["x"]]}]}]}"#,
                "view.blocks[0].items[0].cells: is a list of strings",
                r#"{"kind": "table", "columns""#,
            ),
            (
                r#"{"title":"T","blocks":[{"kind":"bars","items":[{"text":"e2e","value":{"n":9}}]}]}"#,
                "view.blocks[0].items[0].value: is a number",
                r#"{"kind": "bars""#,
            ),
            (
                r#"{"title":"T","blocks":[{"kind":"list","items":"a"}]}"#,
                "view.blocks[0].items: is a list",
                r#"{"kind": "list""#,
            ),
            (
                r#"{"title":"T","progress":"most"}"#,
                "view.progress: is done of total",
                r#""done": 38"#,
            ),
            (r#"[1,2]"#, "a view is an object", r#""title": "Tests""#),
            (r#"{"title": "#, "its JSON:", r#""title": "Tests""#),
        ];
        for (raw, want, like) in cases {
            let e = parse_view(raw.as_bytes()).expect_err(raw);
            assert!(e.starts_with(want), "{raw}: {e}");
            assert!(e.contains(like), "{raw}: no example in {e}");
        }
    }

    /// A view written right is read as it was, with nothing to say.
    #[test]
    fn a_right_view_is_as_it_was() {
        for (name, ex) in crate::viewwords::VIEW_EXAMPLES {
            let (v, notes) = parse_view(ex.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
            let plain: View = serde_json::from_str(ex).unwrap();
            assert_eq!(v, plain, "{name}");
            assert!(notes.is_empty(), "{name}: {notes:?}");
        }
    }
}
