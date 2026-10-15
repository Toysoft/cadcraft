//! The secret a control-channel client presents with every request, and where it lives.
//!
//! The control server (`cadcraft --control PORT`) listens on the loopback interface only, but
//! every program on the machine can reach that, and so can a web page the user visits: a browser
//! sends `fetch` bodies to `127.0.0.1` without asking. Its methods open, save and write files as
//! the user, so a request has to prove it comes from someone who may act as the user: the token
//! is a random value the app writes to a file only the user can read
//! (`<settings>/control.token`, see [`config_dir`]), or the value of `CADCRAFT_CONTROL_TOKEN`
//! when that is set for the app and its clients alike.

use std::ffi::OsString;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::path::{Path, PathBuf};

/// The variable that sets the token for the app and its clients (`cadcraft-cli mcp --connect`).
pub const ENV: &str = "CADCRAFT_CONTROL_TOKEN";
/// The token file's name in the settings directory.
pub const FILE_NAME: &str = "control.token";

/// The per-user settings directory (it holds `logs/`, `autosave/`, `support/` and the control
/// token): `CADCRAFT_CONFIG_DIR` if set, else `$XDG_CONFIG_HOME/cadcraft` or `~/.config/cadcraft`
/// on Linux and the BSDs, `~/Library/Application Support/CADCraft` on macOS, `%APPDATA%\CADCraft`
/// on Windows.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(std::env::consts::OS, |k| std::env::var_os(k))
}

/// [`config_dir`] for operating system `os` (`std::env::consts::OS` names) with the environment
/// passed in (`var` looks a variable up). An empty variable counts as unset, so a blank value never
/// means the working directory.
pub fn config_dir_from(os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("CADCRAFT_CONFIG_DIR") {
        return Some(dir);
    }
    match os {
        "macos" => set("HOME").map(|h| h.join("Library/Application Support/CADCraft")),
        "windows" => set("APPDATA").map(|a| a.join("CADCraft")),
        // The XDG spec ignores a relative XDG_CONFIG_HOME.
        _ => set("XDG_CONFIG_HOME").filter(|p| p.is_absolute()).or_else(|| set("HOME").map(|h| h.join(".config"))).map(|c| c.join("cadcraft")),
    }
}

/// Where the app saves its token: `<settings>/control.token`, if there is a settings directory.
pub fn default_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(FILE_NAME))
}

/// The token `CADCRAFT_CONTROL_TOKEN` sets, if any (blank counts as unset).
pub fn from_env() -> Option<String> {
    std::env::var(ENV).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// A fresh random token: 128 bits as 32 hex digits. The standard library's hasher keys come
/// from the operating system's random source, so two hashers give two unpredictable words.
pub fn generate() -> String {
    let word = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u64(std::process::id().into());
        h.finish()
    };
    format!("{:016x}{:016x}", word(1), word(2))
}

/// Save `token` at `path`, readable by the user only (mode 0600 on Unix; elsewhere the settings
/// directory's own permissions apply).
pub fn write(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let mut f = open.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An older file keeps its mode through `open`; make sure of it.
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    use std::io::Write;
    f.write_all(token.as_bytes())?;
    f.write_all(b"\n")
}

/// The token saved at `path`, trimmed; an error when the file is missing or blank.
pub fn read(path: &Path) -> std::io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    let token = text.trim();
    if token.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: empty token file", path.display())));
    }
    Ok(token.to_string())
}

/// Whether `presented` is `expected`, compared in time that doesn't depend on where they differ.
pub fn matches(expected: &str, presented: &str) -> bool {
    let (a, b) = (expected.as_bytes(), presented.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (x, y) in a.iter().zip(b.iter().chain(std::iter::repeat(&0))) {
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn the_override_variable_wins_and_empty_values_are_unset() {
        let vars = [("CADCRAFT_CONFIG_DIR", "/tmp/cc"), ("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x"), ("APPDATA", "C:\\a")];
        for os in ["linux", "macos", "windows", "freebsd"] {
            assert_eq!(config_dir_from(os, env(&vars)), Some(PathBuf::from("/tmp/cc")), "{os}");
        }
        let empty = [("CADCRAFT_CONFIG_DIR", ""), ("HOME", ""), ("XDG_CONFIG_HOME", ""), ("APPDATA", "")];
        assert_eq!(config_dir_from("linux", env(&empty)), None, "no usable variable means no directory, not the working directory");
        assert_eq!(config_dir_from("linux", env(&[])), None);
    }

    #[test]
    fn linux_and_bsd_follow_xdg_config_home() {
        assert_eq!(config_dir_from("linux", env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/u")])), Some(PathBuf::from("/x/cadcraft")));
        assert_eq!(config_dir_from("freebsd", env(&[("HOME", "/home/u")])), Some(PathBuf::from("/home/u/.config/cadcraft")));
        // The XDG spec says a relative XDG_CONFIG_HOME is ignored.
        assert_eq!(
            config_dir_from("linux", env(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.config/cadcraft"))
        );
    }

    #[test]
    fn macos_uses_application_support_and_windows_appdata() {
        assert_eq!(config_dir_from("macos", env(&[("HOME", "/Users/u")])), Some(PathBuf::from("/Users/u/Library/Application Support/CADCraft")));
        assert_eq!(
            config_dir_from("windows", env(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")])),
            Some(PathBuf::from("C:\\Users\\u\\AppData\\Roaming").join("CADCraft"))
        );
    }

    #[test]
    fn tokens_are_long_random_and_compare_exactly() {
        let (a, b) = (generate(), generate());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(matches(&a, &a));
        assert!(!matches(&a, &b));
        assert!(!matches(&a, &a[..31]), "a prefix is not the token");
        assert!(!matches(&a, &format!("{a}0")), "nor is the token with more after it");
        assert!(!matches("", "x") && matches("", ""));
    }

    #[test]
    fn the_token_file_round_trips_and_is_private() {
        let dir = std::env::temp_dir().join(format!("cadcraft-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("deeper").join(FILE_NAME);
        write(&path, "abc123").unwrap();
        assert_eq!(read(&path).unwrap(), "abc123");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Rewritten, not appended to.
        write(&path, "def").unwrap();
        assert_eq!(read(&path).unwrap(), "def");
        std::fs::write(&path, "  \n").unwrap();
        assert!(read(&path).is_err(), "a blank file is no token");
        assert!(read(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
