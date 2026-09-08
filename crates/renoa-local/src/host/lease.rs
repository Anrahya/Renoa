use std::{
    fs::{File, OpenOptions},
    path::Path,
};

/// Explicit unlock releases ownership even when a concurrent fork temporarily
/// retains this open file description until exec closes its inherited descriptor.
pub(super) struct ExecutionLease(File);

impl ExecutionLease {
    pub(super) fn acquire(path: &Path) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()?;
        Ok(Self(file))
    }
}

impl Drop for ExecutionLease {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            eprintln!("Host execution lease could not be explicitly released: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_does_not_wait_for_an_inherited_descriptor_to_close() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("worker.lock");
        let lease = ExecutionLease::acquire(&path).expect("owner");
        let inherited = lease
            .0
            .try_clone()
            .expect("same open file description as fork");
        assert!(ExecutionLease::acquire(&path).is_err());
        drop(lease);
        let next = ExecutionLease::acquire(&path).expect("new owner before inherited fd closes");
        drop(inherited);
        assert!(ExecutionLease::acquire(&path).is_err());
        drop(next);
        assert!(ExecutionLease::acquire(&path).is_ok());
    }
}
