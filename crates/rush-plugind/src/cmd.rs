//! `rush plugind` and `rush plugin …` as the binary calls them: with this
//! process's terminal, printing an error as Go's exitIf does ("rush: …")
//! and giving the exit code.
//! Ported from cmd/rush/main.go:178-191 and :373-378.

use std::io::{BufRead, IsTerminal, Write};

/// `rush plugind`: the plugin broker. rush starts it when a plugin is
/// approved; it is not meant to be run by hand.
pub fn plugind_main() -> i32 {
    rush_events::set_process("plugind");
    exit_if(crate::run())
}

/// `rush plugin <args>` (and `rush plugins`), args after the command.
pub fn plugin_main(args: &[String]) -> i32 {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    let mut read_line = move || {
        let mut s = String::new();
        let _ = stdin.lock().read_line(&mut s);
        s
    };
    let res = rush_plugin::cmd::plugin_cmd(
        args,
        &mut rush_plugin::cmd::Io {
            out: &mut out,
            stdin_terminal: terminal,
            read_line: &mut read_line,
        },
    );
    let _ = out.flush();
    drop(out);
    exit_if(res)
}

fn exit_if(r: Result<(), String>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "rush: {e}");
            rush_events::flush();
            1
        }
    }
}
