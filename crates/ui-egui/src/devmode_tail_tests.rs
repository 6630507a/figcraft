//! Dev Mode in the UI, the tail: the compare view compares with a saved version picked at its
//! bottom, and falls back when that version goes away.

use figcraft_engine::Session;
use serde_json::json;

use crate::dev_compare::CompareView;
use crate::present_tests::{Ui, id};

#[test]
fn compare_view_compares_with_a_saved_version() {
    let mut s = Session::new();
    let f = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 300, "height": 400, "name": "Login"})).unwrap());
    let r = id(&s.execute("shape.rectangle", &json!({"x": 20, "y": 300, "width": 120, "height": 40, "parent": f})).unwrap());
    let mut u = Ui::new(s);
    u.app.ui.right_tab = crate::RightTab::Inspect;
    u.frame(vec![]);
    // Neither ready for dev nor any version: nothing to compare with, the view doesn't stay.
    u.app.ui.dev.compare = Some(CompareView::new(f, None));
    u.frames(2, Vec::new);
    assert!(u.app.ui.dev.compare.is_none());
    let v = u.run("file.saveVersion", json!({"name": "Kickoff"}));
    let v = v["id"].as_str().unwrap().to_string();
    u.run("node.set", json!({"id": r, "props": {"width": 160}}));
    u.run("dev.compare", json!({"id": f, "version": v, "show": true}));
    u.frames(2, Vec::new);
    assert_eq!(u.app.inspect()["devCompare"]["version"], json!(v));
    // Opened without one, a layer that isn't ready compares with the newest version.
    u.app.ui.dev.compare = Some(CompareView::new(f, None));
    u.frames(2, Vec::new);
    assert_eq!(u.app.inspect()["devCompare"]["version"], json!(v));
    // Ready for dev: the snapshot from when it was marked is there too, and a deleted version
    // falls back to it.
    u.run("node.set", json!({"id": f, "props": {"devStatus": "readyForDev"}}));
    u.run("node.set", json!({"id": r, "props": {"height": 48}}));
    u.run("version.delete", json!({"id": v}));
    u.frames(2, Vec::new);
    assert_eq!(u.app.inspect()["devCompare"]["version"], json!(null));
    assert!(u.app.ui.dev.compare.is_some());
}
