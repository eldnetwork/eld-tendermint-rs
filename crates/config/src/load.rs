//! Read `config.toml` and the genesis file it points at.
//!
//! `TMHOME` and `--home` are resolved here. Other viper environment overrides are left for a later CLI.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// `<home>/config.toml` if it exists, otherwise `<home>/config/config.toml`.
///
/// # Errors
///
/// Returns [`Error::ConfigNotFound`] when neither file exists.
pub fn find_config_file(home: &Path) -> Result<PathBuf, Error> {
    let direct = home.join("config.toml");
    if direct.is_file() {
        return Ok(direct);
    }
    let nested = home.join(crate::CONFIG_DIR).join("config.toml");
    if nested.is_file() {
        return Ok(nested);
    }
    Err(Error::ConfigNotFound {
        home: home.display().to_string(),
    })
}

/// Parse TOML bytes, starting from [`crate::Config::default_config`].
///
/// # Errors
///
/// Returns [`Error::Toml`] or [`Error::InvalidDuration`] when the file cannot be decoded.
/// This does not run `ValidateBasic`.
pub fn load_toml(bytes: &[u8]) -> Result<crate::Config, Error> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| Error::Toml("config is not utf-8".to_owned()))?;
    toml::from_str(text).map_err(|err| Error::Toml(err.to_string()))
}

/// Read a `config.toml` path. Does not run `ValidateBasic`.
///
/// # Errors
///
/// Returns an I/O error or a TOML decode error.
pub fn load_file(path: &Path) -> Result<crate::Config, Error> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    load_toml(&bytes)
}

/// Load the config for a node home, set `root_dir` to `home`, and run `ValidateBasic`.
///
/// # Errors
///
/// Returns an error when the file is missing, invalid, or fails `ValidateBasic`.
pub fn load_home(home: &Path) -> Result<crate::Config, Error> {
    let path = find_config_file(home)?;
    let mut config = load_file(&path)?;
    config.set_root(home);
    config.validate_basic()?;
    Ok(config)
}

/// `--home`, else `TMHOME`, else `$HOME/.tendermint`.
///
/// # Errors
///
/// Returns [`Error::HomeNotSet`] when neither a flag, `TMHOME`, nor `HOME` is set.
pub fn resolve_home(flag: Option<&str>) -> Result<PathBuf, Error> {
    if let Some(home) = flag.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    if let Ok(tmhome) = std::env::var("TMHOME") {
        if !tmhome.is_empty() {
            return Ok(PathBuf::from(tmhome));
        }
    }
    let home = std::env::var("HOME").map_err(|_| Error::HomeNotSet)?;
    Ok(PathBuf::from(home).join(crate::DEFAULT_TENDERMINT_DIR))
}
