//! A small syntax highlighter: one pass over a line's bytes, no regular
//! expressions, state carried from line to line for strings and block
//! comments. Port of photon/hl
//! (~/go/pkg/mod/github.com/0xdeafcafe/photon@v0.0.0-20261008232558-b01e070b3366/hl).
//!
//! API (fixed; other crates build against it):
//! - `Lang::for_name(name)`: a language from a fence tag, file name or
//!   interpreter; `None` when there's none to highlight.
//! - `lang.line(&mut state, line, |start, end, class| …)`: classes of one
//!   line's byte ranges; untouched bytes are plain.
//! - `State::fence()`: inside a markdown fence, the fenced language.

/// What a span of code is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
}

/// A language the highlighter knows.
#[derive(Debug)]
pub struct Lang {
    pub name: &'static str,
}

/// What carries from one line to the next: an open string, block comment
/// or markdown fence.
#[derive(Clone, Debug, Default)]
pub struct State {
    fence: Option<&'static Lang>,
}

impl State {
    /// Inside a markdown fence, its language.
    pub fn fence(&self) -> Option<&'static Lang> {
        self.fence
    }
}

impl Lang {
    /// The language of a fence tag, file name or interpreter.
    pub fn for_name(_name: &str) -> Option<&'static Lang> {
        None // TODO(port): photon/hl For
    }

    /// Calls `out(start, end, class)` for each classified byte range of `s`.
    pub fn line(&self, _st: &mut State, _s: &str, _out: impl FnMut(usize, usize, Class)) {}
}
