fn main() {
    println!("cargo:rerun-if-changed=native/ddc.c");
    println!("cargo:rerun-if-env-changed=DDCUTIL_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=DDCUTIL_LIB_DIR");
    let mut build = cc::Build::new();
    build.file("native/ddc.c");
    if let Ok(path) = std::env::var("DDCUTIL_INCLUDE_DIR") {
        build.include(path);
    }
    build.compile("monitor_ddc");
    if let Ok(path) = std::env::var("DDCUTIL_LIB_DIR") {
        println!("cargo:rustc-link-search=native={path}");
    }
    println!("cargo:rustc-link-lib=ddcutil");
}
