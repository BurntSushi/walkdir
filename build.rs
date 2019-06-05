#[cfg(not(target_os = "dragonfly"))]
fn main() {
    enable_getdents();
}

#[cfg(target_os = "dragonfly")]
fn main() {
    enable_getdents();
    cc::Build::new()
        .file("src/os/unix/errno-dragonfly.c")
        .compile("errno-dragonfly");
}

fn enable_getdents() {
    if std::env::var_os("CARGO_CFG_WALKDIR_DISABLE_GETDENTS").is_some() {
        return;
    }
    let os = match std::env::var("CARGO_CFG_TARGET_OS") {
        Err(_) => return,
        Ok(os) => os,
    };
    if os == "linux" {
        println!("cargo:rustc-cfg=walkdir_getdents");
    }
}
