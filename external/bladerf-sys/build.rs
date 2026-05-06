use std::env;
use std::path::PathBuf;

fn main() {
    let lib =
        pkg_config::probe_library("libbladeRF").expect("Could not find libbladeRF via pkg-config");

    let include_path = lib
        .include_paths
        .first()
        .expect("no include path for libbladeRF headers");

    let header = include_path.join("libbladeRF.h");
    if !header.exists() {
        let alt = include_path.join("bladeRF").join("libbladeRF.h");
        if alt.exists() {
            generate_bindings(&alt, include_path);
            return;
        }
        panic!("Could not find libbladeRF.h at {:?} or {:?}", header, alt);
    }
    generate_bindings(&header, include_path);
}

fn generate_bindings(header: &std::path::Path, include_path: &std::path::Path) {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let out_path = out_dir.join("bindgen.rs");

    let bindings = bindgen::builder()
        .header(header.to_string_lossy().to_string())
        .clang_arg(format!("-I{}", include_path.to_string_lossy()))
        .allowlist_function("^bladerf_.+")
        .allowlist_type("^bladerf_.+")
        .allowlist_var("^BLADERF_.+")
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .generate()
        .expect("Failed to generate libbladeRF bindings");

    bindings
        .write_to_file(out_path)
        .expect("Failed to write bindings to file");
}
