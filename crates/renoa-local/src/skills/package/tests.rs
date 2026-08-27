use std::fs;

use tempfile::tempdir;

use super::{
    MAX_SOURCE_BYTES, MAX_SOURCE_FILES, admit_source_budget, inspect_source, load_owned, publish,
};

#[test]
fn aggregate_source_bounds_fail_instead_of_multiplying_package_limits() {
    assert_eq!(
        admit_source_budget(
            std::path::Path::new("skills"),
            MAX_SOURCE_FILES - 1,
            MAX_SOURCE_BYTES - 1,
            1,
            1,
        )
        .expect("exact aggregate limits"),
        (MAX_SOURCE_FILES, MAX_SOURCE_BYTES)
    );
    assert!(
        admit_source_budget(
            std::path::Path::new("skills"),
            MAX_SOURCE_FILES,
            MAX_SOURCE_BYTES,
            1,
            0,
        )
        .is_err()
    );
}

#[test]
fn standard_skill_is_captured_published_and_verified() {
    let directory = tempdir().expect("temporary directory");
    let source = directory.path().join("source");
    let skill = source.join("code-review");
    fs::create_dir_all(skill.join("references")).expect("create skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: code-review\ndescription: Review code carefully.\nmetadata:\n  owner: renoa\n---\nCheck the diff.\n",
    )
    .expect("write SKILL.md");
    fs::write(skill.join("references/checks.md"), "# Checks\n").expect("write reference");
    let snapshot = inspect_source(&source).expect("inspect source");
    assert!(snapshot.rejections.is_empty());
    assert_eq!(snapshot.skills.len(), 1);
    let store = directory.path().join("owned");
    super::initialize_store(&store).expect("initialize store");
    publish(&store, &snapshot.skills[0]).expect("publish skill");

    let loaded = load_owned(&store, &snapshot.skills[0].digest).expect("load owned skill");

    assert_eq!(loaded.metadata.name, "code-review");
    assert_eq!(loaded.body, "Check the diff.\n");
    assert_eq!(loaded.files, ["SKILL.md", "references/checks.md"]);
}

#[test]
fn installed_content_changes_are_rejected() {
    let directory = tempdir().expect("temporary directory");
    let source = directory.path().join("source");
    let skill = source.join("stable");
    fs::create_dir_all(&skill).expect("create skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: stable\ndescription: Stable instructions.\n---\nOriginal.\n",
    )
    .expect("write SKILL.md");
    let snapshot = inspect_source(&source).expect("inspect source");
    let store = directory.path().join("owned");
    super::initialize_store(&store).expect("initialize store");
    let installed = publish(&store, &snapshot.skills[0]).expect("publish skill");
    let installed_file = installed.join("SKILL.md");
    let mut permissions = fs::metadata(&installed_file)
        .expect("installed file metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(&installed_file, permissions).expect("make installed file writable");
    fs::write(
        &installed_file,
        "---\nname: stable\ndescription: Stable instructions.\n---\nChanged.\n",
    )
    .expect("tamper with installed skill");

    assert!(matches!(
        load_owned(&store, &snapshot.skills[0].digest),
        Err(crate::skills::SkillError::Conflict(_))
    ));
}

#[cfg(unix)]
#[test]
fn republishing_repairs_owner_only_read_only_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().expect("temporary directory");
    let source = directory.path().join("source");
    let skill = source.join("repair");
    fs::create_dir_all(&skill).expect("create skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: repair\ndescription: Repair permissions.\n---\nRepair.\n",
    )
    .expect("write skill");
    let snapshot = inspect_source(&source).expect("inspect source");
    let store = directory.path().join("owned");
    super::initialize_store(&store).expect("initialize store");
    let installed = publish(&store, &snapshot.skills[0]).expect("publish skill");
    let installed_file = installed.join("SKILL.md");
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o700))
        .expect("make installed directory writable");
    fs::set_permissions(&installed_file, fs::Permissions::from_mode(0o600))
        .expect("make installed file writable");

    publish(&store, &snapshot.skills[0]).expect("repair existing package");

    assert_eq!(
        fs::metadata(&installed)
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o500
    );
    assert_eq!(
        fs::metadata(&installed_file)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
}

#[test]
fn invalid_entries_are_isolated_and_symlinks_fail_closed() {
    let directory = tempdir().expect("temporary directory");
    let source = directory.path().join("source");
    fs::create_dir_all(source.join("valid")).expect("create valid skill");
    fs::write(
        source.join("valid/SKILL.md"),
        "---\nname: valid\ndescription: A valid skill.\n---\nUse it.\n",
    )
    .expect("write valid skill");
    fs::create_dir_all(source.join("wrong")).expect("create invalid skill");
    fs::write(
        source.join("wrong/SKILL.md"),
        "---\nname: another\ndescription: Wrong directory.\n---\n",
    )
    .expect("write invalid skill");
    fs::create_dir_all(source.join("not-a-skill/cache")).expect("create unrelated directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("../valid/SKILL.md", source.join("wrong/link.md")).expect("create symlink");
    }

    let snapshot = inspect_source(&source).expect("inspect source");

    assert_eq!(snapshot.skills.len(), 1);
    assert_eq!(snapshot.rejections.len(), 1);
    assert_eq!(snapshot.rejections[0].entry_name, "wrong");
}

#[test]
fn vendor_frontmatter_is_rejected_instead_of_silently_ignored() {
    let directory = tempdir().expect("temporary directory");
    let source = directory.path().join("source");
    fs::create_dir_all(source.join("deploy")).expect("create skill");
    fs::write(
        source.join("deploy/SKILL.md"),
        "---\nname: deploy\ndescription: Deploy the service.\ndisable-model-invocation: true\n---\nDeploy.\n",
    )
    .expect("write skill");

    let snapshot = inspect_source(&source).expect("inspect source");

    assert!(snapshot.skills.is_empty());
    assert_eq!(snapshot.rejections.len(), 1);
    assert!(snapshot.rejections[0].reason.contains("unknown field"));
}

#[test]
fn duplicate_frontmatter_fields_are_rejected_as_ambiguous() {
    let directory = tempdir().expect("temporary directory");
    let source = directory.path().join("source");
    fs::create_dir_all(source.join("duplicate")).expect("create skill");
    fs::write(
        source.join("duplicate/SKILL.md"),
        "---\nname: duplicate\nname: replaced\ndescription: Ambiguous skill.\n---\nNever load.\n",
    )
    .expect("write skill");

    let snapshot = inspect_source(&source).expect("inspect source");

    assert!(snapshot.skills.is_empty());
    assert_eq!(snapshot.rejections.len(), 1);
}

#[test]
fn owned_loading_rejects_a_pathlike_digest_before_joining_it_to_the_store() {
    let directory = tempdir().expect("temporary directory");

    let error = load_owned(
        directory.path(),
        "../../outside...................................................",
    )
    .expect_err("pathlike digest must fail before filesystem resolution");

    assert!(error.to_string().contains("invalid content digest"));
}
