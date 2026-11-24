//! Locating the `assets/` directory at runtime.

use std::path::{Path, PathBuf};

/// Environment variable that overrides the assets directory.
pub const ASSETS_DIR_ENV: &str = "CHEMXEL_ASSETS_DIR";
/// Environment variable that overrides where worlds are saved.
pub const SAVES_DIR_ENV: &str = "CHEMXEL_SAVES_DIR";

/// Directory holding the saved worlds: `CHEMXEL_SAVES_DIR`, or `saves/` next to the
/// assets directory (the project root), so `cargo run` and opening the executable
/// directly use the same worlds.
pub fn saves_dir(assets: &Path) -> PathBuf {
    std::env::var_os(SAVES_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| assets.parent().map(|root| root.join("saves")))
        .unwrap_or_else(|| PathBuf::from("saves"))
}

/// Finds the `assets/` directory.
///
/// Search order:
/// 1. The `CHEMXEL_ASSETS_DIR` environment variable.
/// 2. `./assets` relative to the current working directory (the `cargo run` case).
/// 3. An `assets/` folder next to the executable or in any of its ancestors
///    (covers `target/release/chemxel.exe` and packaged builds).
///
/// Returns `None` if nothing is found; callers should fall back to defaults.
pub fn assets_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ASSETS_DIR_ENV) {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return Some(dir);
        }
        tracing::warn!(
            "{ASSETS_DIR_ENV}={} is not a directory; ignoring",
            dir.display()
        );
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = find_in(&cwd)
    {
        return Some(found);
    }

    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).find_map(find_in)
}

fn find_in(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join("assets");
    candidate.is_dir().then_some(candidate)
}
