use std::env;
use std::path::PathBuf;

fn main() {
    let lib =
        pkg_config::probe_library("LimeSuite").expect("Could not find LimeSuite via pkg-config");

    let include_path = lib
        .include_paths
        .first()
        .expect("no include path for LimeSuite headers");

    // LimeSuite.h is typically at <include>/lime/LimeSuite.h
    let header = include_path.join("lime").join("LimeSuite.h");
    if !header.exists() {
        // Fall back to direct include path
        let alt = include_path.join("LimeSuite.h");
        if alt.exists() {
            generate_bindings(&alt, include_path);
            return;
        }
        panic!("Could not find LimeSuite.h at {:?} or {:?}", header, alt);
    }
    generate_bindings(&header, include_path);
}

fn generate_bindings(header: &std::path::Path, include_path: &std::path::Path) {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let out_path = out_dir.join("bindgen.rs");

    let bindings = bindgen::builder()
        .header(header.to_string_lossy().to_string())
        .clang_arg(format!("-I{}", include_path.to_string_lossy()))
        .allowlist_function("^LMS_.+")
        .allowlist_type("^lms_.+")
        .allowlist_var("^LMS_.+")
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .generate()
        .expect("Failed to generate LimeSuite bindings");

    bindings
        .write_to_file(out_path)
        .expect("Failed to write bindings to file");
}
