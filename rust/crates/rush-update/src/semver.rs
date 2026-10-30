//! Which of two versions is newer, by semver's order: 0.2.0 after
//! 0.2.0-nightly.20261029 after 0.1.0. Release tags are vMAJOR.MINOR.PATCH
//! and nightlies add -nightly.YYYYMMDD (.github/workflows/release.yml).

use std::cmp::Ordering;

/// Whether latest is newer than cur. A version that isn't semver is never
/// newer, so an odd tag doesn't nag.
pub fn newer(cur: &str, latest: &str) -> bool {
    match (Version::parse(cur), Version::parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    // Numbers come before words, as semver orders them.
    Num(u64),
    Word(String),
}

#[derive(PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Vec<Ident>,
}

impl Version {
    fn parse(v: &str) -> Option<Version> {
        let v = v.trim();
        let v = v.strip_prefix('v').unwrap_or(v);
        let v = v.split_once('+').map_or(v, |(v, _build)| v);
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        let mut nums = core.split('.').map(|n| {
            (!n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
                .then(|| n.parse().ok())
                .flatten()
        });
        let core = [nums.next()??, nums.next()??, nums.next()??];
        if nums.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|i| {
                    if i.is_empty() {
                        None
                    } else if i.bytes().all(|c| c.is_ascii_digit()) {
                        i.parse().ok().map(Ident::Num)
                    } else {
                        Some(Ident::Word(i.to_string()))
                    }
                })
                .collect::<Option<_>>()?,
        };
        Some(Version { core, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        self.core.cmp(&o.core).then_with(|| {
            // A release comes after its prereleases.
            match (self.pre.is_empty(), o.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&o.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
