//! Write-behind is the whole process's, so its tests run in a binary of
//! their own, one after the other.

use rush_state::store::{Overlay, Store};
use std::collections::BTreeMap;
use std::fs;

// With write-behind on, a save returns at once and the writer puts the
// newest of each file on disk; a copy kept before is made before the save
// that follows it. The writer encodes state.json from the overlay as it
// was saved: changing it meanwhile doesn't show in that save.
#[test]
fn write_behind() {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    rush_state::write_behind();

    let mut s = Store::default();
    s.config.group_by = "old".into();
    s.save_config().unwrap();
    rush_state::flush().unwrap();
    rush_state::keep_before("test");
    for g in ["a", "b", "new"] {
        s.config.group_by = g.into();
        s.save_config().unwrap();
    }
    rush_state::flush().unwrap();
    let read = |name: &str| fs::read_to_string(d.path().join(name)).unwrap_or_default();
    assert!(
        read("config.json").contains(r#""new""#),
        "config.json is {}",
        read("config.json")
    );
    assert!(
        read("config.json.test").contains(r#""old""#),
        "kept before: {}",
        read("config.json.test")
    );

    let mut s = Store::default();
    s.overlay = Overlay {
        names: BTreeMap::from([("a".into(), "saved".into())]),
        ..Overlay::default()
    };
    s.save_overlay().unwrap();
    s.overlay.names.insert("a".into(), "after".into());
    rush_state::flush().unwrap();
    assert!(
        read("state.json").contains(r#""saved""#),
        "state.json is {}",
        read("state.json")
    );
}
