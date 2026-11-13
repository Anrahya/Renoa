use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::plugins::PluginError;

const MAX_DISCOVERY_DEPTH: usize = 16;
const MAX_DISCOVERED_SKILLS: usize = 512;
const MAX_SKILL_MD_BYTES: u64 = 32 * 1_024 * 1_024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct DiscoveredSkill {
    pub(super) name: String,
    pub(super) path: String,
}

struct SkillCandidate {
    entry: DiscoveredSkill,
    dir: PathBuf,
}

pub(super) enum SelectedPackage {
    Plugin { root: PathBuf, path: Option<String> },
    Skill { root: PathBuf, path: Option<String> },
}

pub(super) fn validate_skill_selector(skill: &str) -> Result<(), PluginError> {
    if skill.is_empty()
        || skill == "."
        || skill == ".."
        || skill.contains(['/', '\\'])
        || skill.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(PluginError::Invalid(
            "skill selector must be a non-empty skill name without path separators or control characters; omit skill in inspect to list available skills".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn select(
    root: &Path,
    repository: &str,
    commit: &str,
    base_path: Option<&str>,
    skill: Option<&str>,
) -> Result<SelectedPackage, PluginError> {
    if let Some(wanted) = skill {
        validate_skill_selector(wanted)?;
    }
    let has_plugin = is_real_file(&root.join("plugin.json"))?;
    let has_skill = is_real_file(&root.join("SKILL.md"))?;
    if has_plugin && skill.is_none() {
        return Ok(SelectedPackage::Plugin {
            root: root.to_path_buf(),
            path: base_path.map(str::to_owned),
        });
    }
    if has_skill {
        if let Some(wanted) = skill {
            let actual = skill_name(root, &root_fallback_name(repository, base_path))?;
            if actual != wanted {
                let candidate = [DiscoveredSkill {
                    name: actual,
                    path: base_path.unwrap_or_default().to_owned(),
                }];
                return Err(selection_error(
                    &format!(
                        "skill `{wanted}` was not found in {}",
                        location(repository, commit, base_path)
                    ),
                    repository,
                    commit,
                    &candidate,
                ));
            }
        }
        return Ok(SelectedPackage::Skill {
            root: root.to_path_buf(),
            path: base_path.map(str::to_owned),
        });
    }

    let mut candidates = Vec::new();
    walk_skills(root, root, base_path, 0, &mut candidates)?;
    candidates.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));
    match_discovered_skill(candidates, repository, commit, base_path, skill, has_plugin)
}

fn match_discovered_skill(
    candidates: Vec<SkillCandidate>,
    repository: &str,
    commit: &str,
    base_path: Option<&str>,
    skill: Option<&str>,
    has_plugin: bool,
) -> Result<SelectedPackage, PluginError> {
    if candidates.is_empty() {
        let hint = if has_plugin {
            format!(
                "{} contains plugin.json and no skill `{}`; retry inspect without skill: {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":\"{repository}\",\"commit\":\"{commit}\"{}}}}}",
                location(repository, commit, base_path),
                skill.unwrap_or_default(),
                path_field(base_path),
            )
        } else {
            format!(
                "no plugin.json or SKILL.md was found in {}; verify the repository or path and retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":\"{repository}\",\"commit\":\"{commit}\"}}}}",
                location(repository, commit, base_path),
            )
        };
        return Err(PluginError::Invalid(hint));
    }

    match skill {
        None => {
            let entries = candidates
                .iter()
                .map(|c| c.entry.clone())
                .collect::<Vec<_>>();
            Err(selection_error(
                &format!(
                    "{} holds neither plugin.json nor SKILL.md; choose one of the discovered skills",
                    location(repository, commit, base_path)
                ),
                repository,
                commit,
                &entries,
            ))
        }
        Some(wanted) => {
            let mut matching = candidates
                .into_iter()
                .filter(|candidate| candidate.entry.name == wanted)
                .collect::<Vec<_>>();
            match matching.len() {
                1 => {
                    let chosen = matching.pop().expect("single match");
                    Ok(SelectedPackage::Skill {
                        root: chosen.dir,
                        path: Some(chosen.entry.path),
                    })
                }
                0 => Err(PluginError::Invalid(format!(
                    "skill `{wanted}` was not found in {}; inspect returned no matching skill. Pick one by skill or path: retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":\"{repository}\",\"commit\":\"{commit}\",\"skill\":\"<name>\"}}}}",
                    location(repository, commit, base_path)
                ))),
                _ => {
                    let entries = matching.iter().map(|c| c.entry.clone()).collect::<Vec<_>>();
                    Err(selection_error(
                        &format!(
                            "skill `{wanted}` matches multiple folders in {}; pass path to select one",
                            location(repository, commit, base_path)
                        ),
                        repository,
                        commit,
                        &entries,
                    ))
                }
            }
        }
    }
}

fn walk_skills(
    base_root: &Path,
    dir: &Path,
    base_path: Option<&str>,
    depth: usize,
    out: &mut Vec<SkillCandidate>,
) -> Result<(), PluginError> {
    if depth > MAX_DISCOVERY_DEPTH {
        return Ok(());
    }
    if dir != base_root && is_real_file(&dir.join("SKILL.md"))? {
        if out.len() >= MAX_DISCOVERED_SKILLS {
            return Err(PluginError::Invalid(format!(
                "repository exceeds {MAX_DISCOVERED_SKILLS} skills; specify path in inspect"
            )));
        }
        let rel = dir
            .strip_prefix(base_root)
            .expect("walk stays inside base_root")
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect::<Vec<_>>()
            .join("/");
        let repo_path = match base_path {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}/{rel}"),
            _ => rel,
        };
        let folder = dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned();
        let name = skill_name(dir, &folder)?;
        out.push(SkillCandidate {
            entry: DiscoveredSkill {
                name,
                path: repo_path,
            },
            dir: dir.to_path_buf(),
        });
        return Ok(());
    }

    let mut children = fs::read_dir(dir)
        .map_err(|source| PluginError::Io {
            action: "list GitHub skill candidates",
            path: dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| PluginError::Io {
            action: "read GitHub skill entry",
            path: dir.to_path_buf(),
            source,
        })?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for entry in children {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| PluginError::Io {
            action: "inspect GitHub directory entry",
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() {
            walk_skills(base_root, &entry.path(), base_path, depth + 1, out)?;
        }
    }
    Ok(())
}

fn is_real_file(path: &Path) -> Result<bool, PluginError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(!meta.file_type().is_symlink() && meta.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PluginError::Io {
            action: "inspect GitHub source file",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn skill_name(dir: &Path, fallback: &str) -> Result<String, PluginError> {
    let skill_md = dir.join("SKILL.md");
    let meta = fs::symlink_metadata(&skill_md).map_err(|source| PluginError::Io {
        action: "inspect SKILL.md metadata",
        path: skill_md.clone(),
        source,
    })?;
    if meta.len() > MAX_SKILL_MD_BYTES {
        return Ok(fallback.to_owned());
    }
    let bytes = fs::read(&skill_md).map_err(|source| PluginError::Io {
        action: "read SKILL.md",
        path: skill_md,
        source,
    })?;
    Ok(crate::skills::package::frontmatter_name(&bytes).unwrap_or_else(|| fallback.to_owned()))
}

fn root_fallback_name(repository: &str, base_path: Option<&str>) -> String {
    base_path
        .and_then(|path| Path::new(path).file_name().and_then(|value| value.to_str()))
        .or_else(|| repository.rsplit('/').next())
        .unwrap_or_default()
        .to_owned()
}

fn location(repository: &str, commit: &str, base_path: Option<&str>) -> String {
    match base_path {
        Some(path) => format!("{repository} at commit {commit} path `{path}`"),
        None => format!("{repository} at commit {commit}"),
    }
}

fn path_field(base_path: Option<&str>) -> String {
    match base_path {
        Some(path) => format!(",\"path\":{}", serde_json::json!(path)),
        None => String::new(),
    }
}

fn selection_error(
    summary: &str,
    repository: &str,
    commit: &str,
    skills: &[DiscoveredSkill],
) -> PluginError {
    let list = serde_json::to_string(skills).unwrap_or_else(|_| "[]".to_owned());
    let first_path = skills.first().map_or("<path>", |item| item.path.as_str());
    PluginError::Invalid(format!(
        "{summary}: {list}; retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{},\"commit\":{},\"path\":{}}}}} (or pass \"skill\":\"<name>\")",
        serde_json::json!(repository),
        serde_json::json!(commit),
        serde_json::json!(first_path),
    ))
}
