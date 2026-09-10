use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// Settings, read from `~/.config/caffeine-daemon/config.toml`. Every key is
/// optional; a missing file means the defaults below.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub bind: String,
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
    /// When set, callers must present this key; when unset, nothing is checked.
    #[serde(rename = "api-key")]
    pub api_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8787".to_owned(),
            ttl: Duration::from_secs(600),
            api_key: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn Error>> {
        match path() {
            Some(path) => Self::read(&path),
            None => Ok(Self::default()),
        }
    }

    pub fn read(path: &Path) -> Result<Self, Box<dyn Error>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("cannot read {}: {err}", path.display()).into()),
        };
        let mut config: Config = toml::from_str(&raw)
            .map_err(|err| format!("cannot parse {}: {err}", path.display()))?;
        // An empty key would lock every caller out rather than open the API up.
        config.api_key = config.api_key.filter(|key| !key.is_empty());
        Ok(config)
    }
}

impl Config {
    /// The port out of `bind`, for callers that need to reach this daemon.
    pub fn port(&self) -> Option<u16> {
        self.bind.rsplit_once(':')?.1.parse().ok()
    }
}

pub fn path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("caffeine-daemon").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_means_defaults() {
        let config = Config::read(Path::new("/nonexistent/caffeine.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn file_values_win_over_defaults() {
        let dir = std::env::temp_dir().join(format!("caffeine-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "ttl = \"90s\"\nbind = \"127.0.0.1:9000\"\n").unwrap();

        let config = Config::read(&path).unwrap();
        assert_eq!(config.bind, "127.0.0.1:9000");
        assert_eq!(config.ttl, Duration::from_secs(90));
        assert_eq!(config.api_key, None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_api_key_is_read_when_present() {
        let dir = std::env::temp_dir().join(format!("caffeine-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "api-key = \"s3cret\"\n").unwrap();

        let config = Config::read(&path).unwrap();
        assert_eq!(config.api_key.as_deref(), Some("s3cret"));
        assert_eq!(config.bind, Config::default().bind);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_port_comes_out_of_bind() {
        assert_eq!(Config::default().port(), Some(8787));
        assert_eq!(
            Config {
                bind: "127.0.0.1:9000".into(),
                ..Config::default()
            }
            .port(),
            Some(9000)
        );
        assert_eq!(
            Config {
                bind: "nonsense".into(),
                ..Config::default()
            }
            .port(),
            None
        );
    }

    #[test]
    fn an_empty_api_key_counts_as_unset() {
        let dir = std::env::temp_dir().join(format!("caffeine-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "api-key = \"\"\n").unwrap();

        assert_eq!(Config::read(&path).unwrap().api_key, None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let dir = std::env::temp_dir().join(format!("caffeine-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "nope = 1\n").unwrap();

        assert!(Config::read(&path).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
