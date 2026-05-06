#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$script_dir/manifest.toml"

read_manifest_value() {
  local key="$1"
  awk -F '"' -v key="$key" '$0 ~ "^" key " = " { print $2; exit }' "$manifest"
}

if [ "$#" -ne 0 ]; then
  echo "usage: $0" >&2
  exit 2
fi

git_short="$(git -C "$script_dir" rev-parse --short HEAD 2>/dev/null || echo nogit)"
version="${CAPTURE_VERSION:-capture-ci-${git_short}}"
archive_name="${version}.tar.zst"
archive_url_base="$(read_manifest_value archive_url)"
archive_url="${CAPTURE_ARCHIVE_URL:-${archive_url_base%/*}/${archive_name}}"
archive_path="$script_dir/$archive_name"

if ! command -v zstd >/dev/null 2>&1; then
  echo "zstd is required to create $archive_name" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to create deterministic tar metadata" >&2
  exit 1
fi

file_list="$(mktemp)"
trap 'rm -f "$file_list"' EXIT

find "$script_dir" -maxdepth 1 -type f \( -name '*.wav' -o -name '*.json' -o -name '*.mat' \) \
  -exec basename {} \; | LC_ALL=C sort > "$file_list"

if [ ! -s "$file_list" ]; then
  echo "no capture fixtures found in $script_dir" >&2
  exit 1
fi

python3 - "$script_dir" "$file_list" <<'PY' | zstd -19 -T0 -f -o "$archive_path" >/dev/null
import os
import sys
import tarfile

root = sys.argv[1]
file_list = sys.argv[2]

with open(file_list, "r", encoding="utf-8") as handle:
    files = [line.strip() for line in handle if line.strip()]

with tarfile.open(fileobj=sys.stdout.buffer, mode="w|") as archive:
    for rel_path in files:
        abs_path = os.path.join(root, rel_path)
        info = archive.gettarinfo(abs_path, arcname=rel_path)
        info.uid = 0
        info.gid = 0
        info.uname = ""
        info.gname = ""
        info.mtime = 0
        with open(abs_path, "rb") as source:
            archive.addfile(info, source)
PY

if command -v sha256sum >/dev/null 2>&1; then
  archive_sha256="$(sha256sum "$archive_path" | awk '{ print $1 }')"
else
  archive_sha256="$(shasum -a 256 "$archive_path" | awk '{ print $1 }')"
fi

archive_size="$(wc -c < "$archive_path" | tr -d ' ')"

python3 - "$manifest" "$version" "$archive_name" "$archive_url" "$archive_sha256" "$archive_size" <<'PY'
import sys

manifest, version, archive_name, archive_url, archive_sha256, archive_size = sys.argv[1:]
updates = {
    "version": f'"{version}"',
    "archive_name": f'"{archive_name}"',
    "archive_url": f'"{archive_url}"',
    "archive_sha256": f'"{archive_sha256}"',
    "archive_size": archive_size,
}

with open(manifest, "r", encoding="utf-8") as handle:
    lines = handle.readlines()

seen = set()
out = []
for line in lines:
    stripped = line.strip()
    replaced = False
    for key, value in updates.items():
        if stripped.startswith(f"{key} = "):
            out.append(f"{key} = {value}\n")
            seen.add(key)
            replaced = True
            break
    if not replaced:
        out.append(line)

for key, value in updates.items():
    if key not in seen:
        out.append(f"{key} = {value}\n")

with open(manifest, "w", encoding="utf-8") as handle:
    handle.writelines(out)
PY

echo "archive_path=$archive_path"
echo "archive_url=$archive_url"
echo "archive_size=$archive_size"
echo "archive_sha256=$archive_sha256"
echo "updated_manifest=$manifest"
