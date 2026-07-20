//! Smoke check for the `cdma-an` binary target.
//!
//! We don't spawn the process here — wiring a tokio server up in a unit-test
//! environment is more complexity than a "does the bin target exist" check
//! needs. Instead, we look for the compiled binary path that Cargo exposes
//! through `CARGO_BIN_EXE_<name>` to integration tests.

#[test]
fn cdma_an_binary_target_is_built() {
    let path = env!("CARGO_BIN_EXE_cdma-an");
    assert!(
        !path.is_empty(),
        "CARGO_BIN_EXE_cdma-an should resolve to a built binary path"
    );
    let p = std::path::Path::new(path);
    assert!(
        p.exists(),
        "cdma-an binary not found at {path}; binary target may be misconfigured"
    );
}
