#[cfg(not(target_os = "dragonfly"))]
fn main() {}

#[cfg(target_os = "dragonfly")]
fn main() {
    cc::Build::new()
        .file("src/os/unix/errno-dragonfly.c")
        .compile("errno-dragonfly");
}
