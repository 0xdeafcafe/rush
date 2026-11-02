//! The tests of rush-ui-projects-machine's projectmem.rs that need the whole UI.

use rush_ui_projects_machine::data::*;
use rush_ui_projects_machine::projectmem::*;
use rush_ui_projects_machine::rows::*;

/// Projects shows a project's memory by kind and how many to review,
/// on a row picked to review it (TestProjectMemoryRow).
#[test]
fn project_memory_row() {
    let mut m = crate::new_model("t");
    let key = "/src/app";
    let now = m.now;
    m.kept::<Data>().mem_counts.insert(
        key.into(),
        MemSummary {
            root: key.into(),
            notes: 3,
            kinds: [("feedback".into(), 2), ("project".into(), 1)].into(),
            review: 2,
            at: now,
            ..MemSummary::default()
        },
    );
    let p = Project {
        key: key.into(),
        title: "app".into(),
        agents: Vec::new(),
        open: false,
        repo: true,
    };
    let mut rows = Vec::new();
    project_memory(&m, &p, 100, &mut rows);
    let text: Vec<String> = rows.iter().map(|r| r.line.plain()).collect();
    let text = text.join("\n");
    for want in ["MEMORY  3 notes", "2 feedback · 1 project", "2 to review"] {
        assert!(text.contains(want), "rows lack {want:?}:\n{text}");
    }
    let last = rows.last().unwrap();
    assert!(last.pickable() && matches!(last.what, What::Mem(_)));
}
