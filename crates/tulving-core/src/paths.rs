//! Where state lives: `$TULVING_HOME`, else `~/.tulving`. One path on
//! every platform, so harnesses and docs never need a lookup table.

use std::path::PathBuf;

/// State home: `$TULVING_HOME`, else `~/.tulving`.
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("TULVING_HOME") {
        return PathBuf::from(h);
    }
    let user_home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(user_home).join(".tulving")
}

/// The ledger file: `<home>/tulving.db` (WAL sidecars appear beside it).
pub fn ledger_path() -> PathBuf {
    home().join("tulving.db")
}
