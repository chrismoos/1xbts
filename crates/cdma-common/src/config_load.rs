//! JSON config loading with optional `<name>.local.json` overrides.
//!
//! Each node config (`bts.json`, `bsc.json`, ...) may have a sibling
//! `<name>.local.json` containing a sparse override. The override is
//! deep-merged on top of the base file before deserialization, so users can
//! customize a few fields without editing the checked-in defaults.
//!
//! Merge rules:
//! - Objects merge recursively, key by key.
//! - Arrays and scalars in the overlay replace whatever is in the base.
//! - `null` in the overlay deletes the key from the base.
//!
//! See `docs/CONFIG.md` for the user-facing documentation.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

/// Read `path` as JSON. If `<stem>.local.json` (or, when the input ends in
/// `.example.json`, `<stem-without-example>.local.json`) exists alongside it,
/// deep-merge that file on top before returning.
pub fn load_json_with_local_override(path: &Path) -> io::Result<Value> {
    let raw = fs::read_to_string(path)?;
    let mut base: Value = serde_json::from_str(&raw).map_err(io::Error::other)?;

    if let Some(local_path) = local_override_path(path) {
        if local_path.exists() {
            let local_raw = fs::read_to_string(&local_path)?;
            let overlay: Value = serde_json::from_str(&local_raw).map_err(io::Error::other)?;
            merge_json(&mut base, overlay);
        }
    }

    Ok(base)
}

/// Compute the `<name>.local.json` sibling path for a given config path.
/// Returns `None` if `path` already ends in `.local.json` (avoid recursion).
fn local_override_path(path: &Path) -> Option<std::path::PathBuf> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.ends_with(".local.json") {
        return None;
    }
    let stem = file_name
        .strip_suffix(".example.json")
        .or_else(|| file_name.strip_suffix(".json"))?;
    Some(path.with_file_name(format!("{stem}.local.json")))
}

/// Deep-merge `overlay` into `base`.
///
/// - Objects merge recursively.
/// - Arrays and scalars in `overlay` replace the value in `base`.
/// - A `null` value in `overlay` removes the key from `base`.
pub fn merge_json(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                if v.is_null() {
                    b.remove(&k);
                } else {
                    merge_json(b.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (b, o) => *b = o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_nested_objects() {
        let mut base = json!({ "a": { "x": 1, "y": 2 }, "b": 3 });
        merge_json(&mut base, json!({ "a": { "y": 20, "z": 30 } }));
        assert_eq!(base, json!({ "a": { "x": 1, "y": 20, "z": 30 }, "b": 3 }));
    }

    #[test]
    fn overlay_scalar_replaces_base() {
        let mut base = json!({ "a": 1 });
        merge_json(&mut base, json!({ "a": "two" }));
        assert_eq!(base, json!({ "a": "two" }));
    }

    #[test]
    fn overlay_array_replaces_base_array() {
        let mut base = json!({ "xs": [1, 2, 3] });
        merge_json(&mut base, json!({ "xs": [9] }));
        assert_eq!(base, json!({ "xs": [9] }));
    }

    #[test]
    fn null_overlay_deletes_key() {
        let mut base = json!({ "a": 1, "b": 2 });
        merge_json(&mut base, json!({ "a": null }));
        assert_eq!(base, json!({ "b": 2 }));
    }

    #[test]
    fn overlay_adds_new_key() {
        let mut base = json!({ "a": 1 });
        merge_json(&mut base, json!({ "b": 2 }));
        assert_eq!(base, json!({ "a": 1, "b": 2 }));
    }

    #[test]
    fn object_replaces_scalar() {
        let mut base = json!({ "a": 1 });
        merge_json(&mut base, json!({ "a": { "nested": true } }));
        assert_eq!(base, json!({ "a": { "nested": true } }));
    }

    #[test]
    fn local_override_path_for_plain_json() {
        assert_eq!(
            local_override_path(Path::new("/x/bts.json")),
            Some(Path::new("/x/bts.local.json").to_path_buf()),
        );
    }

    #[test]
    fn local_override_path_for_example_json() {
        assert_eq!(
            local_override_path(Path::new("/x/voice-gw.example.json")),
            Some(Path::new("/x/voice-gw.local.json").to_path_buf()),
        );
    }

    #[test]
    fn local_override_path_skips_local_json() {
        assert_eq!(local_override_path(Path::new("/x/bts.local.json")), None);
    }
}
