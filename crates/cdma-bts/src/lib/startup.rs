//! Process bootstrap for the BTS binary.
//!
//! Config-directory resolution and log-filter policy are shared with every
//! other component. The BTS config file name and its profile lookup are local.

use std::path::{Path, PathBuf};

use cdma_common::error::Error;

pub use cdma_common::startup::{
    CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR, DEFAULT_LOG_FILTER, init_logging, resolve_config_dir,
    wait_for_shutdown,
};

/// BTS node config file name within the config directory.
pub const BTS_CONFIG_FILENAME: &str = "bts.json";

/// Resolve `bts.<profile>.json` within the config directory. Profile names are
/// restricted so a profile cannot address a file outside that directory.
pub fn resolve_bts_profile_path(config_dir: &Path, profile: &str) -> Result<PathBuf, Error> {
    if profile.is_empty()
        || !profile
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "invalid BTS profile {profile:?}; use lowercase letters, digits, and hyphens"
        )
        .into());
    }
    Ok(config_dir.join(format!("bts.{profile}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bts_profile_path_uses_config_directory() {
        let path = resolve_bts_profile_path(Path::new("config"), "sprint").expect("valid profile");
        assert_eq!(path, Path::new("config").join("bts.sprint.json"));
    }

    #[test]
    fn bts_profile_name_rejects_paths() {
        assert!(resolve_bts_profile_path(Path::new("config"), "../evil").is_err());
        assert!(resolve_bts_profile_path(Path::new("config"), "UPPER").is_err());
        assert!(resolve_bts_profile_path(Path::new("config"), "").is_err());
    }
}
