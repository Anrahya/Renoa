//! One installation root owns Host data, credentials, plugins, and runtime files.

use std::{
    env, fs, io,
    path::{Component, Path, PathBuf},
};

pub const HOST_DATABASE_PATH: &str = "state/host.sqlite3";
const DIRECTORIES: [&str; 7] = [
    "config",
    "credentials",
    "plugins",
    "agents",
    "sessions",
    "state",
    "runtime",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenoaHome {
    root: PathBuf,
}

impl RenoaHome {
    /// Explicit service configuration takes precedence, then `RENOA_HOME`, then `~/.renoa`.
    /// # Errors
    /// Rejects missing home settings, relative paths, traversal, and symlinks in managed roots.
    pub fn resolve(explicit: Option<PathBuf>) -> io::Result<Self> {
        let root = if let Some(root) = explicit.filter(|root| !root.as_os_str().is_empty()) {
            root
        } else if let Some(root) = env::var_os("RENOA_HOME") {
            PathBuf::from(root)
        } else {
            #[cfg(windows)]
            let base = env::var_os("USERPROFILE");
            #[cfg(not(windows))]
            let base = env::var_os("HOME");
            PathBuf::from(
                base.filter(|base| !base.is_empty())
                    .ok_or_else(|| invalid("HOME must be set or provide RENOA_HOME"))?,
            )
            .join(".renoa")
        };
        Self::at(root)
    }
    /// Validates an installation location without writing it.
    /// # Errors
    /// Rejects relative paths, traversal, and non-directory or linked managed roots.
    pub fn at(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        if !root.is_absolute()
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(invalid(
                "Renoa home must be an absolute path without traversal",
            ));
        }
        let home = Self { root };
        home.preflight()?;
        Ok(home)
    }
    /// Creates the fixed layout, cleaning up newly created empty directories on failure.
    /// # Errors
    /// Returns invalid roots or filesystem failures.
    pub fn initialize(&self) -> io::Result<()> {
        self.preflight()?;
        let mut created = Vec::new();
        let result = (|| {
            create(&self.root, &mut created)?;
            for directory in DIRECTORIES {
                create(&self.root.join(directory), &mut created)?;
            }
            Ok(())
        })();
        if result.is_err() {
            for path in created.into_iter().rev() {
                fs::remove_dir(path)?;
            }
        }
        result
    }
    fn preflight(&self) -> io::Result<()> {
        inspect_ancestors(&self.root)?;
        for directory in DIRECTORIES {
            inspect_directory(&self.root.join(directory))?;
        }
        Ok(())
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }
    #[must_use]
    pub fn host_database(&self) -> PathBuf {
        self.root.join(HOST_DATABASE_PATH)
    }
    /// # Errors
    /// Rejects a non-UUID agent identity before constructing its filesystem path.
    pub fn agent_workspace(&self, agent: &str) -> io::Result<PathBuf> {
        let agent =
            uuid::Uuid::parse_str(agent).map_err(|_| invalid("agent identity must be a UUID"))?;
        let path = self
            .root
            .join("agents")
            .join(agent.to_string())
            .join("workspace");
        inspect_ancestors(&path)?;
        Ok(path)
    }
    /// Opens an agent workspace without adopting a linked agent directory.
    /// # Errors
    /// Rejects invalid identities, linked directories, and filesystem failures.
    pub fn initialize_agent_workspace(&self, agent: &str) -> io::Result<PathBuf> {
        let path = self.agent_workspace(agent)?;
        let mut created = Vec::new();
        if let Err(error) = create(&path, &mut created) {
            for directory in created.into_iter().rev() {
                fs::remove_dir(directory)?;
            }
            return Err(error);
        }
        Ok(path)
    }
    #[must_use]
    pub fn model_credentials(&self) -> PathBuf {
        self.root.join("credentials/models.sqlite3")
    }
    #[must_use]
    pub fn node_database(&self) -> PathBuf {
        self.root.join("state/node.sqlite3")
    }
    #[must_use]
    pub fn coordinator_database(&self) -> PathBuf {
        self.root.join("state/coordinator.sqlite3")
    }
    /// # Errors
    /// Rejects unrecognized surface names.
    pub fn surface_directory(&self, surface: &str) -> io::Result<PathBuf> {
        if !matches!(surface, "telegram" | "slack" | "discord") {
            return Err(invalid("unknown Renoa surface"));
        }
        Ok(self.root.join("state/surfaces").join(surface))
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn inspect_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(invalid(&format!(
            "managed Renoa directory must be a plain directory, never a symbolic link or another file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
fn inspect_ancestors(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        inspect_directory(&current)?;
    }
    Ok(())
}
fn create(path: &Path, created: &mut Vec<PathBuf>) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(invalid("managed Renoa directory changed during creation")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                create(parent, created)?;
            }
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder.create(path)?;
            created.push(path.to_owned());
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
