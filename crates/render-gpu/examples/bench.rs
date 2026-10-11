//! CPU vs GPU canvas frame times on the synthetic pages of `cargo xtask bench`:
//! `cargo run --release -p figcraft-render-gpu --example bench [-- 1000,10000,50000]`.
//!
//! CPU rows are the canvas path (`Retained`, levels of detail on). GPU rows wait for the GPU to
//! finish ("gpu"), then also copy the image back to the CPU ("+ readback", what the prototype's
//! canvas does before uploading it as a texture).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Instant;

use figcraft_doc::{Document, NodeId};
use figcraft_render::{Options, Retained, View, render_view};
use figcraft_render_gpu::Canvas;

fn mean(n: usize, mut f: impl FnMut(usize)) -> f64 {
    let t = Instant::now();
    for i in 0..n {
        f(i);
    }
    t.elapsed().as_secs_f64() * 1000.0 / n.max(1) as f64
}

fn canvas_opts() -> Options {
    Options { lod: true, ..Default::default() }
}

struct Rows(Vec<(String, Vec<f64>)>);

impl Rows {
    fn put(&mut self, col: usize, name: String, v: f64) {
        let i = self.0.iter().position(|r| r.0 == name).unwrap_or_else(|| {
            self.0.push((name, Vec::new()));
            self.0.len() - 1
        });
        let row = &mut self.0[i].1;
        row.resize(col + 1, f64::NAN);
        row[col] = v;
    }
}

/// A copy of `doc` with the last layer of the screen nearest `view`'s centre moved by `k` × 6 units.
fn moved(doc: &Document, page: NodeId, view: &View, k: f64) -> Arc<Document> {
    let c = view.page_rect().center();
    let screens = doc.node(page).map(|p| p.children.clone()).unwrap_or_default();
    let nearest = screens.into_iter().filter_map(|s| doc.world_bounds(s).map(|b| (s, (b.center() - c).hypot2()))).min_by(|a, b| a.1.total_cmp(&b.1));
    let mut d = doc.clone();
    if let Some(target) = nearest.and_then(|(s, _)| doc.node(s)?.children.last().copied())
        && let Some(n) = d.node_mut(target)
    {
        n.transform = figcraft_geom::Affine::translate((k * 6.0, 0.0)) * n.transform;
    }
    Arc::new(d)
}

fn measure(rows: &mut Rows, col: usize, gpu: &mut Canvas, layers: usize) {
    let (doc, page) = figcraft_engine::synthetic::large_document(layers).expect("synthetic document");
    let bounds = doc.page_content_bounds(page).expect("content");
    let doc = Arc::new(doc);
    let fit = |w: u32, h: u32, dpr: f64| {
        let s = (f64::from(w) / bounds.width()).min(f64::from(h) / bounds.height()) * 2.0;
        View { origin: (bounds.x0, bounds.y0), scale: s.min(dpr), width: w, height: h }
    };
    let views = [
        ("1440×900 @100%", View { origin: (0.0, 0.0), scale: 1.0, width: 1440, height: 900 }),
        ("1440×900 zoomed out (2× fit)", fit(1440, 900, 1.0)),
        ("4K @2x, 100%", View { origin: (0.0, 0.0), scale: 2.0, width: 3840, height: 2160 }),
        ("4K @2x, zoomed out (2× fit)", fit(3840, 2160, 2.0)),
    ];
    let opts = canvas_opts();
    for (vname, view) in views {
        let pan =
            |v: &View, i: usize| View { origin: (v.origin.0 + if i.is_multiple_of(2) { 40.0 / v.scale } else { 0.0 }, v.origin.1), ..v.clone() };
        let zoom = |v: &View, i: usize| View { scale: v.scale * if i.is_multiple_of(2) { 1.1 } else { 1.0 }, ..v.clone() };
        let edits = [moved(&doc, page, &view, 1.0), moved(&doc, page, &view, 0.0)];
        // CPU, as the canvas renders today.
        rows.put(col, format!("{vname}: CPU full render"), mean(3, |_| _ = render_view(&doc, page, &view, &opts)));
        rows.put(
            col,
            format!("{vname}: CPU full render, exact (no LOD, as the GPU draws)"),
            mean(3, |_| _ = render_view(&doc, page, &view, &Options::default())),
        );
        let mut r = Retained::default();
        let _ = r.render(&doc, page, &view, &opts);
        rows.put(col, format!("{vname}: CPU pan 40 px (retained)"), mean(10, |i| _ = r.render(&doc, page, &pan(&view, i), &opts)));
        rows.put(col, format!("{vname}: CPU zoom step (retained)"), mean(4, |i| _ = r.render(&doc, page, &zoom(&view, i), &opts)));
        let _ = r.render(&doc, page, &view, &opts);
        rows.put(col, format!("{vname}: CPU one-layer edit (retained)"), mean(10, |i| _ = r.render(&edits[i % 2], page, &view, &opts)));
        // GPU.
        let ok = gpu.draw(&doc, page, &view, &opts);
        if let Err(why) = ok {
            eprintln!("{vname}: GPU falls back: {why}");
            continue;
        }
        gpu.gpu.wait();
        for (suffix, read) in [("gpu", false), ("gpu + readback", true)] {
            let frame = |gpu: &mut Canvas, d: &Arc<Document>, v: &View| {
                if gpu.draw(d, page, v, &opts).is_ok() {
                    if read {
                        let _ = gpu.gpu.read();
                    } else {
                        gpu.gpu.wait();
                    }
                }
            };
            rows.put(
                col,
                format!("{vname}: GPU full render, scene built ({suffix})"),
                mean(3, |_| {
                    gpu.forget();
                    frame(gpu, &doc, &view);
                }),
            );
            frame(gpu, &doc, &view);
            rows.put(col, format!("{vname}: GPU pan 40 px ({suffix})"), mean(10, |i| frame(gpu, &doc, &pan(&view, i))));
            rows.put(col, format!("{vname}: GPU zoom step ({suffix})"), mean(10, |i| frame(gpu, &doc, &zoom(&view, i))));
            rows.put(col, format!("{vname}: GPU one-layer edit ({suffix})"), mean(10, |i| frame(gpu, &edits[i % 2], &view)));
        }
    }
}

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "1000,10000,50000".into())
        .split(',')
        .filter_map(|s| s.trim().replace('_', "").parse().ok())
        .collect();
    figcraft_text::scan_system_fonts();
    let t = Instant::now();
    let mut gpu = Canvas::new().expect("a GPU adapter");
    println!("GPU: {} (device + renderer {:.0} ms)", gpu.gpu.adapter, t.elapsed().as_secs_f64() * 1000.0);
    let mut rows = Rows(Vec::new());
    for (col, n) in sizes.iter().enumerate() {
        eprintln!("measuring {n} layers…");
        measure(&mut rows, col, &mut gpu, *n);
    }
    let w = rows.0.iter().map(|r| r.0.chars().count()).max().unwrap_or(10);
    let mut head = format!("| {:<w$} |", "ms (mean)");
    let mut rule = format!("|{}|", "-".repeat(w + 2));
    for n in &sizes {
        head.push_str(&format!(" {:>12} |", format!("{n} layers")));
        rule.push_str(&format!("{}:|", "-".repeat(13)));
    }
    println!("{head}\n{rule}");
    for (name, vals) in &rows.0 {
        let cells: String = vals.iter().map(|v| format!(" {:>12} |", if v.is_finite() { format!("{v:.2}") } else { "-".into() })).collect();
        println!("| {name:<w$} |{cells}");
    }
}
