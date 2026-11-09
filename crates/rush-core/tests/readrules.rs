//! readrules: which commands a session's own Bash rules allow once its
//! well-known variables and escaped spaces are taken as safe.

use rush_core::readrules::reads_only;

/// The cleanup agent's reads (rush-ui-projects-machine's CARE_READS) and
/// its scratch writes.
fn care() -> Vec<String> {
    [
        "Bash(du:*)",
        "Bash(df:*)",
        "Bash(ls:*)",
        "Bash(stat:*)",
        "Bash(git status:*)",
        "Bash(git worktree list:*)",
        "Bash(git rev-list:*)",
        "Bash(git branch)",
        "Bash(git branch -vv)",
        "Bash(xcrun simctl list:*)",
        "Write(/$TMPDIR/**)",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn reads_with_well_known_variables_pass() {
    let rules = care();
    for (cmd, want) in [
        // What the cleanup agent was asked about.
        ("echo $TMPDIR; getconf DARWIN_USER_TEMP_DIR", true),
        (
            r"du -xhd1 /Users/me/Library/Developer/Xcode /Users/me/Library/Developer/Xcode/iOS\ DeviceSupport",
            true,
        ),
        (
            r#"du -xhd1 "/Users/me/Library/Developer/Xcode/iOS DeviceSupport""#,
            true,
        ),
        // Each well-known variable, bare, braced and quoted.
        ("du -sh $TMPDIR", true),
        ("du -sh ${HOME}/Library/Caches", true),
        (r#"ls -la "$HOME/.cache""#, true),
        ("echo $RUSH_SESSION $PWD $USER", true),
        ("ls ~/Library/Caches", true),
        ("df -h && du -sh ~/Library", true),
        ("df -h;", true),
        ("git status --short", true),
        ("git worktree list --porcelain", true),
        ("git branch", true),
        ("git branch -vv", true),
        ("xcrun simctl list devices", true),
        ("pwd", true),
        ("printf '%s\\n' $HOME", true),
        // Any other variable, or one that isn't plain.
        ("echo $SECRET", false),
        ("du -sh $X", false),
        ("du -sh ${HOME:-/}", false),
        ("echo $1", false),
        ("echo $?", false),
        ("echo $", false),
        (r#"echo "$OTHER""#, false),
        // Not a read the rules allow, with or without a variable.
        ("rm -rf $TMPDIR/x", false),
        ("rm -rf ~/Library/Caches", false),
        ("du -sh $TMPDIR && rm -rf $TMPDIR/x", false),
        ("ls; rm -rf y", false),
        ("git branch -D rush/a", false),
        ("git -C /r status", false),
        ("find $HOME -delete", false),
        ("bash $TMPDIR/cleanup.sh", false),
        ("dutree x", false),
        // Something that runs or writes besides.
        ("echo $(rm -rf ~)", false),
        ("echo `rm -rf ~`", false),
        (r#"echo "$(id)""#, false),
        ("du -sh x > $TMPDIR/out", false),
        ("du -sh x | xargs rm", false),
        ("du -sh x & rm y", false),
        ("du -sh x || rm y", false),
        ("(du -sh x)", false),
        ("{ du -sh x; }", false),
        ("HOME=/ du -sh ~", false),
        ("$HOME/bin/tool", false),
        ("du -sh x\nrm y", false),
        ("du 'unclosed", false),
        ("du \"unclosed", false),
        ("du -sh x &&", false),
        ("", false),
    ] {
        assert_eq!(reads_only(cmd, &rules), want, "{cmd}");
    }
}

#[test]
fn without_bash_rules_only_prints_pass() {
    let none: Vec<String> = Vec::new();
    for (cmd, want) in [
        ("echo $TMPDIR", true),
        ("getconf DARWIN_USER_TEMP_DIR", true),
        ("du -sh $TMPDIR", false),
        ("ls", false),
    ] {
        assert_eq!(reads_only(cmd, &none), want, "{cmd}");
    }
}
