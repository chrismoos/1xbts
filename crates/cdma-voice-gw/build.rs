fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(
            &["../../proto/voice_gateway/v1/service.proto"],
            &["../../proto"],
        )?;
    Ok(())
}
