fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Server scaffolding is needed only by the in-crate gRPC mock under
    // #[cfg(test)], but tonic-build emits both client and server when
    // build_server=true and gates them by traits — no runtime cost when
    // only the client is linked.
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &["../../proto/pdsn_management/v1/service.proto"],
            &["../../proto"],
        )?;
    Ok(())
}
