//! On Windows, a program's main thread gets a 1 MiB stack, where Linux and
//! macOS give it 8 MiB. Devscriptor runs each command on its main thread, and
//! in a debug build the command's futures need more than 1 MiB; the binary
//! asks Windows for 8 MiB too.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.ends_with("-windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    } else if target.contains("-windows-gnu") {
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
    }
}
