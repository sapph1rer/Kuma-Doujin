use std::path::PathBuf;

/// Per-user data directory: `%LOCALAPPDATA%\KumaDoujin` on Windows,
/// `$XDG_DATA_HOME/kumadoujin` or `~/.local/share/kumadoujin` elsewhere.
/// `KUMADOUJIN_DATA_DIR` overrides it.
pub fn data_dir() -> PathBuf {
    let env = |k| std::env::var_os(k).map(PathBuf::from);
    if let Some(dir) = env("KUMADOUJIN_DATA_DIR") {
        return dir;
    }
    if cfg!(windows)
        && let Some(dir) = env("LOCALAPPDATA")
    {
        return dir.join("KumaDoujin");
    }
    // shortcut: no macOS ~/Library path; upgrade to the `directories` crate if macOS matters.
    env("XDG_DATA_HOME")
        .or_else(|| env("HOME").map(|h| h.join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kumadoujin")
}

pub fn database_path() -> PathBuf {
    data_dir().join("kumadoujin.sqlite3")
}
