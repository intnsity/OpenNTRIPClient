//! Build script: embed the application icon and VERSIONINFO block into the
//! Windows executable so Explorer, the taskbar, and the file Properties
//! dialog identify the tool. Every other target is a deliberate no-op.
//!
//! The guard reads CARGO_CFG_TARGET_OS (the TARGET, not the host), so the
//! script stays correct under cross-compilation; winresource itself then
//! needs a resource compiler (rc.exe on MSVC, windres on GNU) on the host.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // FILEVERSION/PRODUCTVERSION numerics are derived from CARGO_PKG_VERSION
    // by winresource; the string fields below are what users actually see.
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico");
    res.set("ProductName", "Open NTRIP Client");
    res.set("CompanyName", "Open NTRIP Client contributors");
    res.set(
        "FileDescription",
        "Open NTRIP Client - NTRIP/RTK diagnostic tool",
    );
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.set(
        "LegalCopyright",
        "(c) 2026 Open NTRIP Client contributors, GPL-3.0-or-later",
    );
    res.set("OriginalFilename", "OpenNtripClient.exe");
    res.set("InternalName", "OpenNtripClient.exe");
    // A release exe without its identity block is a defect: fail the build
    // loudly instead of shipping quietly unbranded.
    res.compile().expect("failed to compile Windows resources");
}
