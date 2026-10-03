//! `eld-tendermint unsafe-reset-all`. Deletes chain data without opening a database.

use std::io;
use std::path::Path;

use eld_tendermint_config::load_home;
use eld_tendermint_privval::FilePV;

use crate::error::{Error, fail};

/// Removes `data/` (or a configured `db_dir`) and the address book, then writes
/// a height-0 `priv_validator_state.json`.
///
/// Config, genesis, and key files are left in place. A missing data directory
/// or address book is not an error.
///
/// # Errors
///
/// Returns an error when the home config cannot be loaded or a delete or write fails.
pub(crate) fn unsafe_reset_all(home: &Path, keep_addr_book: bool) -> Result<(), Error> {
    let config = load_home(home).map_err(fail)?;
    let db_dir = config.db_dir();
    remove_dir_if_present(&db_dir)?;
    std::fs::create_dir_all(&db_dir).map_err(fail)?;
    if !keep_addr_book {
        remove_file_if_present(&config.p2p.addr_book_file())?;
    }
    let state_file = config.priv_validator_state_file();
    create_parent(&state_file)?;
    FilePV::save_reset_state(&state_file).map_err(fail)
}

fn remove_dir_if_present(path: &Path) -> Result<(), Error> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::new(format!(
            "failed to remove {}: {err}",
            path.display()
        ))),
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::new(format!(
            "failed to remove {}: {err}",
            path.display()
        ))),
    }
}

fn create_parent(path: &Path) -> Result<(), Error> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).map_err(fail)
}
