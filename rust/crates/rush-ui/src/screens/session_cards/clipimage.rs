//! An image on the clipboard into the message box, as ctrl+v does in
//! Claude Code: a terminal pastes only text, so a screenshot copied to the
//! clipboard never reaches rush as a paste. After Go's
//! internal/ui/clipimage.go (setClipboard :18, pasteClipImage :42,
//! clipImage :65, clipJXA :140).

use std::path::PathBuf;
use std::process::Command;

use super::{Act, act};
use crate::frame::Out;
use crate::frame::editor::is_image_file;
use crate::model::Model;

/// Whether a read of the clipboard is under way (pastingImg).
#[derive(Default)]
pub struct ClipState {
    pub pasting: bool,
}

/// Reads an image off the clipboard off the UI thread, and attaches it to
/// agent key's box once it's back (pasteClipImage, clipImageMsg).
pub fn paste_clip_image(m: &mut Model, key: &str) -> Out {
    if m.kept::<ClipState>().pasting {
        return Out::Idle; // one read at a time
    }
    m.kept::<ClipState>().pasting = true;
    m.flash("reading the clipboard…");
    let key = key.to_string();
    m.later(clip_image, move |m, r| {
        m.kept::<ClipState>().pasting = false;
        match r {
            Err(e) => {
                m.flash_err(&format!("couldn't read the clipboard: {e}"));
                Out::Draw
            }
            Ok(None) => {
                m.flash("no image on the clipboard");
                Out::Draw
            }
            Ok(Some(p)) => act(m, &key, Act::AttachImages(vec![p])),
        }
    });
    Out::Draw
}

/// The image on the clipboard as a file: an image file copied in the
/// Finder as itself, image data saved as a PNG under the temp dir. None
/// when the clipboard holds no image.
pub fn clip_image() -> Result<Option<String>, String> {
    let dir = std::env::temp_dir().join("rush-images");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!(
        "clipboard-{}.png",
        super::clock::stamp(rush_core::time::now())
    ));
    let outs = out.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") {
        // Asked of NSPasteboard directly: AppleScript's «the clipboard»
        // converts every format it holds first, seconds for a screenshot.
        let o = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", CLIP_JXA, &outs])
            .output()
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
        }
        let res = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if let Some(p) = res.strip_prefix("file:") {
            // A copied file: attach the file itself when it's an image.
            let ok = std::fs::metadata(p).is_ok_and(|m| !m.is_dir()) && is_image_file(p);
            return Ok(ok.then(|| p.to_string()));
        }
        if res.is_empty() {
            return Ok(None);
        }
    } else {
        let o = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Command::new("wl-paste")
                .args(["--type", "image/png"])
                .output()
        } else {
            Command::new("xclip")
                .args(["-selection", "clipboard", "-t", "image/png", "-o"])
                .output()
        };
        let Ok(o) = o else { return Ok(None) };
        if !o.status.success() || o.stdout.is_empty() {
            return Ok(None);
        }
        std::fs::write(&out, &o.stdout).map_err(|e| e.to_string())?;
    }
    if std::fs::metadata(&out).map_or(true, |m| m.len() == 0) {
        let _ = std::fs::remove_file(&out);
        return Ok(None);
    }
    Ok(Some(outs))
}

/// Puts text on the clipboard through pbcopy as well as the terminal, on a
/// Mac rush runs on: Terminal.app ignores OSC 52 (setClipboard). Runs off
/// the UI thread; the terminal's copy is the frame's.
pub fn pbcopy(m: &mut Model, text: String) {
    if !cfg!(target_os = "macos")
        || std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
    {
        return;
    }
    m.later(
        move || {
            use std::io::Write;
            if let Ok(mut c) = Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut i) = c.stdin.take() {
                    let _ = i.write_all(text.as_bytes());
                }
                let _ = c.wait();
            }
        },
        |_, ()| Out::Idle,
    );
}

/// The folder clipboard images are saved in.
pub fn images_dir() -> PathBuf {
    std::env::temp_dir().join("rush-images")
}

/// Writes the clipboard's image to argv[0] as a PNG and prints "png",
/// prints "file:" and the path of a copied file, or prints nothing.
pub const CLIP_JXA: &str = r#"ObjC.import('AppKit');
function run(argv) {
  const pb = $.NSPasteboard.generalPasteboard;
  const url = $.NSURL.URLFromPasteboard(pb);
  if (!url.isNil() && url.isFileURL) return 'file:' + url.path.js;
  let d = pb.dataForType('public.png');
  if (d.isNil()) {
    const tiff = pb.dataForType('public.tiff');
    if (tiff.isNil()) return '';
    d = $.NSBitmapImageRep.imageRepWithData(tiff).representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $());
  }
  return d.writeToFileAtomically(argv[0], true) ? 'png' : '';
}"#;
