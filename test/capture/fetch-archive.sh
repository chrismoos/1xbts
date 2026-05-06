#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$script_dir/manifest.toml"

read_manifest_value() {
  local key="$1"
  awk -F '"' -v key="$key" '$0 ~ "^" key " = " { print $2; exit }' "$manifest"
}

archive_name="$(read_manifest_value archive_name)"
archive_url="$(read_manifest_value archive_url)"
archive_sha256="$(read_manifest_value archive_sha256)"

capture_files_present() {
  find "$script_dir" -maxdepth 1 -type f \( -name '*.wav' -o -name '*.json' -o -name '*.mat' \) \
    -print -quit | grep -q .
}

if capture_files_present; then
  echo "capture fixtures already present"
  exit 0
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required to fetch capture fixtures" >&2
  exit 1
fi

if ! command -v zstd >/dev/null 2>&1; then
  echo "zstd is required to extract capture fixtures" >&2
  exit 1
fi

curl_args=(--fail --location --retry 5 --retry-delay 2)
if [ -n "${CAPTURE_DOWNLOAD_TOKEN:-}" ]; then
  curl_args+=(--header "X-Capture-Token: ${CAPTURE_DOWNLOAD_TOKEN}")
fi

echo "downloading capture fixtures from $archive_url"

if [ -n "$archive_sha256" ] && [ "$archive_sha256" != "REPLACE_WITH_SHA256_AFTER_UPLOAD" ]; then
  # Stream through tee so we can verify the hash without writing a full copy to disk.
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
  hash_file="$tmp_dir/archive.sha256"

  curl "${curl_args[@]}" "$archive_url" \
    | tee >(sha256sum | cut -d' ' -f1 > "$hash_file") \
    | zstd -dc | tar -xf - -C "$script_dir"

  actual_sha256="$(cat "$hash_file")"
  if [ "$actual_sha256" != "$archive_sha256" ]; then
    echo "capture archive sha256 mismatch: expected $archive_sha256 got $actual_sha256" >&2
    exit 1
  fi
else
  echo "warning: manifest archive_sha256 is not set; skipping archive verification" >&2
  curl "${curl_args[@]}" "$archive_url" | zstd -dc | tar -xf - -C "$script_dir"
fi

if ! capture_files_present; then
  echo "capture archive extracted but no capture fixture files were found" >&2
  exit 1
fi

echo "capture fixtures ready"
