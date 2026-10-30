//! The ts plugin's tools: where, as file and 1-based line with a column or
//! a symbol on that line, or a symbol alone to search for; and what each
//! asks the language server and how it says the answer.
//! Ported from internal/bundled/ts/tools.go:1-449.

use std::time::Duration;

use rush_plugin::Ctx;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::pool::Pool;
use super::server::{Server, decode, path_of};

/// One tool call's arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ask {
    pub file: String,
    pub line: i64,
    pub character: i64,
    pub symbol: String,
    pub query: String,
    pub direction: String,
    pub new_name: String,
    pub to: String,
}

/// What a tool does, given its server, the URI of its file, where in it,
/// the call and the folder it was asked from.
pub type Do = fn(&Ctx, &Server, &str, Position, &Ask, &str) -> Result<String, String>;

/// One tool: its name, what it's for, and what it does.
pub struct Op {
    pub name: &'static str,
    pub description: &'static str,
    /// "position", "file" or "query"
    pub needs: &'static str,
    pub run: Do,
}

/// LSP's: a 0-based line and UTF-16 column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    pub line: i64,
    pub character: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Range {
    pub start: Position,
    pub end: Position,
}

/// A Location or a LocationLink, as servers answer either.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Location {
    uri: String,
    range: Option<Range>,
    #[serde(rename = "targetUri")]
    target_uri: String,
    #[serde(rename = "targetSelectionRange")]
    target_selection_range: Option<Range>,
}

impl Location {
    pub(crate) fn at(&self) -> (String, Position) {
        if !self.target_uri.is_empty()
            && let Some(r) = self.target_selection_range
        {
            return (self.target_uri.clone(), r.start);
        }
        (
            self.uri.clone(),
            self.range.map(|r| r.start).unwrap_or_default(),
        )
    }
}

/// The tools, in the order they're listed.
pub static OPS: &[Op] = &[
    Op {
        name: "definition",
        needs: "position",
        description: "Where the symbol at a position is declared.",
        run: definition,
    },
    Op {
        name: "references",
        needs: "position",
        description: "Every use of the symbol at a position, across the checkout, its declaration left out.",
        run: references,
    },
    Op {
        name: "implementation",
        needs: "position",
        description: "What implements the interface or abstract member at a position.",
        run: implementation,
    },
    Op {
        name: "hover",
        needs: "position",
        description: "The type and docs of the symbol at a position.",
        run: hover,
    },
    Op {
        name: "callers",
        needs: "position",
        description: "The functions that call the function at a position.",
        run: callers,
    },
    Op {
        name: "callees",
        needs: "position",
        description: "The functions the function at a position calls.",
        run: callees,
    },
    Op {
        name: "outline",
        needs: "file",
        description: "A file's declarations, nested, with their lines: read only the range you need.",
        run: outline,
    },
    Op {
        name: "diagnostics",
        needs: "file",
        description: "A file's type errors and warnings, as the compiler sees them now.",
        run: diagnostics,
    },
    Op {
        name: "find_symbol",
        needs: "query",
        description: "Declarations whose name matches query, across the checkout. In a monorepo this loads every \
                      project: prefer a position-taking tool with file and line when you know where to look.",
        run: find_symbol,
    },
    Op {
        name: "rename",
        needs: "position",
        description: "Renames the symbol at a position to new_name everywhere \
                      it's used, and writes the files. It lists the files changed.",
        run: super::edits::rename,
    },
    Op {
        name: "rename_file",
        needs: "file",
        description: "Moves a file to the path to and rewrites every import \
                      of it. It lists the files changed.",
        run: super::edits::rename_file,
    },
];

/// The op called name.
pub fn op_named(name: &str) -> Option<&'static Op> {
    OPS.iter().find(|o| o.name == name)
}

/// How long one call may take; a cold monorepo takes a while.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Does op for a, asked from cwd.
pub fn run_op(ctx: &Ctx, p: &Pool, op: &Op, a: &Ask, cwd: &str) -> Result<String, String> {
    let ctx = ctx.with_timeout(CALL_TIMEOUT);
    let mut a = a.clone();
    let mut file = a.file.clone();
    if !file.is_empty() && !std::path::Path::new(&file).is_absolute() {
        file = std::path::Path::new(cwd)
            .join(&file)
            .to_string_lossy()
            .into_owned();
    }
    let anchor = if file.is_empty() { cwd } else { &file };
    let s = p.server_for(&ctx, anchor)?;
    if op.needs == "query" {
        if a.query.is_empty() {
            a.query = a.symbol.clone();
        }
        if a.query.is_empty() {
            return Err("give query: the name, or part of it, to look for".into());
        }
        return (op.run)(&ctx, &s, "", Position::default(), &a, cwd);
    }
    if file.is_empty() {
        return Err(
            "give file, and for this tool a line with a character or a symbol on it".into(),
        );
    }
    let uri = s.open(&file)?;
    let mut pos = Position::default();
    if op.needs == "position" {
        pos = position(&file, &a)?;
    }
    (op.run)(&ctx, &s, &uri, pos, &a, cwd)
}

/// Where a points in file: its column, or where its symbol first stands
/// whole on its line.
pub(crate) fn position(file: &str, a: &Ask) -> Result<Position, String> {
    if a.line < 1 {
        return Err("give line, from 1, with character or symbol".into());
    }
    let b = std::fs::read(file)
        .map_err(|e| rush_plugin::gopath::os_err("open", std::path::Path::new(file), &e))?;
    let text = String::from_utf8_lossy(&b);
    let lines: Vec<&str> = text.split('\n').collect();
    if a.line as usize > lines.len() {
        return Err(format!("{file} has {} lines, not {}", lines.len(), a.line));
    }
    let line = lines[a.line as usize - 1];
    if a.character > 0 {
        let prefix: String = line.chars().take(a.character as usize - 1).collect();
        return Ok(Position {
            line: a.line - 1,
            character: utf16_len(&prefix),
        });
    }
    if !a.symbol.is_empty() {
        let Some(i) = word_index(line, &a.symbol) else {
            return Err(format!(
                "no {} on line {} of {file}: {}",
                rush_plugin::text::go_q(&a.symbol),
                a.line,
                line.trim()
            ));
        };
        return Ok(Position {
            line: a.line - 1,
            character: utf16_len(&line[..i]),
        });
    }
    Err("give character (from 1) or symbol, to say where on the line".into())
}

/// The byte offset of word's first whole occurrence in s.
pub(crate) fn word_index(s: &str, word: &str) -> Option<usize> {
    let is_word = |r: char| r == '_' || r == '$' || r.is_ascii_alphanumeric();
    if word.is_empty() {
        return None;
    }
    let step = word.chars().next().map_or(1, char::len_utf8);
    let mut off = 0;
    loop {
        let i = off + s[off..].find(word)?;
        let before = s[..i].chars().next_back();
        let after = s[i + word.len()..].chars().next();
        if before.is_none_or(|c| !is_word(c)) && after.is_none_or(|c| !is_word(c)) {
            return Some(i);
        }
        off = i + step;
    }
}

pub(crate) fn utf16_len(s: &str) -> i64 {
    s.encode_utf16().count() as i64
}

pub(crate) fn doc_params(uri: &str, pos: Position, extra: Value) -> Value {
    let mut m = serde_json::json!({ "textDocument": { "uri": uri }, "position": pos });
    if let (Some(m), Value::Object(extra)) = (m.as_object_mut(), extra) {
        m.extend(extra);
    }
    m
}

fn definition(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    _: &Ask,
    cwd: &str,
) -> Result<String, String> {
    locations(
        ctx,
        s,
        "textDocument/definition",
        uri,
        pos,
        Value::Null,
        cwd,
    )
}

fn references(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    _: &Ask,
    cwd: &str,
) -> Result<String, String> {
    let extra = serde_json::json!({ "context": { "includeDeclaration": false } });
    locations(ctx, s, "textDocument/references", uri, pos, extra, cwd)
}

fn implementation(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    _: &Ask,
    cwd: &str,
) -> Result<String, String> {
    locations(
        ctx,
        s,
        "textDocument/implementation",
        uri,
        pos,
        Value::Null,
        cwd,
    )
}

fn locations(
    ctx: &Ctx,
    s: &Server,
    method: &str,
    uri: &str,
    pos: Position,
    extra: Value,
    cwd: &str,
) -> Result<String, String> {
    let res = s.call(ctx, method, &doc_params(uri, pos, extra))?;
    let locs = decode_locations(res.as_deref().map_or("", RawValue::get))?;
    if locs.is_empty() {
        return Ok("none".into());
    }
    let mut out: Vec<String> = locs
        .iter()
        .map(|l| {
            let (u, p) = l.at();
            where_(&u, p, cwd)
        })
        .collect();
    out.sort();
    Ok(limit(&out, 200))
}

pub(crate) fn decode_locations(res: &str) -> Result<Vec<Location>, String> {
    let t = res.trim();
    match t.chars().next() {
        None | Some('n') => Ok(Vec::new()),
        Some('{') => rush_state::json::from_slice::<Location>(t.as_bytes())
            .map(|l| vec![l])
            .map_err(|e| e.to_string()),
        _ => rush_state::json::from_slice(t.as_bytes()).map_err(|e| e.to_string()),
    }
}

/// "path:line:col: the line", path from cwd when it's under it.
pub(crate) fn where_(uri: &str, p: Position, cwd: &str) -> String {
    let mut path = path_of(uri);
    let mut text = String::new();
    if let Ok(b) = std::fs::read(&path) {
        let b = String::from_utf8_lossy(&b);
        if let Some(l) = b.split('\n').nth(p.line as usize) {
            text = l.trim().to_string();
            if text.len() > 160 {
                let mut cut = 160;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text = format!("{}…", &text[..cut]);
            }
        }
    }
    if let Some(rel) = crate::go_rel(cwd, &path)
        && !rel.starts_with("..")
    {
        path = rel;
    }
    format!("{path}:{}:{}: {text}", p.line + 1, p.character + 1)
}

pub(crate) fn limit(lines: &[String], n: usize) -> String {
    if lines.len() <= n {
        return lines.join("\n");
    }
    format!("{}\n… and {} more", lines[..n].join("\n"), lines.len() - n)
}

fn hover(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    _: &Ask,
    _: &str,
) -> Result<String, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Hover {
        contents: Option<Box<RawValue>>,
    }
    let res = s.call(
        ctx,
        "textDocument/hover",
        &doc_params(uri, pos, Value::Null),
    )?;
    let h: Hover = decode(res.as_deref())?;
    let t = markup(h.contents.as_deref().map_or("", RawValue::get));
    if !t.is_empty() {
        return Ok(t);
    }
    Ok("nothing to say about that position".into())
}

/// Hover contents as text: MarkupContent, a MarkedString, or a list of
/// them.
pub(crate) fn markup(v: &str) -> String {
    let v = v.trim();
    if v.starts_with('{') {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Mc {
            value: String,
        }
        if let Ok(mc) = rush_state::json::from_slice::<Mc>(v.as_bytes())
            && !mc.value.is_empty()
        {
            return mc.value.trim().to_string();
        }
    }
    if let Ok(s) = serde_json::from_str::<String>(v) {
        return s.trim().to_string();
    }
    if let Ok(list) = serde_json::from_str::<Vec<Box<RawValue>>>(v) {
        return list
            .iter()
            .map(|x| markup(x.get()))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    String::new()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HierarchyItem {
    name: String,
    kind: i64,
    uri: String,
    #[serde(rename = "selectionRange")]
    selection_range: Range,
}

fn callers(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    _: &Ask,
    cwd: &str,
) -> Result<String, String> {
    calls(ctx, s, "callHierarchy/incomingCalls", "from", uri, pos, cwd)
}

fn callees(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    _: &Ask,
    cwd: &str,
) -> Result<String, String> {
    calls(ctx, s, "callHierarchy/outgoingCalls", "to", uri, pos, cwd)
}

fn calls(
    ctx: &Ctx,
    s: &Server,
    method: &str,
    side: &str,
    uri: &str,
    pos: Position,
    cwd: &str,
) -> Result<String, String> {
    let r = s.call(
        ctx,
        "textDocument/prepareCallHierarchy",
        &doc_params(uri, pos, Value::Null),
    )?;
    let items: Vec<Box<RawValue>> = decode(r.as_deref())?;
    let Some(first) = items.first() else {
        return Ok("no function at that position".into());
    };
    let r = s.call(ctx, method, &serde_json::json!({ "item": first }))?;
    let res: Vec<std::collections::HashMap<String, Box<RawValue>>> = decode(r.as_deref())?;
    let mut out: Vec<String> = res
        .iter()
        .filter_map(|c| {
            let it: HierarchyItem =
                rush_state::json::from_slice(c.get(side)?.get().as_bytes()).ok()?;
            Some(format!(
                "{} {}  {}",
                kind_name(it.kind),
                it.name,
                where_(&it.uri, it.selection_range.start, cwd)
            ))
        })
        .collect();
    if out.is_empty() {
        return Ok("none".into());
    }
    out.sort();
    Ok(limit(&out, 200))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DocSymbol {
    name: String,
    kind: i64,
    range: Range,
    children: Option<Vec<DocSymbol>>,
    location: Option<Location>,
}

fn outline(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    _: Position,
    _: &Ask,
    _: &str,
) -> Result<String, String> {
    let r = s.call(
        ctx,
        "textDocument/documentSymbol",
        &serde_json::json!({ "textDocument": { "uri": uri } }),
    )?;
    let syms: Vec<DocSymbol> = decode(r.as_deref())?;
    let mut out = Vec::new();
    fn walk(ss: &[DocSymbol], depth: usize, out: &mut Vec<String>) {
        for d in ss {
            let r = d.location.as_ref().and_then(|l| l.range).unwrap_or(d.range);
            out.push(format!(
                "{}{} {}  {}-{}",
                "  ".repeat(depth),
                kind_name(d.kind),
                d.name,
                r.start.line + 1,
                r.end.line + 1
            ));
            walk(d.children.as_deref().unwrap_or_default(), depth + 1, out);
        }
    }
    walk(&syms, 0, &mut out);
    if out.is_empty() {
        return Ok("no declarations".into());
    }
    Ok(limit(&out, 400))
}

/// A member that's there, even as null, read as written.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

fn diagnostics(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    _: Position,
    _: &Ask,
    cwd: &str,
) -> Result<String, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Diag {
        range: Range,
        severity: i64,
        message: String,
        #[serde(deserialize_with = "present")]
        code: Option<Box<RawValue>>,
    }
    let raw = s.diagnostics(ctx, uri, Duration::from_secs(15));
    let ds: Vec<Diag> = decode(raw.as_deref())?;
    if ds.is_empty() {
        return Ok("no problems".into());
    }
    let mut path = path_of(uri);
    if let Some(rel) = crate::go_rel(cwd, &path)
        && !rel.starts_with("..")
    {
        path = rel;
    }
    let out: Vec<String> = ds
        .iter()
        .map(|d| {
            let mut code = d
                .code
                .as_deref()
                .map_or("", RawValue::get)
                .trim_matches('"')
                .to_string();
            if !code.is_empty() {
                code = format!(" TS{}", code.trim_start_matches("TS"));
            }
            let sev = match d.severity {
                1 => "error",
                2 => "warning",
                3 => "info",
                4 => "hint",
                _ => "",
            };
            format!(
                "{path}:{}:{}: {sev}{code}: {}",
                d.range.start.line + 1,
                d.range.start.character + 1,
                d.message.replace('\n', " ")
            )
        })
        .collect();
    Ok(limit(&out, 200))
}

fn find_symbol(
    ctx: &Ctx,
    s: &Server,
    _: &str,
    _: Position,
    a: &Ask,
    cwd: &str,
) -> Result<String, String> {
    let r = s.call(
        ctx,
        "workspace/symbol",
        &serde_json::json!({ "query": a.query }),
    )?;
    let syms: Vec<DocSymbol> = decode(r.as_deref())?;
    let mut out = Vec::new();
    for d in &syms {
        let Some(l) = &d.location else {
            continue;
        };
        let (u, p) = l.at();
        let path = path_of(&u);
        if !path.starts_with(&s.root) || path.contains("/node_modules/") {
            continue;
        }
        out.push(format!(
            "{} {}  {}",
            kind_name(d.kind),
            d.name,
            where_(&u, p, cwd)
        ));
    }
    if out.is_empty() {
        return Ok("none".into());
    }
    out.sort();
    Ok(limit(&out, 100))
}

/// LSP's SymbolKind in a word.
pub(crate) fn kind_name(k: i64) -> &'static str {
    const NAMES: [&str; 27] = [
        "",
        "file",
        "module",
        "namespace",
        "package",
        "class",
        "method",
        "property",
        "field",
        "constructor",
        "enum",
        "interface",
        "function",
        "variable",
        "constant",
        "string",
        "number",
        "boolean",
        "array",
        "object",
        "key",
        "null",
        "enum-member",
        "struct",
        "event",
        "operator",
        "type-parameter",
    ];
    if k > 0 && (k as usize) < NAMES.len() {
        return NAMES[k as usize];
    }
    "symbol"
}

#[cfg(test)]
mod tests {
    use super::*;

    // ts_test.go TestWordIndex
    #[test]
    fn word_indexes() {
        for (s, w, want) in [
            ("const userName = user;", "user", Some(17)),
            ("user.id", "user", Some(0)),
            ("$user + user", "user", Some(8)),
            ("users", "user", None),
        ] {
            assert_eq!(word_index(s, w), want, "word_index({s:?}, {w:?})");
        }
    }

    // ts_test.go TestPositionCountsUTF16: LSP columns count UTF-16 units:
    // an emoji before a name is two.
    #[test]
    fn position_counts_utf16() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.ts");
        std::fs::write(&f, "const a = 1;\nconst s = \"😀\"; foo();\n").unwrap();
        let f = f.to_str().unwrap();
        let ask = |line, symbol: &str, character| Ask {
            line,
            symbol: symbol.into(),
            character,
            ..Default::default()
        };
        assert_eq!(
            position(f, &ask(2, "foo", 0)),
            Ok(Position {
                line: 1,
                character: 16
            })
        );
        assert!(
            position(f, &ask(2, "bar", 0)).is_err(),
            "found a name that isn't on the line"
        );
        assert_eq!(position(f, &ask(1, "", 7)).unwrap().character, 6);
    }

    // ts_test.go TestDecodeLocations
    #[test]
    fn decodes_locations() {
        let one = decode_locations(
            r#"{"uri":"file:///a.ts","range":{"start":{"line":2,"character":3},"end":{"line":2,"character":4}}}"#,
        )
        .unwrap();
        let links = decode_locations(
            r#"[{"targetUri":"file:///b.ts","targetSelectionRange":{"start":{"line":5,"character":1},"end":{"line":5,"character":2}}}]"#,
        )
        .unwrap();
        assert_eq!(
            one[0].at(),
            (
                "file:///a.ts".into(),
                Position {
                    line: 2,
                    character: 3
                }
            )
        );
        assert_eq!(
            links[0].at(),
            (
                "file:///b.ts".into(),
                Position {
                    line: 5,
                    character: 1
                }
            )
        );
        assert!(decode_locations("null").unwrap().is_empty());
    }

    // ts_test.go TestMarkup
    #[test]
    fn markups() {
        for (input, want) in [
            (r#"{"kind":"markdown","value":"**x**"}"#, "**x**"),
            (r#""plain""#, "plain"),
            (r#"["a",{"language":"ts","value":"b"}]"#, "a\n\nb"),
        ] {
            assert_eq!(markup(input), want, "markup({input})");
        }
    }

    #[test]
    fn limits_and_kinds() {
        let lines: Vec<String> = (0..5).map(|i| i.to_string()).collect();
        assert_eq!(limit(&lines, 3), "0\n1\n2\n… and 2 more");
        assert_eq!((kind_name(12), kind_name(99)), ("function", "symbol"));
        assert_eq!(op_named("find_symbol").map(|o| o.needs), Some("query"));
    }
}
