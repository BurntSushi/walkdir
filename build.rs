fn main() {
    if check_cfg_supported() {
        println!("cargo:rustc-check-cfg=cfg(walkdir_getdents)");
        println!("cargo:rustc-check-cfg=cfg(walkdir_unix)");
    }

    let target_os = match std::env::var_os("CARGO_CFG_TARGET_OS") {
        Some(target_os) => target_os,
        None => return,
    };
    let target_os = target_os.to_string_lossy();
    if supports_unix_backend(&target_os) {
        println!("cargo:rustc-cfg=walkdir_unix");
    }
    if target_os == "dragonfly" {
        cc::Build::new()
            .file("src/os/unix/errno-dragonfly.c")
            .compile("errno-dragonfly");
    }
    if target_os == "linux"
        && std::env::var_os("CARGO_CFG_WALKDIR_DISABLE_GETDENTS").is_none()
    {
        println!("cargo:rustc-cfg=walkdir_getdents");
    }
}

fn check_cfg_supported() -> bool {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output =
        match std::process::Command::new(rustc).arg("--version").output() {
            Ok(output) => output,
            Err(_) => return false,
        };
    let version = String::from_utf8_lossy(&output.stdout);
    let minor = version
        .split_whitespace()
        .nth(1)
        .and_then(|version| version.split('.').nth(1))
        .and_then(|minor| minor.parse::<u32>().ok());
    matches!(minor, Some(minor) if minor >= 80)
}

fn supports_unix_backend(target_os: &str) -> bool {
    matches!(
        target_os,
        "android"
            | "dragonfly"
            | "emscripten"
            | "freebsd"
            | "fuchsia"
            | "haiku"
            | "linux"
            | "macos"
            | "netbsd"
            | "openbsd"
            | "solaris"
    )
}
