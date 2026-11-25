use embed_manifest::manifest::{DpiAwareness, ExecutionLevel};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    // asInvoker: never request elevation. PerMonitorV2: crisp capture on mixed-DPI setups.
    // (The default manifest also opts into Common Controls v6 and the UTF-8 code page.)
    embed_manifest(
        new_manifest("SnapIt")
            .requested_execution_level(ExecutionLevel::AsInvoker)
            .dpi_awareness(DpiAwareness::PerMonitorV2),
    )
    .expect("unable to embed manifest");

    // Icon + version information. Binaries without these look anonymous to AV heuristics.
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("../../assets/snapit.ico").canonicalize().expect("assets/snapit.ico (run: cargo run -p snapit-gfx --example make_icon)");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u32> = version.split(['.', '-']).filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let commas = parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    let icon_path = icon.to_string_lossy().replace('\\', "\\\\").trim_start_matches("\\\\\\\\?\\\\").to_string();
    let rc = format!(
        r#"1 ICON "{icon_path}"
1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Snap It"
      VALUE "FileDescription", "Snap It screenshot tool"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "snapit"
      VALUE "OriginalFilename", "snapit.exe"
      VALUE "ProductName", "Snap It"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("snapit.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/snapit.ico");
}
