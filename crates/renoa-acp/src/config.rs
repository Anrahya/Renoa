use std::{env, path::PathBuf};

use renoa_local::LocalHost;

use crate::ServerError;

/// Process configuration for the local ACP adapter.
pub struct Config {
    host: LocalHost,
}

impl Config {
    /// Reads the local runtime and durable data paths from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when required provider settings or a usable data directory are absent.
    pub fn from_environment() -> Result<Self, ServerError> {
        let data_directory = data_directory()?;
        Ok(Self {
            host: LocalHost::new(
                data_directory.join("sessions"),
                required_path("RENOA_PI_BRIDGE")?,
                required("RENOA_PI_PROVIDER")?,
                required("RENOA_PI_MODEL")?,
                required_path("RENOA_PI_AUTH_STORE")?,
            )?,
        })
    }

    pub(crate) const fn host(&self) -> &LocalHost {
        &self.host
    }
}

fn required(name: &str) -> Result<String, ServerError> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::Configuration(format!("{name} must be set")))
}

fn required_path(name: &str) -> Result<PathBuf, ServerError> {
    required(name).map(PathBuf::from)
}

fn data_directory() -> Result<PathBuf, ServerError> {
    if let Some(path) = env::var_os("RENOA_DATA_DIR").filter(|path| !path.is_empty()) {
        return absolute(PathBuf::from(path));
    }
    #[cfg(target_os = "macos")]
    {
        home_directory().map(|home| home.join("Library/Application Support/Renoa"))
    }
    #[cfg(target_os = "windows")]
    {
        return env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Renoa"))
            .ok_or_else(|| ServerError::Configuration("LOCALAPPDATA must be set".to_owned()));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = env::var_os("XDG_DATA_HOME").filter(|path| !path.is_empty()) {
            return absolute(PathBuf::from(path).join("renoa"));
        }
        home_directory().map(|home| home.join(".local/share/renoa"))
    }
}

fn home_directory() -> Result<PathBuf, ServerError> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ServerError::Configuration("HOME must be set".to_owned()))
}

fn absolute(path: PathBuf) -> Result<PathBuf, ServerError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}
