//! Regenerates assets/snapit.ico:  cargo run -p snapit-gfx --example make_icon
fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/snapit.ico");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let bytes = snapit_gfx::brand::ico(&[16, 20, 24, 32, 40, 48, 64, 256]).unwrap();
    std::fs::write(&out, &bytes).unwrap();
    println!("wrote {} ({} bytes)", out.display(), bytes.len());
}
