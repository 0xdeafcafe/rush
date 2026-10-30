//! Any setting rush keeps in config.json can be set for one run from the
//! environment: RUSH_ and its JSON key in upper snake case, a nested one
//! after its parent's, so copyOnSelect is RUSH_COPY_ON_SELECT and
//! hibernate.afterMinutes RUSH_HIBERNATE_AFTER_MINUTES. Settings that are
//! lists or maps can't be. A value from the environment isn't saved: what
//! config.json held is written back in its place, unless you change the
//! setting in rush meanwhile.
//!
//! Ported from internal/state/envconfig.go. Go walks Config by reflection;
//! here the settings are listed (FIELDS), in Config's order.

use crate::config::Config;

/// One setting's value.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    OptBool(Option<bool>),
}

/// A setting the environment can set: its JSON path, and how to read and
/// change it on a Config.
struct Field {
    key: &'static str,
    get: fn(&Config) -> Val,
    set: fn(&mut Config, Val),
}

macro_rules! fields {
    ($( $key:literal => $kind:ident : $($f:ident).+ ),* $(,)?) => {
        &[ $( Field {
            key: $key,
            get: |c| Val::$kind(c.$($f).+.clone()),
            set: |c, v| if let Val::$kind(x) = v { c.$($f).+ = x },
        } ),* ]
    };
}

/// Every setting that isn't a list or a map, as Go's walk finds them.
static FIELDS: &[Field] = fields![
    "twatter" => Bool: feed,
    "cleanupDiskGB" => Int: cleanup_disk_gb,
    "cleanupRAMMinutes" => Int: cleanup_ram_minutes,
    "hideMinimap" => Bool: hide_minimap,
    "gridBelow" => Bool: grid_below,
    "gridShare" => Float: grid_share,
    "pixelPictures" => Bool: pixel_pictures,
    "compactKind" => Str: compact_kind,
    "compactModel" => Str: compact_model,
    "pruneModel" => Str: prune_model,
    "foldersImported" => Bool: folders_imported,
    "active" => Str: active,
    "defaultProfile" => Str: default_profile,
    "profilesMovedIn" => Bool: profiles_moved_in,
    "atLimit" => Str: at_limit,
    "builtinProfiles" => Bool: builtin_profiles,
    "switchOnLimit" => Str: switch_on_limit,
    "stayOnAccount" => Bool: stay_on_account,
    "groupBy" => Str: group_by,
    "splitBy" => Str: split_by,
    "listLook" => Str: list_look,
    "dispatch.kind" => Str: dispatch.kind,
    "dispatch.agent" => Str: dispatch.agent,
    "dispatch.model" => Str: dispatch.model,
    "dispatch.effort" => Str: dispatch.effort,
    "dispatch.permission" => Str: dispatch.permission,
    "dispatch.runIn" => Str: dispatch.run_in,
    "dispatch.onLimit" => Str: dispatch.on_limit,
    "dispatch.lean" => OptBool: dispatch.lean,
    "dispatch.every.model" => Str: dispatch.every.model,
    "dispatch.every.effort" => Str: dispatch.every.effort,
    "dispatch.every.mode" => Str: dispatch.every.mode,
    "dispatch.harnessStarts" => Bool: dispatch.harness_starts,
    "quiet" => Bool: quiet,
    "dockLines" => Int: dock_lines,
    "sideWidth" => Float: side_width,
    "view" => Str: view,
    "listOnly" => Bool: list_only,
    "chatFull" => Bool: chat_full,
    "enterOn" => Str: enter_on,
    "sortBy" => Str: sort_by,
    "stackAt" => Int: stack_at,
    "hibernate.afterMinutes" => Int: hibernate.after_minutes,
    "everywhere.attribution" => Str: everywhere.attribution,
    "activeMinutes" => Int: active_minutes,
    "justLeftMinutes" => Int: just_left_minutes,
    "justLeftCount" => Int: just_left_count,
    "cleanupHours" => Int: cleanup_hours,
    "keepOrphans" => Bool: keep_orphans,
    "restartOnInstall" => Bool: restart_on_install,
    "trimRunningTmp" => Bool: trim_running_tmp,
    "keepTranscriptsPlain" => Bool: keep_transcripts_plain,
    "subLine" => Str: sub_line,
    "hideLogo" => Bool: hide_logo,
    "colorBlind" => Bool: color_blind,
    "theme" => Str: theme,
    "showWhitespace" => Bool: show_whitespace,
    "copyOnSelect" => OptBool: copy_on_select,
    "searchTranscriptsOnKey" => Bool: search_transcripts_on_key,
    "enterSendsCommand" => Bool: enter_sends_command,
    "rowKey" => Str: row_key,
    "menuBar" => Bool: menu_bar,
    "menuBarAsked" => Bool: menu_bar_asked,
    "onboarding.hidden" => Bool: onboarding.hidden,
    "advisor" => Bool: advisor,
];

/// One setting the environment set.
#[derive(Clone, Debug)]
pub(crate) struct EnvSet {
    field: usize,
    /// The variable.
    pub(crate) name: String,
    /// What config.json held.
    file: Val,
    /// What the variable set.
    env: Val,
}

/// The variable that sets the setting at key, a JSON path ("copyOnSelect",
/// "hibernate.afterMinutes").
pub fn env_name(key: &str) -> String {
    let mut b = String::from("RUSH_");
    let bytes = key.as_bytes();
    for (i, r) in key.char_indices() {
        if r == '.' {
            b.push('_');
        } else if r.is_uppercase() && i > 0 && bytes[i - 1] != b'.' {
            b.push('_');
            b.push(r);
        } else {
            b.extend(r.to_uppercase());
        }
    }
    b
}

/// Sets what the environment names on c, and returns what it set.
pub(crate) fn apply_env(c: &mut Config, getenv: &dyn Fn(&str) -> Option<String>) -> Vec<EnvSet> {
    let mut out = Vec::new();
    for (i, f) in FIELDS.iter().enumerate() {
        let name = env_name(f.key);
        let Some(raw) = getenv(&name) else { continue };
        let was = (f.get)(c);
        let Some(set) = parse_env(&was, raw.trim()) else {
            continue;
        };
        (f.set)(c, set.clone());
        out.push(EnvSet {
            field: i,
            name,
            file: was,
            env: set,
        });
    }
    out
}

/// Reads a variable's value as a setting of like's type.
fn parse_env(like: &Val, s: &str) -> Option<Val> {
    Some(match like {
        Val::Str(_) => Val::Str(s.into()),
        Val::Bool(_) => Val::Bool(parse_bool(s)?),
        Val::Int(_) => Val::Int(s.parse().ok()?),
        Val::Float(_) => Val::Float(s.parse().ok()?),
        Val::OptBool(_) => Val::OptBool(Some(parse_bool(s)?)),
    })
}

pub(crate) fn parse_bool(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// c as config.json should keep it: each setting the environment set and
/// you haven't changed since as the file had it.
pub(crate) fn for_saving(c: &Config, env: &[EnvSet]) -> Config {
    let mut c = c.clone();
    for e in env {
        let f = &FIELDS[e.field];
        if (f.get)(&c) == e.env {
            (f.set)(&mut c, e.file.clone());
        }
    }
    c
}

/// The bool setting at key as the environment sets it, if it does: for
/// code that reads config.json itself.
pub fn env_bool(key: &str) -> Option<bool> {
    let s = std::env::var(env_name(key)).ok()?;
    parse_bool(s.trim())
}

pub(crate) fn lookup_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names() {
        for (key, want) in [
            ("copyOnSelect", "RUSH_COPY_ON_SELECT"),
            ("theme", "RUSH_THEME"),
            ("hibernate.afterMinutes", "RUSH_HIBERNATE_AFTER_MINUTES"),
            ("searchTranscriptsOnKey", "RUSH_SEARCH_TRANSCRIPTS_ON_KEY"),
            ("cleanupDiskGB", "RUSH_CLEANUP_DISK_G_B"),
            ("dispatch.every.mode", "RUSH_DISPATCH_EVERY_MODE"),
        ] {
            assert_eq!(env_name(key), want, "{key}");
        }
    }
}
