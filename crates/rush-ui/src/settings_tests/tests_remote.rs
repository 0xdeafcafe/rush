//! The Remote page's pairing sheet and device rows, drawn in a whole UI.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rush_ui_settings::remote::{Device, PairSheet, Pairing, qr_lines, sections};

use super::*;
use crate::frame::sheet::Sheet;

fn sheet(url: &str) -> PairSheet {
    PairSheet {
        p: Pairing {
            code: "ABCD2345".into(),
            url: url.into(),
            expires: String::new(),
            ssh: "ssh -p 2222 rush@mac".into(),
            host_key: "SHA256:xyz".into(),
        },
        err: None,
        opened: 0,
        paired: None,
        gone: Arc::new(AtomicBool::new(false)),
    }
}

/// The link as a QR code where there's room, the link itself where
/// there isn't, and the code under either (pairSheet.body).
#[test]
fn pair_sheet_draws_the_qr_code() {
    let m = crate::new_model("0.1.0");
    let url = "https://rush.example.ts.net:8443/pair#ABCD2345";
    let qr = qr_lines(url);
    assert_eq!(
        qr.len(),
        23,
        "a 41-module code and its quiet zone, two to a row"
    );
    assert!(qr.iter().all(|l| l.width() == 45));
    let tall = sheet(url).body(&m, 66, 60);
    let text: Vec<String> = tall.iter().map(Line::plain).collect();
    assert_eq!(text.iter().filter(|l| l.contains('▀')).count(), 23);
    assert!(
        text.iter()
            .any(|l| l.contains("or from an SSH app:  ssh -p 2222 rush@mac"))
    );
    assert!(
        text.iter()
            .any(|l| l.contains("Code ABCD-2345 · works once"))
    );
    assert!(tall.iter().all(|l| l.width() <= 66));
    let short = sheet(url).body(&m, 66, 20);
    let text: Vec<String> = short.iter().map(Line::plain).collect();
    assert!(!text.iter().any(|l| l.contains('▀')));
    assert!(
        text.iter()
            .any(|l| l.contains("Make the window taller for the QR code"))
    );
    assert!(text.iter().any(|l| l == url));
    let none = sheet("").body(&m, 66, 60);
    let text: Vec<String> = none.iter().map(Line::plain).collect();
    assert!(
        text.iter()
            .any(|l| l.contains("from an SSH app:") && !l.contains("or from"))
    );
}

/// Serve's state and each device's last sighting in Go's colours:
/// running green and the counts dim, a sighting dim (remoteSections,
/// deviceRow).
#[test]
fn serve_and_devices_in_colour() {
    let mut m = crate::new_model("0.1.0");
    {
        let r = &mut m.kept::<State>().remote;
        r.devs = vec![Device {
            id: "d1".into(),
            kind: "web".into(),
            name: "iPhone".into(),
            ..Device::default()
        }];
    }
    let secs = sections(&m);
    let serve = &secs[0].rows[1];
    assert_eq!(serve.value, "running · 1 paired, 0 connected now");
    assert_eq!(serve.styled.spans[0].style.fg, GREEN);
    assert_eq!(serve.styled.spans[1].style.fg, DIM);
    let dev = &secs[1].rows[0];
    assert_eq!(dev.value, "seen never");
    assert_eq!(dev.styled.spans[0].style.fg, DIM);
    let row = form::setting_row(false, dev, 20, 12, 80);
    assert!(
        row.spans
            .iter()
            .any(|s| s.text == "seen never" && s.style.fg == DIM)
    );
}
