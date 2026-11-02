//! Prints each line's runs, for diffing against photon/hl's: `hldump
//! go,md,sh file…` writes "i j C;" per run, a line per source line.

use std::io::Write;

use rush_hl::{Class, Lang, State};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((langs, files)) = args.split_first() else {
        eprintln!("usage: hldump lang,lang file…");
        std::process::exit(2);
    };
    let mut w = std::io::BufWriter::new(std::io::stdout().lock());
    for f in files {
        let src = std::fs::read(f).unwrap_or_default();
        let src = String::from_utf8_lossy(&src);
        for name in langs.split(',') {
            let l = Lang::for_name(name);
            let mut st = State::default();
            for s in src.split('\n') {
                rush_hl::line(l, &mut st, s, |i, j, c| {
                    let c = match c {
                        Class::Plain => 'P',
                        Class::Keyword => 'K',
                        Class::String => 'S',
                        Class::Number => 'N',
                        Class::Function => 'F',
                        Class::Type => 'T',
                        Class::Comment => 'C',
                    };
                    let _ = write!(w, "{i} {j} {c};");
                });
                let _ = writeln!(w);
            }
        }
    }
}
