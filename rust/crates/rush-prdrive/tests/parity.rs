//! rush pr and rush watch, Rust against Go: each tests/parity/*.case.json
//! is a command and the fake gh answers it gets; *.want.json is what Go's
//! rush said (gofixtures/gen.sh writes them). The Rust commands must say
//! the same, byte for byte, end with the same code, and ask gh the same
//! things in the same order.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rush_gh::ghpr::RunError;
use rush_watch::gh::{GhOutput, set_thread_gh};
use serde::Deserialize;

#[derive(Deserialize, Clone)]
struct Answer {
    #[serde(rename = "match")]
    matches: Vec<String>,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    exit: i32,
}

#[derive(Deserialize)]
struct Case {
    cmd: String,
    args: Vec<String>,
    session: String,
    on: Vec<String>,
    gh: Vec<Answer>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Want {
    stdout: String,
    stderr: String,
    code: i32,
    asked: Vec<Vec<String>>,
}

fn run(case: &Case) -> Want {
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    std::fs::create_dir_all(home.path().join("plugins")).unwrap();
    std::fs::write(
        home.path().join("plugins").join("bundled-on.json"),
        serde_json::to_vec(&case.on).unwrap(),
    )
    .unwrap();
    let asked = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let (kept, answers) = (asked.clone(), case.gh.clone());
    set_thread_gh(Some(Arc::new(move |_: Option<&Path>, args: &[String]| {
        kept.lock().unwrap().push(args.to_vec());
        let line = args.join(" ");
        let a = answers
            .iter()
            .find(|a| a.matches.iter().all(|m| line.contains(m.as_str())))
            .cloned()
            .unwrap_or(Answer {
                matches: vec![],
                stdout: String::new(),
                stderr: format!("no fake answer for: {line}"),
                exit: 1,
            });
        GhOutput {
            stdout: a.stdout.into_bytes(),
            stderr: a.stderr.clone().into_bytes(),
            err: (a.exit != 0).then(|| RunError::Exit {
                status: format!("exit status {}", a.exit),
                stderr: a.stderr,
            }),
        }
    })));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = match case.cmd.as_str() {
        "pr" => {
            let env = rush_prdrive::Env {
                dir: home.path().to_path_buf(),
                session: case.session.clone(),
                host: Box::new(rush_prdrive::FilesHost),
                start: Some(Box::new(|_| Err("no followers in tests".into()))),
            };
            rush_prdrive::pr_cmd_in(&env, &case.args, &mut stdout, &mut stderr)
        }
        _ => {
            let env = rush_watch::Env {
                dir: home.path().to_path_buf(),
                session: case.session.clone(),
                task: String::new(),
                options: Some(Box::new(|o| {
                    o.log = Some(Box::new(|_, _| {}));
                })),
            };
            rush_watch::watch_cmd_in(&env, &case.args, &mut stdout, &mut stderr)
        }
    };
    set_thread_gh(None);
    rush_state::set_thread_dir(None);
    let asked = asked.lock().unwrap().clone();
    Want {
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
        code,
        asked,
    }
}

#[test]
fn says_what_go_says() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("parity");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".case.json"))
        .collect();
    cases.sort();
    assert!(cases.len() > 40, "only {} cases", cases.len());
    let mut wrong = Vec::new();
    for f in &cases {
        let name = f.file_name().unwrap().to_string_lossy().replace(".case.json", "");
        let case: Case = serde_json::from_slice(&std::fs::read(f).unwrap()).unwrap();
        let want: Want =
            serde_json::from_slice(&std::fs::read(dir.join(format!("{name}.want.json"))).unwrap())
                .unwrap();
        let got = run(&case);
        if got != want {
            let mut why = format!("== {name}\n");
            if got.code != want.code {
                why += &format!("code {} want {}\n", got.code, want.code);
            }
            if got.stdout != want.stdout {
                why += &format!("stdout:\n{}\nwant:\n{}\n", got.stdout, want.stdout);
            }
            if got.stderr != want.stderr {
                why += &format!("stderr:\n{}\nwant:\n{}\n", got.stderr, want.stderr);
            }
            if got.asked != want.asked {
                why += &format!("asked:\n{:#?}\nwant:\n{:#?}\n", got.asked, want.asked);
            }
            wrong.push(why);
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ from Go:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
