// SPDX-License-Identifier: MPL-2.0

fn main() {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=web-assets-api-version");
    let version: u32 = std::fs::read_to_string(manifest.join("web-assets-api-version"))
        .expect("resource API version file")
        .trim()
        .parse()
        .expect("resource API version must be a positive integer");
    assert!(version != 0, "resource API version cannot be zero");
    std::fs::write(
        output.join("web_assets_version.rs"),
        format!("pub const WEB_ASSETS_API_VERSION: u32 = {version};\n"),
    )
    .expect("generate resource compatibility version");
    // Host editor builds do not need ESP-IDF, Python, or the firmware UI bundle.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("espidf") {
        return;
    }
    println!("cargo:rustc-check-cfg=cfg(esp_idf_spiram)");
    println!("cargo:rerun-if-changed=src/server.html");
    println!("cargo:rerun-if-changed=src/editor/editor.html");
    println!("cargo:rerun-if-changed=src/editor/preview-worker.js");
    println!("cargo:rerun-if-changed=../../scripts/build-server-web.py");
    let status = std::process::Command::new("python3")
        .arg(manifest.join("../../scripts/build-server-web.py"))
        .arg(&manifest)
        .arg(output)
        .arg(version.to_string())
        .status()
        .expect("Python 3 is required to package the admin UI");
    assert!(status.success(), "admin UI packaging failed");
    embuild::espidf::sysenv::output();
}
