fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                "../../proto/bsc/v1/service.proto",
                "../../proto/events/v1/msc.proto",
                "../../proto/msc_management/v1/service.proto",
                "../../proto/voice_gateway/v1/service.proto",
            ],
            &["../../proto"],
        )?;
    Ok(())
}
