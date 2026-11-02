//! A plugin connection's round trip and notification throughput, read by
//! the process's one reader (`Conn::new`) and by a thread of its own
//! (`Conn::framed` on the same socket, as every connection was read
//! before). `cargo bench -p rush-plugin --bench rpc`.

use std::hint::black_box;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use criterion::{Criterion, criterion_group, criterion_main};
use rush_plugin::Ctx;
use rush_plugin::rpc::{Conn, Handler};

/// How notifications are sent in one go for the throughput bench.
const BURST: usize = 1000;

fn conn(s: UnixStream, shared: bool, h: Option<Handler>) -> Conn {
    if shared {
        return Conn::new(s, h).unwrap();
    }
    let r = s.try_clone().unwrap();
    let c = s.try_clone().unwrap();
    Conn::framed(
        r,
        s,
        move || {
            let _ = c.shutdown(std::net::Shutdown::Both);
        },
        h,
    )
}

fn pair(shared: bool, a: Option<Handler>, b: Option<Handler>) -> (Conn, Conn) {
    let (x, y) = UnixStream::pair().unwrap();
    (conn(x, shared, a), conn(y, shared, b))
}

fn bench(c: &mut Criterion) {
    let echo: Handler = Arc::new(|_, _, _, p| Ok(p.map(|p| p.to_owned())));
    for (name, shared) in [("thread each", false), ("shared reader", true)] {
        let (a, b) = pair(shared, None, Some(echo.clone()));
        let ctx = Ctx::background();
        c.bench_function(&format!("call round trip ({name})"), |bn| {
            bn.iter(|| {
                let got: u64 = a.call(&ctx, "echo", &black_box(7u64)).unwrap();
                got
            })
        });
        a.close();
        b.wait();

        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let count: Handler = Arc::new(move |_, _, _, _| {
            let _ = tx.lock().unwrap().send(());
            Ok(None)
        });
        let (a, b) = pair(shared, None, Some(count));
        c.bench_function(&format!("{BURST} notifications ({name})"), |bn| {
            bn.iter(|| {
                for i in 0..BURST {
                    a.notify("ev", &i).unwrap();
                }
                for _ in 0..BURST {
                    rx.recv().unwrap();
                }
            })
        });
        a.close();
        b.wait();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
