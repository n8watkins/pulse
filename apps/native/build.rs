fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=fonts");
    println!("cargo:rerun-if-changed=assets");
    slint_build::compile("ui/panel.slint").expect("compile ui/panel.slint");
    embed_windows_resources();
}

/// Icon, version details and manifest for the exe.
///
/// Only runs when the build host is Windows (the resource compiler is `rc.exe`).
/// Checking or linting for the Windows target from WSL skips it, so `cargo clippy --target
/// x86_64-pc-windows-msvc` needs no resource tools.
#[cfg(windows)]
fn embed_windows_resources() {
    use std::path::PathBuf;

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut nums = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
    );
    let asset = |name: &str| {
        manifest_dir
            .join("assets")
            .join(name)
            .to_string_lossy()
            .replace('\\', "/")
    };

    let rc = format!(
        r#"#include <winver.h>
1 ICON "{icon}"
1 RT_MANIFEST "{manifest}"
VS_VERSION_INFO VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE VFT2_UNKNOWN
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "n8builds.dev"
      VALUE "FileDescription", "Pulse"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Pulse"
      VALUE "LegalCopyright", "Copyright (c) 2026 n8builds.dev"
      VALUE "OriginalFilename", "Pulse.exe"
      VALUE "ProductName", "Pulse"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = asset("pulse.ico"),
        manifest = asset("pulse.manifest"),
    );
    let rc_path = out_dir.join("pulse.rc");
    std::fs::write(&rc_path, rc).expect("write pulse.rc");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile resources");
}

#[cfg(not(windows))]
fn embed_windows_resources() {}
