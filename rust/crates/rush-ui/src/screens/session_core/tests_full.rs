//! The Changes view's file in full (alt+v): git's diff of the picked file
//! laid over the whole of it, the arrows scrolling it, esc going back to
//! the file in the list; and its hunks coloured as code. After Go's
//! rushmode.go:4386 (alt+v), :4319 (↑↓), :4158 (esc), :3835-3838 (the
//! hints) and convo/changes.go:698 FileView.

use super::*;

/// The row of the screen whose text has s.
fn row_with(m: &mut Model, s: &str) -> Option<usize> {
    screen(m).iter().position(|l| l.contains(s))
}

/// A git checkout with a.rs committed and then changed, open in a
/// session showing Changes; the folder, to remove.
fn checkout() -> (Model, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "rush-full-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    let before: String = (1..=30).map(|i| format!("let v{i} = {i};\n")).collect();
    std::fs::write(dir.join("a.rs"), &before).unwrap();
    git(&["add", "a.rs"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "one",
    ]);
    std::fs::write(
        dir.join("a.rs"),
        before.replace("let v2 = 2;", "let v2 = \"two\";"),
    )
    .unwrap();
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 30);
    sess(&mut m).info.cwd = dir.to_string_lossy().into_owned();
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "changes"
    ));
    screen(&mut m); // with no loop, git is read at once
    (m, dir)
}

#[test]
fn alt_v_shows_the_picked_file_in_full() {
    let (mut m, dir) = checkout();
    press(&mut m, "up");
    let sel = m.session.as_ref().unwrap().sel.clone();
    assert!(sel.starts_with("tree:"), "{sel}");
    let s = screen(&mut m);
    assert!(
        s[s.len() - 1].contains("the file in full"),
        "{:?}",
        s[s.len() - 1]
    );
    press(&mut m, "alt+v");
    screen(&mut m); // the full diff, read
    let s = screen(&mut m);
    let y = row_with(&mut m, "a.rs  +1 −1").expect("its head");
    assert!(
        s[y + 2].trim_start().starts_with("1   let v1 = 1;"),
        "{:?}",
        s[y + 2]
    );
    assert!(s[y + 3].contains("− let v2 = 2;"), "{:?}", s[y + 3]);
    assert!(s[y + 4].contains("2 + let v2 = \"two\";"), "{:?}", s[y + 4]);
    assert_eq!(
        s[s.len() - 1].trim(),
        "↑↓ · pgup pgdn scroll  ·  esc back to the changes"
    );
    // The arrows scroll it; nothing in it is picked.
    press(&mut m, "down");
    let t = screen(&mut m);
    assert_ne!(t, s, "↓ scrolled");
    assert_eq!(m.session.as_ref().unwrap().sel, sel);
    // esc: back to the file, picked, in the list.
    press(&mut m, "esc");
    screen(&mut m);
    assert!(row_with(&mut m, "▾ Working tree").is_some());
    assert_eq!(m.session.as_ref().unwrap().sel, sel);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hunks_are_coloured_as_code() {
    let (mut m, dir) = checkout();
    press(&mut m, "up");
    press(&mut m, "enter");
    screen(&mut m);
    let lines = crate::screens::session_core::views::body(&mut m, 116, 26).unwrap_or_default();
    let added = lines
        .iter()
        .find(|l| l.plain().contains("+ let v2"))
        .expect("the added line");
    let colours: std::collections::HashSet<_> = added
        .spans
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| s.style.fg)
        .collect();
    assert!(colours.len() > 2, "keyword, string and text: {colours:?}");
    std::fs::remove_dir_all(&dir).ok();
}
