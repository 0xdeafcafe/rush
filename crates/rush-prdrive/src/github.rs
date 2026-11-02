//! A pull request as GitHub says it is now, asked through gh's GraphQL and
//! REST: its reviews, threads, comments, checks, the checks its base
//! requires and its open code-scanning alerts.
//! Ported from internal/prdrive/github.go.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use rush_state::Time;
use rush_watch::gh::{self as wgh, nd};
use rush_watch::go::quote;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// Who wrote something: a GitHub login, and whether the account is a bot
/// (an app), which a person can't be however they name themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Actor {
    pub login: String,
    pub bot: bool,
}

impl Actor {
    pub fn person(login: &str) -> Actor {
        Actor {
            login: login.into(),
            bot: false,
        }
    }

    pub fn bot(login: &str) -> Actor {
        Actor {
            login: login.into(),
            bot: true,
        }
    }
}

/// One comment in a review thread, or on the pull request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comment {
    pub id: i64,
    pub author: Actor,
    pub body: String,
    pub at: Time,
    pub url: String,
}

/// A review thread: comments anchored to a line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Thread {
    pub id: String,
    pub resolved: bool,
    pub outdated: bool,
    pub path: String,
    pub line: i64,
    pub comments: Vec<Comment>,
}

/// A submitted review.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Review {
    pub author: Actor,
    /// APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED
    pub state: String,
    pub at: Time,
    pub commit: String,
    pub body: String,
}

/// One check on the head commit, the latest of its name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Check {
    pub name: String,
    pub workflow: String,
    /// passed, failed, cancelled, pending or skipped
    pub state: String,
    pub started: Time,
    pub run: i64,
    pub job: i64,
    pub url: String,
    pub required: bool,
}

/// A pull request as GitHub says it is now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Raw {
    pub repo: String,
    pub title: String,
    pub url: String,
    pub state: String,
    pub author: String,
    pub number: i64,
    pub draft: bool,
    pub head_ref: String,
    pub head: String,
    pub base: String,
    pub head_repo: String,
    /// When the head commit was pushed.
    pub head_at: Time,
    /// MERGEABLE, CONFLICTING, UNKNOWN
    pub mergeable: String,
    pub body: String,
    pub reviews: Vec<Review>,
    pub threads: Vec<Thread>,
    /// On the pull request itself.
    pub comments: Vec<Comment>,
    pub checks: Vec<Check>,
    /// Open code-scanning alerts.
    pub alerts: Vec<String>,
    /// The checks the base requires, by name.
    pub required: HashSet<String>,
}

static TRANSIENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"HTTP 50[234]|timeout|EOF|connection reset").unwrap());

/// Runs gh with args and gives its output, asking again twice when GitHub
/// stumbles. Tests set gh with rush_watch::gh::set_thread_gh.
pub fn gh(args: &[&str]) -> Result<Vec<u8>, String> {
    for attempt in 0..3u64 {
        let out = wgh::run(None, args);
        let Some(err) = out.err else {
            return Ok(out.stdout);
        };
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !TRANSIENT.is_match(&msg) || attempt == 2 {
            if err == rush_gh::ghpr::RunError::NotFound {
                return Err("gh isn't installed".into());
            }
            let msg = match msg.find('\n') {
                Some(i) if i > 0 => msg[..i].to_string(),
                _ => msg,
            };
            let what = args[..args.len().min(2)].join(" ");
            let why = if msg.is_empty() { err.to_string() } else { msg };
            return Err(format!("gh {what}: {why}"));
        }
        if !rush_watch::stop::sleep(Duration::from_secs(2 + attempt * 4)) {
            return Err("context canceled".into());
        }
    }
    Err("unreachable".into())
}

/// Runs a GraphQL query, its values inlined, and reads its data.
pub(crate) fn gql<T: DeserializeOwned>(query: &str) -> Result<T, String> {
    let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let b = gh(&["api", "graphql", "-f", &format!("query={q}")])?;
    #[derive(Deserialize)]
    struct Msg {
        #[serde(default, deserialize_with = "nd")]
        message: String,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        data: Option<Box<serde_json::value::RawValue>>,
        #[serde(default, deserialize_with = "nd")]
        errors: Vec<Msg>,
    }
    let r: Answer = decode(&b)?;
    if let Some(e) = r.errors.first() {
        return Err(format!("GitHub says: {}", e.message));
    }
    match r.data {
        Some(d) => decode(d.get().as_bytes()),
        None => decode(b"null"),
    }
}

fn decode<T: DeserializeOwned>(b: &[u8]) -> Result<T, String> {
    rush_state::json::from_slice(b).map_err(|e| e.to_string())
}

/// A time as GitHub writes it, null read as the zero time.
fn t0<'de, D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.and_then(|s| Time::parse(&s)).unwrap_or_default())
}

#[derive(Deserialize, Default)]
struct GqlActor {
    #[serde(rename = "__typename", default, deserialize_with = "nd")]
    typename: String,
    #[serde(default, deserialize_with = "nd")]
    login: String,
}

fn actor(a: &Option<GqlActor>) -> Actor {
    match a {
        None => Actor::person("ghost"),
        Some(a) => Actor {
            login: a.login.clone(),
            bot: a.typename == "Bot",
        },
    }
}

#[derive(Deserialize, Default)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct Nodes<T> {
    #[serde(default, deserialize_with = "nd")]
    nodes: Vec<T>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GqlComment {
    #[serde(default, deserialize_with = "nd")]
    database_id: i64,
    #[serde(default)]
    author: Option<GqlActor>,
    #[serde(default, deserialize_with = "nd")]
    body: String,
    #[serde(default, deserialize_with = "nd")]
    url: String,
    #[serde(default, deserialize_with = "t0")]
    created_at: Time,
}

impl GqlComment {
    fn comment(&self) -> Comment {
        Comment {
            id: self.database_id,
            author: actor(&self.author),
            body: self.body.clone(),
            at: self.created_at,
            url: self.url.clone(),
        }
    }
}

/// The start of every query of pull request n of owner/name.
fn query_base(owner: &str, name: &str, n: i64) -> String {
    format!(
        "repository(owner:{},name:{}){{pullRequest(number:{n}){{",
        quote(owner),
        quote(name)
    )
}

/// Pull request n of repo as GitHub says now.
pub fn fetch(repo: &str, n: i64) -> Result<Raw, String> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(format!("{} isn't owner/name", quote(repo)));
    };
    let base = query_base(owner, name, n);
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct GqlReview {
        #[serde(default)]
        author: Option<GqlActor>,
        #[serde(default, deserialize_with = "nd")]
        state: String,
        #[serde(default, deserialize_with = "nd")]
        body: String,
        #[serde(default, deserialize_with = "t0")]
        submitted_at: Time,
        #[serde(default)]
        commit: Option<Oid>,
    }
    #[derive(Deserialize, Default)]
    struct Oid {
        #[serde(default, deserialize_with = "nd")]
        oid: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Dates {
        #[serde(default, deserialize_with = "t0")]
        pushed_date: Time,
        #[serde(default, deserialize_with = "t0")]
        committed_date: Time,
    }
    #[derive(Deserialize, Default)]
    struct CommitNode {
        #[serde(default, deserialize_with = "nd")]
        commit: Dates,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct HeadRepo {
        #[serde(default, deserialize_with = "nd")]
        name_with_owner: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Pull {
        #[serde(default, deserialize_with = "nd")]
        title: String,
        #[serde(default, deserialize_with = "nd")]
        url: String,
        #[serde(default, deserialize_with = "nd")]
        state: String,
        #[serde(default, deserialize_with = "nd")]
        head_ref_name: String,
        #[serde(default, deserialize_with = "nd")]
        head_ref_oid: String,
        #[serde(default, deserialize_with = "nd")]
        base_ref_name: String,
        #[serde(default, deserialize_with = "nd")]
        mergeable: String,
        #[serde(default, deserialize_with = "nd")]
        body: String,
        #[serde(default, deserialize_with = "nd")]
        is_draft: bool,
        #[serde(default)]
        author: Option<GqlActor>,
        #[serde(default)]
        head_repository: Option<HeadRepo>,
        #[serde(default, deserialize_with = "nd")]
        reviews: Nodes<GqlReview>,
        #[serde(default, deserialize_with = "nd")]
        comments: Nodes<GqlComment>,
        #[serde(default, deserialize_with = "nd")]
        commits: Nodes<CommitNode>,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Repository {
        #[serde(default)]
        pull_request: Option<Pull>,
    }
    #[derive(Deserialize, Default)]
    struct Top {
        #[serde(default, deserialize_with = "nd")]
        repository: Repository,
    }
    let top: Option<Top> = gql(&format!(
        "query{{{base}
		title url state isDraft author{{__typename login}} headRefName headRefOid baseRefName
		headRepository{{nameWithOwner}} mergeable body
		reviews(last:100){{nodes{{author{{__typename login}} state body submittedAt commit{{oid}}}}}}
		comments(last:100){{nodes{{databaseId author{{__typename login}} body url createdAt}}}}
		commits(last:1){{nodes{{commit{{pushedDate committedDate}}}}}}
	}}}}}}"
    ))?;
    let Some(p) = top.unwrap_or_default().repository.pull_request else {
        return Err(format!("{repo} has no pull request #{n}"));
    };
    let mut r = Raw {
        repo: repo.into(),
        number: n,
        title: p.title,
        url: p.url,
        state: p.state,
        draft: p.is_draft,
        author: actor(&p.author).login,
        head_ref: p.head_ref_name,
        head: p.head_ref_oid,
        base: p.base_ref_name,
        mergeable: p.mergeable,
        body: p.body,
        ..Default::default()
    };
    if let Some(h) = p.head_repository {
        r.head_repo = h.name_with_owner;
    }
    if let Some(c) = p.commits.nodes.first() {
        r.head_at = c.commit.pushed_date;
        if r.head_at.is_zero() {
            r.head_at = c.commit.committed_date;
        }
    }
    for v in &p.reviews.nodes {
        r.reviews.push(Review {
            author: actor(&v.author),
            state: v.state.clone(),
            at: v.submitted_at,
            body: v.body.clone(),
            commit: v.commit.as_ref().map(|c| c.oid.clone()).unwrap_or_default(),
        });
    }
    for c in &p.comments.nodes {
        r.comments.push(c.comment());
    }
    r.threads = fetch_threads(&base)?;
    r.checks = fetch_checks(&base)?;
    let (rb, branch) = (repo.to_string(), r.base.clone());
    r.required = remember(
        &format!("required {repo} {}", r.base),
        REQUIRED_FOR,
        move || required(&rb, &branch),
    );
    for c in &mut r.checks {
        c.required = r.required.contains(&c.name);
    }
    let rb = repo.to_string();
    r.alerts = remember(
        &format!("alerts {repo} {n} {}", r.head),
        ALERTS_FOR,
        move || alerts(&rb, n),
    );
    Ok(r)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    #[serde(default, deserialize_with = "nd")]
    has_next_page: bool,
    #[serde(default, deserialize_with = "nd")]
    end_cursor: String,
}

fn after(cursor: &str) -> String {
    if cursor.is_empty() {
        String::new()
    } else {
        format!(",after:{}", quote(cursor))
    }
}

fn fetch_threads(base: &str) -> Result<Vec<Thread>, String> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        #[serde(default, deserialize_with = "nd")]
        id: String,
        #[serde(default, deserialize_with = "nd")]
        is_resolved: bool,
        #[serde(default, deserialize_with = "nd")]
        is_outdated: bool,
        #[serde(default, deserialize_with = "nd")]
        path: String,
        #[serde(default, deserialize_with = "nd")]
        line: i64,
        #[serde(default, deserialize_with = "nd")]
        comments: Nodes<GqlComment>,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Threads {
        #[serde(default, deserialize_with = "nd")]
        page_info: PageInfo,
        #[serde(default, deserialize_with = "nd")]
        nodes: Vec<Node>,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Pull {
        #[serde(default, deserialize_with = "nd")]
        review_threads: Threads,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Repository {
        #[serde(default, deserialize_with = "nd")]
        pull_request: Pull,
    }
    #[derive(Deserialize, Default)]
    struct Page {
        #[serde(default, deserialize_with = "nd")]
        repository: Repository,
    }
    let mut out = Vec::new();
    let mut cursor = String::new();
    loop {
        let page: Option<Page> = gql(&format!(
            "query{{{base}reviewThreads(first:100{}){{
			pageInfo{{hasNextPage endCursor}}
			nodes{{id isResolved isOutdated path line
				comments(first:100){{nodes{{databaseId author{{__typename login}} body url createdAt}}}}}}
		}}}}}}}}",
            after(&cursor)
        ))?;
        let rt = page
            .unwrap_or_default()
            .repository
            .pull_request
            .review_threads;
        for t in rt.nodes {
            out.push(Thread {
                id: t.id,
                resolved: t.is_resolved,
                outdated: t.is_outdated,
                path: t.path,
                line: t.line,
                comments: t.comments.nodes.iter().map(GqlComment::comment).collect(),
            });
        }
        if !rt.page_info.has_next_page {
            return Ok(out);
        }
        cursor = rt.page_info.end_cursor;
    }
}

static JOB_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/job/([0-9]+)").unwrap());

fn fetch_checks(base: &str) -> Result<Vec<Check>, String> {
    #[derive(Deserialize, Default)]
    struct Workflow {
        #[serde(default, deserialize_with = "nd")]
        name: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct WorkflowRun {
        #[serde(default, deserialize_with = "nd")]
        database_id: i64,
        #[serde(default, deserialize_with = "nd")]
        workflow: Workflow,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Suite {
        #[serde(default)]
        workflow_run: Option<WorkflowRun>,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        #[serde(rename = "__typename", default, deserialize_with = "nd")]
        typename: String,
        #[serde(default, deserialize_with = "nd")]
        name: String,
        #[serde(default, deserialize_with = "nd")]
        status: String,
        #[serde(default, deserialize_with = "nd")]
        conclusion: String,
        #[serde(default, deserialize_with = "nd")]
        details_url: String,
        #[serde(default, deserialize_with = "t0")]
        started_at: Time,
        #[serde(default, deserialize_with = "nd")]
        context: String,
        #[serde(default, deserialize_with = "nd")]
        state: String,
        #[serde(default, deserialize_with = "nd")]
        target_url: String,
        #[serde(default, deserialize_with = "t0")]
        created_at: Time,
        #[serde(default)]
        check_suite: Option<Suite>,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Contexts {
        #[serde(default, deserialize_with = "nd")]
        page_info: PageInfo,
        #[serde(default, deserialize_with = "nd")]
        nodes: Vec<Node>,
    }
    #[derive(Deserialize, Default)]
    struct Rollup {
        #[serde(default, deserialize_with = "nd")]
        contexts: Contexts,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Commit {
        #[serde(default)]
        status_check_rollup: Option<Rollup>,
    }
    #[derive(Deserialize, Default)]
    struct CommitNode {
        #[serde(default, deserialize_with = "nd")]
        commit: Commit,
    }
    #[derive(Deserialize, Default)]
    struct Pull {
        #[serde(default, deserialize_with = "nd")]
        commits: Nodes<CommitNode>,
    }
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Repository {
        #[serde(default, deserialize_with = "nd")]
        pull_request: Pull,
    }
    #[derive(Deserialize, Default)]
    struct Page {
        #[serde(default, deserialize_with = "nd")]
        repository: Repository,
    }
    let mut latest: HashMap<String, Check> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut cursor = String::new();
    loop {
        let page: Option<Page> = gql(&format!(
            "query{{{base}commits(last:1){{nodes{{commit{{statusCheckRollup{{contexts(first:100{}){{
			pageInfo{{hasNextPage endCursor}}
			nodes{{__typename
				... on CheckRun{{name status conclusion detailsUrl startedAt
					checkSuite{{workflowRun{{databaseId workflow{{name}}}}}}}}
				... on StatusContext{{context state targetUrl createdAt}}}}
		}}}}}}}}}}}}}}}}",
            after(&cursor)
        ))?;
        let mut nodes = page
            .unwrap_or_default()
            .repository
            .pull_request
            .commits
            .nodes;
        let Some(rollup) = nodes
            .first_mut()
            .and_then(|n| n.commit.status_check_rollup.take())
        else {
            break;
        };
        let ctxs = rollup.contexts;
        for c in ctxs.nodes {
            let ch = if c.typename == "CheckRun" {
                let mut ch = Check {
                    name: c.name,
                    state: run_state(&c.status, &c.conclusion).into(),
                    started: c.started_at,
                    url: c.details_url,
                    ..Default::default()
                };
                if let Some(run) = c.check_suite.and_then(|s| s.workflow_run) {
                    ch.workflow = run.workflow.name;
                    ch.run = run.database_id;
                }
                if let Some(m) = JOB_ID.captures(&ch.url) {
                    ch.job = m[1].parse().unwrap_or(0);
                }
                ch
            } else {
                let st = match c.state.as_str() {
                    "SUCCESS" => "passed",
                    "PENDING" | "EXPECTED" => "pending",
                    _ => "failed",
                };
                Check {
                    name: c.context,
                    state: st.into(),
                    started: c.created_at,
                    url: c.target_url,
                    ..Default::default()
                }
            };
            let key = format!("{}\x00{}", ch.workflow, ch.name);
            match latest.get(&key) {
                None => {
                    order.push(key.clone());
                    latest.insert(key, ch);
                }
                Some(old) if !ch.started.before(old.started) => {
                    latest.insert(key, ch);
                }
                Some(_) => {}
            }
        }
        if !ctxs.page_info.has_next_page {
            break;
        }
        cursor = ctxs.page_info.end_cursor;
    }
    Ok(order.iter().filter_map(|k| latest.remove(k)).collect())
}

fn run_state(status: &str, conclusion: &str) -> &'static str {
    if status != "COMPLETED" {
        return "pending";
    }
    match conclusion {
        "SUCCESS" | "NEUTRAL" => "passed",
        "SKIPPED" | "STALE" => "skipped",
        "CANCELLED" => "cancelled",
        _ => "failed",
    }
}

/// The checks base's rules and protection require, by name.
pub fn required(repo: &str, base: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(b) = gh(&["api", &format!("repos/{repo}/rules/branches/{base}")]) {
        #[derive(Deserialize, Default)]
        struct Ctx {
            #[serde(default, deserialize_with = "nd")]
            context: String,
        }
        #[derive(Deserialize, Default)]
        struct Params {
            #[serde(default, deserialize_with = "nd")]
            required_status_checks: Vec<Ctx>,
        }
        #[derive(Deserialize)]
        struct Rule {
            #[serde(rename = "type", default, deserialize_with = "nd")]
            kind: String,
            #[serde(default, deserialize_with = "nd")]
            parameters: Params,
        }
        if let Ok(rules) = decode::<Vec<Rule>>(&b) {
            for r in rules {
                if r.kind == "required_status_checks" {
                    for c in r.parameters.required_status_checks {
                        out.insert(c.context);
                    }
                }
            }
        }
    }
    if let Ok(b) = gh(&["api", &format!("repos/{repo}/branches/{base}/protection")]) {
        #[derive(Deserialize, Default)]
        struct Checks {
            #[serde(default, deserialize_with = "nd")]
            contexts: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Protection {
            #[serde(default, deserialize_with = "nd")]
            required_status_checks: Checks,
        }
        if let Ok(p) = decode::<Protection>(&b) {
            out.extend(p.required_status_checks.contexts);
        }
    }
    out
}

/// A base's required checks barely change, and a head's code-scanning
/// alerts settle once its scan ran: fetch asks each again only after
/// these, so a busy pull request doesn't spend the rate limit re-asking.
const REQUIRED_FOR: Duration = Duration::from_secs(30 * 60);
const ALERTS_FOR: Duration = Duration::from_secs(2 * 60);

type Answer = (Time, Box<dyn Any + Send>);

static REMEMBERED: LazyLock<Mutex<HashMap<String, Answer>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// ask's answer for key, asked again only once it is older than ttl.
pub fn remember<T: Clone + Send + 'static>(key: &str, ttl: Duration, ask: impl FnOnce() -> T) -> T {
    {
        let m = REMEMBERED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = m.get(key)
            && Time::now().sub_nanos(*at) < ttl.as_nanos() as i128
            && let Some(v) = v.downcast_ref::<T>()
        {
            return v.clone();
        }
    }
    let val = ask();
    REMEMBERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_string(), (Time::now(), Box::new(val.clone())));
    val
}

fn alerts(repo: &str, n: i64) -> Vec<String> {
    let Ok(b) = gh(&[
        "api",
        &format!(
            "repos/{repo}/code-scanning/alerts?ref=refs/pull/{n}/merge&state=open&per_page=50"
        ),
    ]) else {
        return Vec::new();
    };
    #[derive(Deserialize, Default)]
    struct Rule {
        #[serde(default, deserialize_with = "nd")]
        id: String,
    }
    #[derive(Deserialize, Default)]
    struct Location {
        #[serde(default, deserialize_with = "nd")]
        path: String,
        #[serde(default, deserialize_with = "nd")]
        start_line: i64,
    }
    #[derive(Deserialize, Default)]
    struct Instance {
        #[serde(default, deserialize_with = "nd")]
        location: Location,
    }
    #[derive(Deserialize)]
    struct Alert {
        #[serde(default, deserialize_with = "nd")]
        rule: Rule,
        #[serde(default, deserialize_with = "nd")]
        most_recent_instance: Instance,
    }
    let Ok(list) = decode::<Vec<Alert>>(&b) else {
        return Vec::new();
    };
    list.iter()
        .map(|a| {
            let l = &a.most_recent_instance.location;
            format!("{} {}:{}", a.rule.id, l.path, l.start_line)
        })
        .collect()
}

/// The login gh is signed in as.
pub fn me_now() -> Result<String, String> {
    let r = gh(&["api", "user", "--jq", ".login"]);
    r.map(|b| String::from_utf8_lossy(&b).trim().to_string())
}
