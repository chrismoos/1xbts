fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                "../../proto/an/v1/service.proto",
                "../../proto/events/v1/an.proto",
            ],
            &["../../proto"],
        )?;
    Ok(())
}
