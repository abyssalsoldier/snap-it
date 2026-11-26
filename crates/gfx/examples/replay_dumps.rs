//! Replays region-snapping F9 dumps (with Settings > Capture > Developer mode on, F9 in the region overlay saves
//! `<save dir>/debug/snap_*.{raw.png,txt}`) through [`edges::boxes_at`], printing
//! the boxes and timing for each, so a change can be checked against real screens:
//!
//! ```text
//! cargo run --release -p snapit-gfx --example replay_dumps -- "<save dir>/debug"
//! ```
use snapit_gfx::{edges, Rect};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("debug dir"));
    let mut txts: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    txts.sort();
    let mut cache: Option<(u64, snapit_gfx::tiny_skia::Pixmap)> = None;
    for t in txts {
        let s = std::fs::read_to_string(&t).unwrap();
        let c = s.lines().next().unwrap().split_whitespace().nth(1).unwrap();
        let (x, y) = c.split_once(',').unwrap();
        let (x, y): (i32, i32) = (x.parse().unwrap(), y.parse().unwrap());
        // Window rect from the report ("  rect Rect { x: .., y: .., w: .., h: .. }").
        let nums: Vec<i32> = s.lines().find(|l| l.trim_start().starts_with("rect Rect")).map(|l| l.split(|c: char| !c.is_ascii_digit() && c != '-').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
        let bounds = if nums.len() == 4 { Rect::new(nums[0], nums[1], nums[2], nums[3]) } else { continue };
        let raw = t.with_extension("raw.png");
        let len = std::fs::metadata(&raw).unwrap().len();
        if cache.as_ref().map(|c| c.0) != Some(len) {
            cache = Some((len, snapit_gfx::io::load(&raw).unwrap()));
        }
        let pm = &cache.as_ref().unwrap().1;
        let t0 = std::time::Instant::now();
        let v = edges::boxes_at(pm, x, y, bounds);
        let dt = t0.elapsed();
        let title = s.lines().find(|l| l.starts_with("window")).unwrap_or("").chars().take(40).collect::<String>();
        println!("{} {x},{y} [{title}] {:.2} ms", &t.file_stem().unwrap().to_string_lossy()[5..], dt.as_secs_f64() * 1000.0);
        for (r, fine) in v {
            println!("    {}{},{} {}x{}", if fine { "fine " } else { "     " }, r.x, r.y, r.w, r.h);
        }
    }
}
