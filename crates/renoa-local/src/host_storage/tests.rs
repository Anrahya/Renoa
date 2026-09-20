use std::{
    cell::Cell,
    io,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use renoa_kernel::{AgentId, KernelError, SessionId};
use tempfile::tempdir;

use super::{
    DirectoryPublication, KERNEL_DATABASE, SessionPublication, create_session_storage,
    create_session_storage_with_hook, delete_session_storage, delete_session_storage_with_hook,
    load_session_after_handoff, open_session_storage_with_hook, publish_session,
};
use crate::{
    LocalHostError, LocalSession, LocalSessionError, ModelProvider, ReasoningLevel,
    selection::RuntimeSelection,
};

#[test]
fn failed_initialization_never_publishes_a_partial_session() {
    let directory = tempdir().expect("temporary directory");
    let session_id = renoa_kernel::SessionId::new();

    let result = publish_session(directory.path(), session_id, |staging| {
        std::fs::write(staging.join("partial"), "not a session")?;
        Err(LocalHostError::Io(io::Error::other(
            "injected creation failure",
        )))
    });

    assert!(matches!(result, Err(LocalHostError::Io(_))));
    assert!(!directory.path().join(session_id.to_string()).exists());
    assert!(
        !directory
            .path()
            .join(format!(".creating-{session_id}"))
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn published_session_directory_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().expect("temporary directory");
    let session_id = renoa_kernel::SessionId::new();

    let DirectoryPublication::Created(published) =
        publish_session(directory.path(), session_id, |_| Ok(()))
            .expect("publish session directory")
    else {
        panic!("new session unexpectedly existed");
    };

    assert_eq!(
        std::fs::metadata(published)
            .expect("session metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn publication_recovers_a_stale_creation_and_reuses_the_published_session() {
    let sessions = tempdir().expect("temporary directory");
    let session_id = SessionId::new();
    let staging = sessions.path().join(format!(".creating-{session_id}"));
    std::fs::create_dir(&staging).expect("create stale staging directory");
    std::fs::write(staging.join("partial"), "incomplete").expect("write stale data");

    let DirectoryPublication::Created(published) =
        publish_session(sessions.path(), session_id, |directory| {
            assert!(!directory.join("partial").exists());
            std::fs::write(directory.join("complete"), "ready")?;
            Ok(())
        })
        .expect("recover session publication")
    else {
        panic!("stale creation unexpectedly resolved as published");
    };
    assert_eq!(
        std::fs::read_to_string(published.join("complete")).expect("read published data"),
        "ready"
    );

    let initialized = Cell::new(false);
    let publication = publish_session(sessions.path(), session_id, |_| {
        initialized.set(true);
        Ok(())
    })
    .expect("reuse published session");
    assert!(matches!(publication, DirectoryPublication::Existing));
    assert!(!initialized.get());
}

#[test]
fn deletion_requires_exclusive_ownership_and_is_idempotent() {
    let sessions = tempdir().expect("temporary directory");
    let workspace = tempdir().expect("workspace directory");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let SessionPublication::Created(opened) = create_session_storage(
        sessions.path(),
        agent_id,
        session_id,
        workspace.path().to_owned(),
        &RuntimeSelection {
            provider: ModelProvider::Xai,
            model: "test".to_owned(),
            reasoning: ReasoningLevel::High,
        },
    )
    .expect("create session storage") else {
        panic!("new session unexpectedly existed");
    };
    let directory = opened.directory.clone();
    let owner = opened.kernel;

    let active_delete = delete_session_storage(sessions.path(), agent_id, session_id);
    assert!(matches!(
        active_delete,
        Err(LocalHostError::Session(LocalSessionError::Kernel(
            KernelError::AlreadyRunning { .. }
        )))
    ));
    assert!(directory.is_dir());

    drop(owner);
    let foreign_delete = delete_session_storage(sessions.path(), AgentId::new(), session_id);
    assert!(matches!(
        foreign_delete,
        Err(LocalHostError::InvalidRequest(message))
            if message == "session belongs to a different agent"
    ));
    assert!(directory.is_dir());

    delete_session_storage(sessions.path(), agent_id, session_id).expect("delete session storage");
    assert!(!directory.exists());
    assert!(
        !sessions
            .path()
            .join(format!(".deleting-{session_id}"))
            .exists()
    );

    delete_session_storage(sessions.path(), agent_id, session_id)
        .expect("repeat session deletion idempotently");
    delete_session_storage(sessions.path(), AgentId::new(), SessionId::new())
        .expect("an absent session stays deletable for any agent");
}

#[test]
fn ownership_handoff_waits_briefly_for_a_released_local_owner() {
    let sessions = tempdir().expect("temporary directory");
    let workspace = tempdir().expect("workspace directory");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let SessionPublication::Created(opened) = create_session_storage(
        sessions.path(),
        agent_id,
        session_id,
        workspace.path().to_owned(),
        &RuntimeSelection {
            provider: ModelProvider::Xai,
            model: "test".to_owned(),
            reasoning: ReasoningLevel::High,
        },
    )
    .expect("create session storage") else {
        panic!("new session unexpectedly existed");
    };
    let kernel_path = opened.directory.join(KERNEL_DATABASE);
    let owner = opened.kernel;
    let (sender, receiver) = mpsc::sync_channel(1);
    let waiter = thread::spawn(move || {
        sender
            .send(load_session_after_handoff(&kernel_path, session_id))
            .expect("send handoff result");
    });

    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    ));
    drop(owner);
    let reopened = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("handoff completed")
        .expect("reopen after ownership release");
    assert_eq!(reopened.agent_id(), agent_id);
    drop(reopened);
    waiter.join().expect("handoff thread completed");
}

#[test]
fn loading_waits_for_a_brief_kernel_handoff_but_refuses_a_live_owner() {
    let sessions = tempdir().expect("temporary directory");
    let workspace = tempdir().expect("workspace directory");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let SessionPublication::Created(opened) = create_session_storage(
        sessions.path(),
        agent_id,
        session_id,
        workspace.path().to_owned(),
        &RuntimeSelection {
            provider: ModelProvider::Xai,
            model: "test".to_owned(),
            reasoning: ReasoningLevel::High,
        },
    )
    .expect("create session storage") else {
        panic!("new session unexpectedly existed");
    };
    let owner = opened.kernel;
    let sessions_for_load = sessions.path().to_owned();
    let workspace_for_load = workspace.path().to_owned();
    let (manifest_sender, manifest_receiver) = mpsc::sync_channel(1);
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    let loading = thread::spawn(move || {
        let result = open_session_storage_with_hook(
            &sessions_for_load,
            agent_id,
            session_id,
            &workspace_for_load,
            || manifest_sender.send(()).expect("announce manifest read"),
        );
        result_sender.send(result).expect("send load result");
    });

    manifest_receiver.recv().expect("load reached manifest");
    assert!(matches!(
        result_receiver.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    ));
    drop(owner);

    let reopened = result_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("load resumes after handoff")
        .expect("reopen after ownership release");
    assert_eq!(reopened.kernel.agent_id(), agent_id);

    let active_load = open_session_storage_with_hook(
        sessions.path(),
        agent_id,
        session_id,
        workspace.path(),
        || {},
    );
    assert!(matches!(
        active_load,
        Err(LocalHostError::Session(LocalSessionError::Kernel(
            KernelError::AlreadyRunning { .. }
        )))
    ));

    drop(reopened);
    loading.join().expect("load thread completed");
}

#[test]
fn deletion_retry_cleans_a_published_tombstone() {
    let sessions = tempdir().expect("temporary directory");
    let session_id = SessionId::new();
    let directory = sessions.path().join(session_id.to_string());
    let tombstone = sessions.path().join(format!(".deleting-{session_id}"));
    std::fs::create_dir(&directory).expect("create session directory");
    std::fs::write(directory.join("data"), "durable").expect("write session data");
    std::fs::rename(&directory, &tombstone).expect("publish deletion tombstone");

    delete_session_storage(sessions.path(), AgentId::new(), session_id)
        .expect("resume session deletion");

    assert!(!directory.exists());
    assert!(!tombstone.exists());
}

#[test]
fn creation_waits_for_a_brief_kernel_handoff_before_first_open() {
    let sessions = tempdir().expect("temporary directory");
    let workspace = tempdir().expect("workspace directory");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let selection = RuntimeSelection {
        provider: ModelProvider::Xai,
        model: "test".to_owned(),
        reasoning: ReasoningLevel::High,
    };
    let sessions_for_creation = sessions.path().to_owned();
    let kernel_path = sessions
        .path()
        .join(session_id.to_string())
        .join(KERNEL_DATABASE);
    let workspace_for_creation = workspace.path().to_owned();
    let (owner_sender, owner_receiver) = mpsc::sync_channel(1);
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    let creating = thread::spawn(move || {
        let result = create_session_storage_with_hook(
            &sessions_for_creation,
            agent_id,
            session_id,
            workspace_for_creation,
            &selection,
            || {
                let owner =
                    LocalSession::load(kernel_path, session_id).expect("hold published kernel");
                owner_sender.send(owner).expect("send temporary owner");
            },
        );
        result_sender.send(result).expect("send creation result");
    });

    let temporary_owner = owner_receiver.recv().expect("receive temporary owner");
    assert!(matches!(
        result_receiver.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    ));
    drop(temporary_owner);

    let SessionPublication::Created(opened) = result_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("creation resumes after handoff")
        .expect("create and open session")
    else {
        panic!("new session unexpectedly existed");
    };
    assert_eq!(opened.kernel.agent_id(), agent_id);
    drop(opened);
    creating.join().expect("creation thread");
}

#[test]
fn creation_and_first_open_are_one_session_lifecycle_transition() {
    let sessions = tempdir().expect("temporary directory");
    let workspace = tempdir().expect("workspace directory");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let selection = RuntimeSelection {
        provider: ModelProvider::Xai,
        model: "test".to_owned(),
        reasoning: ReasoningLevel::High,
    };

    let sessions_for_creation = sessions.path().to_owned();
    let workspace_for_creation = workspace.path().to_owned();
    let (published, publication_observed) = mpsc::sync_channel(1);
    let (resume_creation, resume) = mpsc::sync_channel(1);
    let creating = thread::spawn(move || {
        create_session_storage_with_hook(
            &sessions_for_creation,
            agent_id,
            session_id,
            workspace_for_creation,
            &selection,
            || {
                published.send(()).expect("announce publication");
                resume.recv().expect("resume first open");
            },
        )
    });
    publication_observed
        .recv()
        .expect("session directory was published");

    let sessions_for_delete = sessions.path().to_owned();
    let (deletion_done, deletion_observed) = mpsc::sync_channel(1);
    let deleting = thread::spawn(move || {
        delete_session_storage(&sessions_for_delete, agent_id, session_id)
            .expect("delete created session");
        deletion_done.send(()).expect("announce deletion");
    });
    let deletion_was_blocked = matches!(
        deletion_observed.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );
    resume_creation.send(()).expect("release first open");
    let SessionPublication::Created(opened) = creating
        .join()
        .expect("creation thread")
        .expect("create and open session")
    else {
        panic!("new session unexpectedly existed");
    };
    assert_eq!(opened.manifest.agent_id, agent_id);
    assert_eq!(opened.kernel.agent_id(), agent_id);
    drop(opened);
    deleting.join().expect("deletion thread");

    assert!(
        deletion_was_blocked,
        "deletion must wait until the published kernel is owned"
    );
    assert!(!sessions.path().join(session_id.to_string()).exists());
}

#[test]
fn deletion_and_recreation_are_one_session_lifecycle_transition() {
    let sessions = tempdir().expect("temporary directory");
    let first_workspace = tempdir().expect("first workspace");
    let second_workspace = tempdir().expect("second workspace");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let selection = RuntimeSelection {
        provider: ModelProvider::Xai,
        model: "test".to_owned(),
        reasoning: ReasoningLevel::High,
    };
    create_session_storage(
        sessions.path(),
        agent_id,
        session_id,
        first_workspace.path().to_owned(),
        &selection,
    )
    .expect("create first incarnation");

    let sessions_for_delete = sessions.path().to_owned();
    let (manifest_read, manifest_observed) = mpsc::sync_channel(1);
    let (resume_delete, resume) = mpsc::sync_channel(1);
    let deleting = thread::spawn(move || {
        delete_session_storage_with_hook(&sessions_for_delete, agent_id, session_id, || {
            manifest_read.send(()).expect("announce manifest read");
            resume.recv().expect("resume first deletion");
        })
    });
    manifest_observed
        .recv()
        .expect("first deletion reached manifest");

    let sessions_for_replacement = sessions.path().to_owned();
    let replacement_workspace = second_workspace.path().to_owned();
    let replacement_selection = selection.clone();
    let (replacement_done, replacement_observed) = mpsc::sync_channel(1);
    let replacement = thread::spawn(move || {
        delete_session_storage(&sessions_for_replacement, agent_id, session_id)
            .expect("delete first incarnation");
        create_session_storage(
            &sessions_for_replacement,
            agent_id,
            session_id,
            replacement_workspace,
            &replacement_selection,
        )
        .expect("publish replacement");
        replacement_done.send(()).expect("announce replacement");
    });
    let replacement_was_blocked = matches!(
        replacement_observed.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );
    resume_delete.send(()).expect("release first deletion");
    deleting
        .join()
        .expect("first deletion thread")
        .expect("first deletion");
    replacement.join().expect("replacement thread");

    assert!(
        replacement_was_blocked,
        "replacement must wait until the authorized deletion finishes"
    );
    let manifest = super::read_manifest_file(
        &sessions
            .path()
            .join(session_id.to_string())
            .join(super::MANIFEST_FILE),
    )
    .expect("replacement manifest survives");
    assert_eq!(manifest.workspace, second_workspace.path());
}

#[test]
fn loading_and_recreation_are_one_session_lifecycle_transition() {
    let sessions = tempdir().expect("temporary directory");
    let first_workspace = tempdir().expect("first workspace");
    let second_workspace = tempdir().expect("second workspace");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let selection = RuntimeSelection {
        provider: ModelProvider::Xai,
        model: "test".to_owned(),
        reasoning: ReasoningLevel::High,
    };
    create_session_storage(
        sessions.path(),
        agent_id,
        session_id,
        first_workspace.path().to_owned(),
        &selection,
    )
    .expect("create first incarnation");

    let sessions_for_load = sessions.path().to_owned();
    let first_workspace_for_load = first_workspace.path().to_owned();
    let (manifest_read, manifest_observed) = mpsc::sync_channel(1);
    let (resume_load, resume) = mpsc::sync_channel(1);
    let loading = thread::spawn(move || {
        open_session_storage_with_hook(
            &sessions_for_load,
            agent_id,
            session_id,
            &first_workspace_for_load,
            || {
                manifest_read.send(()).expect("announce manifest read");
                resume.recv().expect("resume first load");
            },
        )
    });
    manifest_observed.recv().expect("load reached manifest");

    let sessions_for_replacement = sessions.path().to_owned();
    let replacement_workspace = second_workspace.path().to_owned();
    let replacement_selection = selection.clone();
    let (replacement_done, replacement_observed) = mpsc::sync_channel(1);
    let replacement = thread::spawn(move || {
        delete_session_storage(&sessions_for_replacement, agent_id, session_id)
            .expect("delete first incarnation");
        create_session_storage(
            &sessions_for_replacement,
            agent_id,
            session_id,
            replacement_workspace,
            &replacement_selection,
        )
        .expect("publish replacement");
        replacement_done.send(()).expect("announce replacement");
    });
    let replacement_was_blocked = matches!(
        replacement_observed.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );
    resume_load.send(()).expect("release first load");
    let opened = loading
        .join()
        .expect("load thread")
        .expect("load first incarnation");
    assert_eq!(opened.manifest.workspace, first_workspace.path());
    assert_eq!(opened.kernel.agent_id(), agent_id);
    drop(opened);
    replacement.join().expect("replacement thread");

    assert!(
        replacement_was_blocked,
        "replacement must wait until manifest and kernel are opened together"
    );
    let manifest = super::read_manifest_file(
        &sessions
            .path()
            .join(session_id.to_string())
            .join(super::MANIFEST_FILE),
    )
    .expect("replacement manifest survives");
    assert_eq!(manifest.workspace, second_workspace.path());
}
