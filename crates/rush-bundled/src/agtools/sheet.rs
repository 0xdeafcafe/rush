//! The contact_sheet tool: many images on one, each captioned with its
//! number and file name, sized to cost about what one screenshot does
//! (contextguard::images). The context guard points agents here once
//! they've looked at many images. Rust-only.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::{Tool, result};
use crate::contextguard::images;

/// The tool's name, as rush serves it.
pub const CONTACT_SHEET: &str = "contact_sheet";

/// What the agent sends.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SheetInput {
    pub images: Vec<String>,
    pub out: String,
}

pub(crate) fn sheet_tool() -> Tool {
    Tool {
        name: CONTACT_SHEET.into(),
        description: "Put several images on one contact sheet and see them all at once: one WebP image with each \
captioned by its number and file name, sized to cost about what one screenshot does (1,568 tokens unless set \
otherwise), where reading each costs that apiece and keeps every one in your context. Use it to look over \
screenshots, renders or frames; read one alone when you need its detail. PNG, JPEG, GIF and WebP, up to 48 a sheet."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "images": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "The images' absolute paths, in the order to show them.",
                },
                "out": { "type": "string", "description": "Where to keep the sheet (.webp); a file in your scratch folder unless given." },
            },
            "required": ["images"],
            "additionalProperties": false,
        }),
        meta: None,
    }
}

/// Makes the sheet: its words and the image itself, for the agent to see
/// at once.
pub(crate) fn call_sheet(args: Option<&RawValue>) -> Value {
    let input: SheetInput =
        match args.map(|a| rush_state::json::from_slice::<SheetInput>(a.get().as_bytes())) {
            Some(Ok(i)) => i,
            Some(Err(e)) => return result(&format!("Its input: {e}"), true),
            None => SheetInput::default(),
        };
    if let Some(p) = input
        .images
        .iter()
        .find(|p| !std::path::Path::new(p).is_absolute())
    {
        return result(
            &format!("Give each image's absolute path: {p} isn't one."),
            true,
        );
    }
    let paths: Vec<PathBuf> = input.images.iter().map(PathBuf::from).collect();
    let s = match images::sheet(&paths, images::budget()) {
        Ok(s) => s,
        Err(e) => return result(&e, true),
    };
    let at = if input.out.is_empty() {
        images::sheet_path(&std::env::temp_dir(), &paths)
    } else {
        PathBuf::from(&input.out)
    };
    let mut said = images::sheet_said(&s, &at);
    if !images::keep(&at, &s.webp) {
        said = format!(
            "{} (It couldn't be kept at {}: it's only here.)",
            said.rsplit_once(" Kept at ")
                .map_or(said.as_str(), |(a, _)| a),
            at.display()
        );
    }
    use base64::Engine as _;
    serde_json::json!({
        "content": [
            { "type": "text", "text": said },
            {
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(&s.webp),
                "mimeType": "image/webp",
            },
        ],
        "isError": false,
    })
}
