//! What falls back to the CPU and what an edit rebuilds (no GPU needed; the pixel comparison
//! against the CPU renderer is `tests/parity.rs`).

use figcraft_color::{Color, Paint};
use figcraft_doc::{Document, Effect, NodeId, NodeKind};
use vello::Scene;

use super::{Builder, Images, dirty_tops};

fn doc() -> (Document, NodeId, NodeId, NodeId) {
    let mut d = Document::new("t");
    let p = d.first_page().unwrap();
    let f = d.create(p, NodeKind::Frame, "F", 0.0, 0.0, 100.0, 100.0).unwrap();
    let r = d.create(f, NodeKind::Rectangle, "R", 10.0, 10.0, 20.0, 20.0).unwrap();
    d.get_mut(r).unwrap().fills = vec![Paint::solid(Color::BLACK)];
    (d, p, f, r)
}

fn build(d: &Document, p: NodeId) -> Result<(), &'static str> {
    Builder { doc: d, skip: &[], images: &mut Images::default() }.page(&mut Scene::new(), p, None)
}

#[test]
fn unsupported_features_fall_back() {
    let (mut d, p, _, r) = doc();
    assert_eq!(build(&d, p), Ok(()));
    d.get_mut(r).unwrap().effects.push(Effect::drop_shadow());
    assert!(build(&d, p).is_err_and(|why| why.starts_with("effects")));
    d.get_mut(r).unwrap().effects[0].visible = false;
    assert_eq!(build(&d, p), Ok(()), "hidden effects draw nothing");
    d.get_mut(r).unwrap().is_mask = true;
    assert_eq!(build(&d, p), Err("masks"));
    d.get_mut(r).unwrap().is_mask = false;
    d.get_mut(r).unwrap().fills = vec![Paint::pattern(1)];
    assert_eq!(build(&d, p), Err("pattern fills"));
    // Hidden layers aren't looked at.
    d.get_mut(r).unwrap().visible = false;
    assert_eq!(build(&d, p), Ok(()));
}

#[test]
fn an_edit_rebuilds_only_its_top_level_layer() {
    let (d, p, f, r) = doc();
    let mut moved = d.clone();
    moved.get_mut(r).unwrap().opacity = 0.5;
    assert_eq!(dirty_tops(&d, &moved, p), Some([f].into()));
    // A new top-level layer changes the page: everything is rebuilt.
    let mut added = d.clone();
    added.create(p, NodeKind::Rectangle, "S", 0.0, 0.0, 5.0, 5.0).unwrap();
    assert_eq!(dirty_tops(&d, &added, p), None);
}
