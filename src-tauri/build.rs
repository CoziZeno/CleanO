fn main() {
    // Only with the `app` feature. The core test build (`--no-default-features`)
    // has no `tauri` dependency, and `tauri-build` panics when it cannot find
    // one. Cargo compiles build scripts with the package's features, so this
    // cfg is the real switch.
    #[cfg(feature = "app")]
    tauri_build::build();
}
