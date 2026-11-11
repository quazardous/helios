//! On Windows, embeds the program's resources in `helios-gui.exe`: its
//! icon, its version information and its application manifest
//! (`packaging/windows/`: per-monitor DPI awareness, long paths). Other
//! targets build nothing here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if !windows {
        return;
    }
    let icon = "../../packaging/windows/helios.ico";
    let manifest = "../../packaging/windows/helios-gui.manifest";
    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed={manifest}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set_manifest_file(manifest)
        .set("ProductName", "Helios")
        .set("FileDescription", "Helios")
        .set("CompanyName", "Helios contributors")
        .set("LegalCopyright", "GNU General Public License v3 or later");
    if let Err(e) = res.compile() {
        panic!("embedding the Windows resources: {e}");
    }
}
