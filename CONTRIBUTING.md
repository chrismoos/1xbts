# Contributing to 1xBTS

1xBTS is an experimental CDMA2000 1x base station and core-network stack. The
project is intended for research, interoperability testing, and lab operation.
Only operate RF hardware where you are authorized to transmit.

## Before Opening a Change

- Keep changes narrowly scoped. Separate protocol fixes, radio/PHY work, web UI
  changes, and cleanup into different pull requests when practical.
- Match existing crate and module boundaries. Public API changes should be
  intentional and called out in the pull request.
- Include tests for protocol, state-machine, encoding/decoding, scheduling, and
  retry behavior changes.
## Local Setup

Install the Rust toolchain with Cargo. Native builds may also need:

- `protoc` for protobuf code generation.
- `pkg-config`, clang/libclang, and a C compiler for native bindings.
- PostgreSQL for HLR/SMSC integration.
- Optional radio and voice libraries: UHD/USRP, LimeSuite, SoapySDR, bladeRF,
  and Baresip libre/re.

For dependency-light Rust checks, build packages with default features disabled
where appropriate. For full radio-enabled builds, install the native SDR
libraries used by the CI workflow.

## Validation

Run formatting before sending a change:

```sh
cargo fmt --all -- --check
```

Run focused tests for the area you changed:

```sh
cargo test -p cdma-bts
cargo test -p cdma-bsc
```

For broader validation, mirror the CI checks:

```sh
cargo build --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

The full workspace build uses default features, including native radio backends.
If your local machine does not have those dependencies, note which focused tests
you ran and let CI cover the full matrix.

## Web Dashboard

The web dashboard lives in `1xbts-web`:

```sh
npm install
npm run lint
npm run build
```

Regenerate protobuf bindings after changing files under `proto`:

```sh
npm run proto
```

## Pull Requests

In the pull request description, include:

- What changed and why.
- Tests or manual validation performed.
- Any RF, timing, database, migration, or interoperability implications.
- Any known limitations or follow-up work.

Do not include generated logs, captures, binaries, or large IQ/WAV files unless
they are explicitly needed for a test fixture and kept small.

## Issues

For bugs, include the affected crate or service, configuration, hardware backend
if applicable, expected behavior, observed behavior, and the smallest log excerpt
that demonstrates the problem.
