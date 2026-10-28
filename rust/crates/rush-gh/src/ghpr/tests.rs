// Ported from internal/ghpr/ghpr_test.go and branches_test.go.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use super::act::parse_comments;
use super::*;

#[test]
fn parse_reads_checks_reviews_and_who_waits() {
    let b = br#"[{"number":7,"title":"t","state":"OPEN","author":{"login":"me"},"reviewDecision":"REVIEW_REQUIRED",
	"reviewRequests":[{"__typename":"User","login":"sam"},{"__typename":"Team","name":"Web","slug":"web-team"}],
	"latestReviews":[{"author":{"login":"kim"},"state":"APPROVED","body":"lgtm"}],
	"statusCheckRollup":[
	 {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},
	 {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"FAILURE"},
	 {"__typename":"CheckRun","name":"e2e","status":"IN_PROGRESS","conclusion":""},
	 {"__typename":"CheckRun","name":"skip","status":"COMPLETED","conclusion":"SKIPPED"},
	 {"__typename":"StatusContext","context":"ci/old","state":"ERROR"}]}]"#;
    let prs = parse(b).unwrap();
    assert_eq!(prs.len(), 1);
    let p = &prs[0];
    assert_eq!(p.counts(), (2, 2, 1));
    assert_eq!(p.waiting, ["sam", "web-team"]);
    assert_eq!(p.reviews.len(), 1);
    assert_eq!(p.reviews[0].who, "kim");
    assert_eq!(p.reviews[0].body, "lgtm");
    assert_eq!(p.author, "me");
}

#[test]
fn parse_comments_reads_who_what_when() {
    let cs = parse_comments(
        br#"{"comments":[{"author":{"login":"kim"},"body":"nice","createdAt":"2026-10-14T10:00:00Z"}]}"#,
    )
    .unwrap();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].who, "kim");
    assert_eq!(cs[0].body, "nice");
    assert_ne!(cs[0].at, 0);
}

// GitHub failing is asked again later each time in a row, and not for ten
// minutes once it says it's asked too much.
#[test]
fn retry_in_backs_off() {
    let min = |m: u64| Duration::from_secs(60 * m);
    for (fails, err, want) in [
        (0, "", BRANCH_EVERY),
        (1, "HTTP 504", 2 * BRANCH_EVERY),
        (3, "HTTP 502", 8 * BRANCH_EVERY),
        (9, "HTTP 504", min(15)),
        (
            1,
            "HTTP 403: You have exceeded a secondary rate limit.",
            min(10),
        ),
    ] {
        assert_eq!(retry_in(fails, err), want, "retry_in({fails}, {err:?})");
    }
}

const FAKE_LIST: &str = r#"[
 {"number":12,"title":"old try","state":"CLOSED","headRefName":"rush/abc","headRefOid":"aaa","isDraft":false,"statusCheckRollup":[],"reviewDecision":"","mergedAt":null,"url":"u12"},
 {"number":14,"title":"the fix","state":"MERGED","headRefName":"rush/abc","headRefOid":"bbb","isDraft":false,
  "statusCheckRollup":[{"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"SUCCESS"}],
  "reviewDecision":"APPROVED","mergedAt":"2026-10-20T10:00:00Z","url":"u14"},
 {"number":15,"title":"wip","state":"OPEN","headRefName":"rush/def","headRefOid":"ccc","isDraft":true,
  "statusCheckRollup":[{"__typename":"CheckRun","name":"build","status":"IN_PROGRESS"},{"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE"}],
  "reviewDecision":"CHANGES_REQUESTED","mergedAt":null,"url":"u15"}
]"#;

// A session's pull request is found by its branch, through one gh pr list
// for the repository, asked in the background and kept.
#[test]
fn by_branch_with_a_fake_gh() {
    let calls = Arc::new(AtomicUsize::new(0));
    let bad = Arc::new(Mutex::new(Vec::new()));
    let (c, b) = (calls.clone(), bad.clone());
    let gh = Gh::new(move |dir: &str, args: &[&str]| {
        c.fetch_add(1, Ordering::SeqCst);
        if dir != "/repo" || !args.join(" ").contains("pr list --state all --limit 100") {
            b.lock().unwrap().push(format!("asked {dir} {args:?}"));
        }
        Ok(FAKE_LIST.as_bytes().to_vec())
    });
    let (tx, rx) = mpsc::channel();
    gh.on_change(move |root| {
        let _ = tx.send(root.to_string());
    });
    assert!(!gh.by_branch("/repo").1, "known before GitHub answered");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        "/repo",
        "GitHub's answer never came in"
    );
    let (by, ok) = gh.by_branch("/repo");
    assert!(ok);
    let by = by.unwrap();
    let merged = by.get("rush/abc").expect("rush/abc");
    assert!(
        merged.number == 14
            && merged.merged()
            && merged.review == "approved"
            && merged.checks == "pass"
            && merged.merged_at != 0
            && merged.head_oid == "bbb",
        "rush/abc: {merged:?} (the merged one beats the closed one)"
    );
    let draft = gh.for_branch("/repo", "rush/def").unwrap();
    assert!(
        draft.state == "draft"
            && draft.checks == "fail"
            && draft.failing == "test"
            && draft.review == "changes"
            && draft.url == "u15",
        "rush/def: {draft:?}"
    );
    assert!(
        gh.for_branch("/repo", "nope").is_none(),
        "a branch with no pull request has one"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "asked again within a minute"
    );
    assert!(bad.lock().unwrap().is_empty(), "{:?}", bad.lock().unwrap());
}

// refresh asks again however recently GitHub answered.
#[test]
fn refresh_asks_again() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let gh = Gh::new(move |_: &str, _: &[&str]| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(b"[]".to_vec())
    });
    let (tx, rx) = mpsc::channel();
    gh.on_change(move |_| {
        let _ = tx.send(());
    });
    gh.look_branches("/repo").unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    gh.refresh("/repo");
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(gh.by_branch("/repo").1);
}

// gh missing says so, and isn't taken for "no pull requests".
#[test]
fn by_branch_without_gh() {
    let gh = Gh::new(|_: &str, _: &[&str]| Err(RunError::Other("exec: not found".into())));
    assert!(gh.look_branches("/repo").is_err(), "no error without gh");
    assert!(!gh.by_branch("/repo").1, "ok without gh");
}

// The Projects page's list is asked once a minute however many ask for it.
#[test]
fn look_shares_the_answer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let gh = Gh::new(move |_: &str, args: &[&str]| {
        if args[0] == "pr" {
            c.fetch_add(1, Ordering::SeqCst);
        }
        Err(RunError::Other("not on GitHub".into()))
    });
    for _ in 0..3 {
        assert!(!gh.look("/src/r").err.is_empty(), "no error said");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "gh asked again");
}

// A remote on a renamed repository's old name still reaches it, but a
// search by that name finds nothing: the list asks by the name it has now.
#[test]
fn list_searches_by_the_repositorys_name_now() {
    let lists = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let l = lists.clone();
    let gh = Gh::new(move |_: &str, args: &[&str]| {
        if args[0] == "repo" {
            return Ok(b"0xdeafcafe/rush\n".to_vec());
        }
        if args[0] == "pr" {
            l.lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
        }
        Ok(b"[]".to_vec())
    });
    let r = gh.list("/src/r");
    assert_eq!(r.err, "");
    let lists = lists.lock().unwrap();
    assert!(!lists.is_empty(), "no list asked");
    for a in lists.iter() {
        assert_eq!(
            a[a.len() - 2..].join(" "),
            "--repo 0xdeafcafe/rush",
            "searched without the repository's name now: {a:?}"
        );
    }
}

// Yours come first, then the most lately touched; one both yours and asked
// of you is listed once; asked of you by name, it says you.
#[test]
fn list_puts_yours_first() {
    let gh = Gh::new(|_: &str, args: &[&str]| {
        let out = match args {
            ["api", "user", ..] => "kim\n",
            ["repo", ..] => "",
            _ if args.contains(&"author:@me") => {
                r#"[{"number":1,"updatedAt":"2026-10-01T00:00:00Z"},{"number":2,"updatedAt":"2026-10-03T00:00:00Z","reviewRequests":[{"login":"kim"}]}]"#
            }
            _ => {
                r#"[{"number":2,"updatedAt":"2026-10-03T00:00:00Z"},{"number":3,"updatedAt":"2026-10-09T00:00:00Z"}]"#
            }
        };
        Ok(out.as_bytes().to_vec())
    });
    let r = gh.list("/src/r");
    let got: Vec<_> = r
        .prs
        .iter()
        .map(|p| (p.number, p.mine, p.asked_you))
        .collect();
    assert_eq!(got, [(2, true, true), (1, true, false), (3, false, true)]);
    assert_eq!(r.prs[0].waiting, ["you"]);
}

// A refused merge comes back as a MergeError; the args are gh's.
#[test]
fn merge_says_why_it_was_refused() {
    let asked = Arc::new(Mutex::new(String::new()));
    let a = asked.clone();
    let gh = Gh::new(move |_: &str, args: &[&str]| {
        *a.lock().unwrap() = args.join(" ");
        Err(RunError::Exit {
            status: "exit status 1".into(),
            stderr: "the base branch policy prohibits the merge".into(),
        })
    });
    match gh.merge("/r", 9, "squash", true) {
        Err(Error::Merge(m)) => assert!(m.admin),
        e => panic!("{e:?}"),
    }
    assert_eq!(*asked.lock().unwrap(), "pr merge 9 --squash --admin");
    gh.approve("/r", 9, "").unwrap_err();
    assert_eq!(*asked.lock().unwrap(), "pr review 9 --approve");
}
