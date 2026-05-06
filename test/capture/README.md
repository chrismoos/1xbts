# Capture Test Corpus

This directory is for large live-capture fixtures used by release-mode tests.
The fixture payloads are intentionally not tracked by git.

The checked-in files here are only the manifest and helper scripts:

- `manifest.toml` records the current archive URL and SHA-256.
- `create-archive.sh` creates the archive to upload to R2.
- `fetch-archive.sh` downloads, verifies, and extracts the archive for CI.

Checked-in golden IQ fixtures stay in `test/iq/` via Git LFS.

## Local Setup

After placing capture `.wav` files in this directory, create the upload archive
with:

```sh
test/capture/create-archive.sh
```

The script archives all `.wav`, `.json`, and `.mat` files directly in this
directory. It names the archive from the current short git hash by default,
prints the archive details, and updates `manifest.toml` with:

- `version`
- `archive_name`
- `archive_url`
- `archive_sha256`
- `archive_size`

Set `CAPTURE_VERSION` if you need to override the generated version:

```sh
CAPTURE_VERSION=capture-ci-custom test/capture/create-archive.sh
```

Upload the resulting archive to the printed `archive_url`, for example:

```text
https://ci.1xbts.org/captures/<archive_name>
```

## CI Download

CI runs `fetch-archive.sh` before tests. It skips downloading when capture files
already exist locally, otherwise it downloads `archive_url`, verifies
`archive_sha256`, and extracts the archive.

If `CAPTURE_DOWNLOAD_TOKEN` is set, the script sends it as `X-Capture-Token` so
the Cloudflare Worker can authorize CI downloads while still allowing rate
limits on the public endpoint.
