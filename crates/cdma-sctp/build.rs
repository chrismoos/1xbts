use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(usrsctp_backend)");
    println!("cargo:rustc-check-cfg=cfg(kernel_backend)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("set by cargo");
    if target_os == "linux" {
        println!("cargo:rustc-cfg=kernel_backend");
    }
    if target_os != "linux" || env::var_os("CARGO_FEATURE_USRSCTP").is_some() {
        println!("cargo:rustc-cfg=usrsctp_backend");
    }
}
