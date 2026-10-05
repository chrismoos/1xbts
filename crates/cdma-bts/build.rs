fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(
            &[
                "../../proto/bsc/v1/service.proto",
                "../../proto/bts_management/v1/service.proto",
            ],
            &["../../proto"],
        )?;
    if std::env::var_os("CARGO_FEATURE_NETWORK_BACKEND").is_some() {
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&["../../proto/radio/v1/service.proto"], &["../../proto"])?;
    }
    Ok(())
}
