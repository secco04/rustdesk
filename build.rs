#[cfg(windows)]
fn build_windows() {
    let file = "src/platform/windows.cc";
    let file2 = "src/platform/windows_delete_test_cert.cc";
    cc::Build::new().file(file).file(file2).compile("windows");
    println!("cargo:rustc-link-lib=WtsApi32");
    println!("cargo:rerun-if-changed={}", file);
    println!("cargo:rerun-if-changed={}", file2);
}

#[cfg(target_os = "macos")]
fn build_mac() {
    let file = "src/platform/macos.mm";
    let mut b = cc::Build::new();
    if let Ok(os_version::OsVersion::MacOS(v)) = os_version::detect() {
        let v = v.version;
        if v.contains("10.14") {
            b.flag("-DNO_InputMonitoringAuthStatus=1");
        }
    }
    b.flag("-std=c++17").file(file).compile("macos");
    println!("cargo:rerun-if-changed={}", file);
}

#[cfg(all(windows, feature = "inline"))]
fn build_manifest() {
    use std::io::Write;
    if std::env::var("PROFILE").unwrap() == "release" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("res/icon.ico")
            .set_language(winapi::um::winnt::MAKELANGID(
                winapi::um::winnt::LANG_ENGLISH,
                winapi::um::winnt::SUBLANG_ENGLISH_US,
            ))
            .set_manifest_file("res/manifest.xml");
        match res.compile() {
            Err(e) => {
                write!(std::io::stderr(), "{}", e).unwrap();
                std::process::exit(1);
            }
            Ok(_) => {}
        }
    }
}

fn install_android_deps() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "android" {
        return;
    }
    let mut target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    if target_arch == "x86_64" {
        target_arch = "x64".to_owned();
    } else if target_arch == "x86" {
        target_arch = "x86".to_owned();
    } else if target_arch == "aarch64" {
        target_arch = "arm64".to_owned();
    } else {
        target_arch = "arm".to_owned();
    }
    let target = format!("{}-android", target_arch);
    let vcpkg_root = std::env::var("VCPKG_ROOT").unwrap();
    let mut path: std::path::PathBuf = vcpkg_root.into();
    if let Ok(vcpkg_root) = std::env::var("VCPKG_INSTALLED_ROOT") {
        path = vcpkg_root.into();
    } else {
        path.push("installed");
    }
    path.push(target);
    println!(
        "cargo:rustc-link-search={}",
        path.join("lib").to_str().unwrap()
    );
    // M2 (plans/soft-frolicking-thimble.md): removed — nothing in this build actually needs
    // oboe/ndk_compat. cpal (the only real oboe consumer) is excluded for android in Cargo.toml
    // (see that file's comment), and audio_service.rs's own android path (`pa_impl`) already uses
    // `scrap::android::ffi::get_audio_raw()` directly, no oboe involved. These were an unconditional
    // blanket link requirement for every Android build regardless of what's actually referenced.
    // println!("cargo:rustc-link-lib=ndk_compat");
    // println!("cargo:rustc-link-lib=oboe");
    println!("cargo:rustc-link-lib=c++");
    println!("cargo:rustc-link-lib=OpenSLES");
}

// Inlined from hbb_common::gen_version() (see the Cargo.toml [build-dependencies] comment for why).
fn gen_version() {
    use std::io::{BufRead, Write};
    println!("cargo:rerun-if-changed=Cargo.toml");
    let mut file = std::fs::File::create("./src/version.rs").unwrap();
    let cargo_toml = std::io::BufReader::new(std::fs::File::open("Cargo.toml").unwrap());
    for line in cargo_toml.lines().map_while(Result::ok) {
        let ab: Vec<&str> = line.split('=').map(|x| x.trim()).collect();
        if ab.len() == 2 && ab[0] == "version" {
            file.write_all(format!("pub const VERSION: &str = {};\n", ab[1]).as_bytes())
                .ok();
            break;
        }
    }
    let build_date = format!("{}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    file.write_all(format!("#[allow(dead_code)]\npub const BUILD_DATE: &str = \"{build_date}\";\n").as_bytes())
        .ok();
}

fn main() {
    gen_version();
    install_android_deps();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    // M1 experiment (plans/soft-frolicking-thimble.md): these were gated on #[cfg(windows)], which
    // reflects the build SCRIPT's own (host) compile target, not CARGO_CFG_TARGET_OS (the crate's
    // actual cross-compile target) — always true when building on a Windows host regardless of
    // target, so cross-compiling to Android from Windows tried to compile windows.cc with
    // Windows.h. Never hit upstream because their own Android builds run from Linux/macOS hosts.
    #[cfg(all(windows, feature = "inline"))]
    if target_os == "windows" {
        build_manifest();
    }
    #[cfg(windows)]
    if target_os == "windows" {
        build_windows();
    }
    if target_os == "macos" {
        #[cfg(target_os = "macos")]
        build_mac();
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
