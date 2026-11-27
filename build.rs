fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let source = "src/ui/macos/PlayerForm.m";
        println!("cargo:rerun-if-changed={source}");
        println!("cargo:rerun-if-changed=src/ui/macos/GlassOverlay.m");
        println!("cargo:rustc-link-lib=framework=OpenGL");
        println!("cargo:rustc-link-lib=framework=QuartzCore");
        cc::Build::new()
            .file(source)
            .file("src/ui/macos/GlassOverlay.m")
            .flag("-fobjc-arc")
            .compile("mytube_forms");
    }
}
