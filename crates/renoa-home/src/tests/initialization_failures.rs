use std::cell::RefCell;

use super::*;

struct FailureProbe {
    fail_at: PathBuf,
    occupied: Option<PathBuf>,
    removals: Vec<PathBuf>,
}

thread_local! {
    static PROBE: RefCell<Option<FailureProbe>> = const { RefCell::new(None) };
}

struct FaultGuard;

impl FaultGuard {
    fn new(fail_at: PathBuf, occupied: Option<PathBuf>) -> Self {
        PROBE.with(|probe| {
            assert!(probe.borrow().is_none());
            *probe.borrow_mut() = Some(FailureProbe {
                fail_at,
                occupied,
                removals: Vec::new(),
            });
        });
        Self
    }

    fn removals() -> Vec<PathBuf> {
        PROBE.with(|probe| probe.borrow().as_ref().unwrap().removals.clone())
    }
}

impl Drop for FaultGuard {
    fn drop(&mut self) {
        PROBE.with(|probe| *probe.borrow_mut() = None);
    }
}

pub(crate) fn before_create(path: &Path) -> io::Result<()> {
    PROBE.with(|probe| {
        let mut probe = probe.borrow_mut();
        if let Some(probe) = probe.as_mut()
            && path == probe.fail_at
        {
            if let Some(occupied) = &probe.occupied {
                fs::write(occupied, "another writer's file")?;
            }
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected creation failure",
            ));
        }
        Ok(())
    })
}

pub(crate) fn before_remove(path: &Path) {
    PROBE.with(|probe| {
        if let Some(probe) = probe.borrow_mut().as_mut() {
            probe.removals.push(path.to_owned());
        }
    });
}

#[test]
fn partial_layout_failure_preserves_the_creation_error_and_attempts_all_cleanup() {
    for blocked in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let home = RenoaHome::at(directory.path().join("home")).unwrap();
        let occupied = home.path().join("sessions/other-writer");
        let fault = FaultGuard::new(home.path().join("state"), blocked.then(|| occupied.clone()));
        let error = home.initialize().expect_err("fail after five children");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("injected creation failure"));
        assert_eq!(
            FaultGuard::removals(),
            ["sessions", "agents", "plugins", "credentials", "config"]
                .map(|name| home.path().join(name))
                .into_iter()
                .chain([home.path().to_owned()])
                .collect::<Vec<_>>()
        );
        for name in ["agents", "plugins", "credentials", "config"] {
            assert!(!home.path().join(name).exists(), "clean up {name}");
        }
        if blocked {
            assert_eq!(
                fs::read_to_string(&occupied).unwrap(),
                "another writer's file"
            );
            assert!(error.to_string().contains("sessions"));
            let cause = error.get_ref().unwrap().source().unwrap();
            assert_eq!(cause.to_string(), "injected creation failure");
            fs::remove_file(occupied).unwrap();
        } else {
            assert!(!home.path().exists());
        }
        drop(fault);
        home.initialize()
            .expect("retry adopts remaining directories");
        for name in DIRECTORIES {
            assert!(home.path().join(name).is_dir());
        }
    }
}

#[test]
fn partial_workspace_failure_preserves_the_creation_error_and_attempts_all_cleanup() {
    for blocked in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let home = RenoaHome::at(directory.path().join("home")).unwrap();
        let agent = "00000000-0000-0000-0000-000000000001";
        let workspace = home.agent_workspace(agent).unwrap();
        let agent_root = workspace.parent().unwrap();
        let occupied = agent_root.join("other-writer");
        let fault = FaultGuard::new(workspace.clone(), blocked.then(|| occupied.clone()));
        let error = home
            .initialize_agent_workspace(agent)
            .expect_err("fail after creating the workspace ancestors");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("injected creation failure"));
        assert_eq!(
            FaultGuard::removals(),
            [
                agent_root.to_owned(),
                home.path().join("agents"),
                home.path().to_owned()
            ]
        );
        assert!(!workspace.exists());
        if blocked {
            assert_eq!(
                fs::read_to_string(&occupied).unwrap(),
                "another writer's file"
            );
            assert!(error.to_string().contains(agent));
            fs::remove_file(occupied).unwrap();
        } else {
            assert!(!home.path().exists());
        }
        drop(fault);
        assert_eq!(home.initialize_agent_workspace(agent).unwrap(), workspace);
        assert!(workspace.is_dir());
    }
}
