//! Dev helper: renders preview PNGs (icon, stickers, effects) into a folder.
//! cargo run -p snapit-gfx --example preview -- <out_dir>
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&dir).unwrap();
    let icon = snapit_gfx::brand::icon(128);
    snapit_gfx::io::save(&icon, &dir.join("icon.png"), snapit_gfx::io::Format::Png, 100).unwrap();
}
