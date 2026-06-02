fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true) // for the #[cfg(test)] in-crate gRPC mock
        .build_client(true)
        .compile_protos(
            &[
                "../../proto/events/v1/msc.proto",
                "../../proto/msc_management/v1/service.proto",
            ],
            &["../../proto"],
        )?;
    Ok(())
}
