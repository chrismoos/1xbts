fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CARGO_FEATURE_GRPC").is_some() {
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&["../../proto/ms/v1/service.proto"], &["../../proto"])?;
    }
    Ok(())
}
