//! Tells the linker where libmpv is. libmpv2-sys only asks for it by name, which
//! isn't enough where it's installed outside the default search path, as Homebrew
//! and MSYS2 install it.

fn main() {
    if let Err(err) = pkg_config::probe_library("mpv") {
        println!("cargo:warning=pkg-config can't find libmpv: {err}");
    }
}
