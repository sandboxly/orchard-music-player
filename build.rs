//! Windows: the icon (Explorer, Start menu, the uninstall entry) and version
//! information, as a resource compiled into the .exe. Nothing elsewhere.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
    }
}

fn windows_resource() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let icon = Path::new(&manifest_dir).join("packaging").join("icons").join("app.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let mut parts: Vec<u16> = version.split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let numeric = parts.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Orchard Music Player"
      VALUE "FileDescription", "Orchard Music Player"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "orchard-music-player"
      VALUE "LegalCopyright", "Stephan Lengl, MIT License"
      VALUE "OriginalFilename", "orchard-music-player.exe"
      VALUE "ProductName", "Orchard Music Player"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
    );
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("orchard-music-player.rc");
    std::fs::write(&out, rc).expect("write the resource script");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("compile the Windows resources");
}
