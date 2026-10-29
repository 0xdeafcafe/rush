//! GitHub through the gh command (`ghpr`: pull requests, checks, reviews)
//! and holding an agent's `git push` until its branch is quiet
//! (`pushlater`). Every gh call goes through [`ghpr::Runner`], so tests use
//! fakes.

pub mod ghpr;
pub mod pushlater;
mod rfc3339;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
