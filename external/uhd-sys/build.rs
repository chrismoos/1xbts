use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let lib = pkg_config::probe_library("uhd").expect("Could not find UHD via pkg-config");

    let uhd_include_path = lib
        .include_paths
        .first()
        .expect("no include path for UHD headers");
    generate_bindings(uhd_include_path);
}

fn generate_bindings(include_path: &Path) {
    let usrp_header = include_path.join("uhd.h");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let out_path = out_dir.join("bindgen.rs");

    let mut builder = bindgen::builder()
        .allowlist_function("^uhd.+")
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .header(usrp_header.to_string_lossy().to_string())
        .clang_arg(format!("-I{}", include_path.to_string_lossy()));

    let target = env::var("TARGET").expect("No TARGET environment variable");
    if target == "armv7-unknown-linux-gnueabihf" {
        builder = builder.clang_arg("-I/usr/lib/gcc/arm-linux-gnueabihf/8/include");
    } else if target == "aarch64-apple-darwin" {
        println!("cargo:rustc-link-search=/opt/homebrew/lib/");
    }

    let bindings = builder.generate().expect("Failed to generate bindings");
    bindings
        .write_to_file(out_path)
        .expect("Failed to write bindings to file");
}
