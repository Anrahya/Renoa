use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::github::MAX_DEPTH;
use crate::{plugins::PluginError, skills::registry::validate_name};

const MAX_LISTED_SKILLS: usize = 40;
const MAX_ERROR_BYTES: usize = 4_096;
const MAX_SKILL_MD_BYTES: u64 = 32 * 1_024 * 1_024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct DiscoveredSkill {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<String>,
}

struct SkillCandidate {
    entry: DiscoveredSkill,
    dir: PathBuf,
}

pub(super) enum SelectedPackage {
    Plugin { root: PathBuf, path: Option<String> },
    Skill { root: PathBuf, path: Option<String> },
}

#[derive(Default)]
struct WalkState {
    candidates: Vec<SkillCandidate>,
    matches: Vec<SkillCandidate>,
    truncated: bool,
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
    let mut state = WalkState::default();
    if has_skill {
        let Some(wanted) = skill else {
            return Ok(SelectedPackage::Skill {
                root: root.to_path_buf(),
                path: base_path.map(str::to_owned),
            });
        };
        let fallback = root_fallback_name(repository, base_path);
        if let Some(actual) = valid_skill_name(root, &fallback)? {
            if actual == wanted {
                return Ok(SelectedPackage::Skill {
                    root: root.to_path_buf(),
                    path: base_path.map(str::to_owned),
                });
            }
            state.candidates.push(SkillCandidate {
                entry: DiscoveredSkill {
                    name: actual,
                    path: base_path.map(str::to_owned),
                },
                dir: root.to_path_buf(),
            });
        }
    }

    let base_depth = base_path.map_or(0, |path| Path::new(path).components().count());
    walk_skills(root, root, base_path, base_depth, 0, skill, &mut state)?;
    state.candidates.sort_by(|left, right| {
        (&left.entry.path, &left.entry.name).cmp(&(&right.entry.path, &right.entry.name))
    });
    state.matches.sort_by(|left, right| {
        (&left.entry.path, &left.entry.name).cmp(&(&right.entry.path, &right.entry.name))
    });
    resolve_selection(state, repository, commit, base_path, skill, has_plugin)
}

fn resolve_selection(
    state: WalkState,
    repository: &str,
    commit: &str,
    base_path: Option<&str>,
    skill: Option<&str>,
    has_plugin: bool,
) -> Result<SelectedPackage, PluginError> {
    if let Some(wanted) = skill {
        let mut matches = state.matches;
        return match matches.len() {
            1 => {
                let chosen = matches.pop().expect("single match");
                Ok(SelectedPackage::Skill {
                    root: chosen.dir,
                    path: chosen.entry.path,
                })
            }
            0 => {
                if state.candidates.is_empty() {
                    return Err(empty_selection_error(
                        repository,
                        commit,
                        base_path,
                        Some(wanted),
                        has_plugin,
                    ));
                }
                let entries = state
                    .candidates
                    .into_iter()
                    .map(|c| c.entry)
                    .collect::<Vec<_>>();
                Err(selection_error(
                    &format!(
                        "skill `{wanted}` was not found in {}",
                        location(repository, commit, base_path)
                    ),
                    repository,
                    commit,
                    &entries,
                    state.truncated,
                ))
            }
            _ => {
                let entries = matches.into_iter().map(|c| c.entry).collect::<Vec<_>>();
                Err(selection_error(
                    &format!(
                        "skill `{wanted}` matches multiple folders in {}; pass path to select one",
                        location(repository, commit, base_path)
                    ),
                    repository,
                    commit,
                    &entries,
                    false,
                ))
            }
        };
    }

    if state.candidates.is_empty() {
        return Err(empty_selection_error(
            repository, commit, base_path, None, has_plugin,
        ));
    }
    let entries = state
        .candidates
        .into_iter()
        .map(|c| c.entry)
        .collect::<Vec<_>>();
    Err(selection_error(
        &format!(
            "{} holds neither plugin.json nor SKILL.md; choose one of the discovered skills",
            location(repository, commit, base_path)
        ),
        repository,
        commit,
        &entries,
        state.truncated,
    ))
}

fn walk_skills(
    base_root: &Path,
    dir: &Path,
    base_path: Option<&str>,
    base_depth: usize,
    depth: usize,
    wanted: Option<&str>,
    state: &mut WalkState,
) -> Result<(), PluginError> {
    if wanted.is_some() && state.matches.len() >= 2 {
        return Ok(());
    }
    if base_depth + depth >= MAX_DEPTH {
        return Ok(());
    }
    if dir != base_root && is_real_file(&dir.join("SKILL.md"))? {
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
            .unwrap_or_default();
        if let Some(name) = valid_skill_name(dir, folder)? {
            let entry = DiscoveredSkill {
                name,
                path: Some(repo_path),
            };
            if wanted.is_some_and(|target| target == entry.name) {
                state.matches.push(SkillCandidate {
                    entry: entry.clone(),
                    dir: dir.to_path_buf(),
                });
            }
            if state.candidates.len() < MAX_LISTED_SKILLS {
                state.candidates.push(SkillCandidate {
                    entry,
                    dir: dir.to_path_buf(),
                });
            } else {
                state.truncated = true;
            }
        }
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
        if entry.file_name() == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| PluginError::Io {
            action: "inspect GitHub directory entry",
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() {
            walk_skills(
                base_root,
                &entry.path(),
                base_path,
                base_depth,
                depth + 1,
                wanted,
                state,
            )?;
            if wanted.is_some() && state.matches.len() >= 2 {
                break;
            }
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

fn valid_skill_name(dir: &Path, fallback: &str) -> Result<Option<String>, PluginError> {
    let skill_md = dir.join("SKILL.md");
    let meta = fs::symlink_metadata(&skill_md).map_err(|source| PluginError::Io {
        action: "inspect SKILL.md metadata",
        path: skill_md.clone(),
        source,
    })?;
    if meta.len() > MAX_SKILL_MD_BYTES {
        return Ok(validate_name(fallback).ok().map(|()| fallback.to_owned()));
    }
    let bytes = fs::read(&skill_md).map_err(|source| PluginError::Io {
        action: "read SKILL.md",
        path: skill_md,
        source,
    })?;
    if let Some(name) = crate::skills::package::frontmatter_name(&bytes)
        && validate_name(&name).is_ok()
    {
        return Ok(Some(name));
    }
    Ok(validate_name(fallback).ok().map(|()| fallback.to_owned()))
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

fn empty_selection_error(
    repository: &str,
    commit: &str,
    base_path: Option<&str>,
    wanted: Option<&str>,
    has_plugin: bool,
) -> PluginError {
    let hint = if has_plugin {
        let path_part = match base_path {
            Some(path) => format!(",\"path\":{}", serde_json::json!(path)),
            None => String::new(),
        };
        format!(
            "{} contains plugin.json and no skill `{}`; retry inspect without skill: {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{},\"commit\":{}{}}}}}",
            location(repository, commit, base_path),
            wanted.unwrap_or_default(),
            serde_json::json!(repository),
            serde_json::json!(commit),
            path_part,
        )
    } else {
        format!(
            "no plugin.json or SKILL.md was found in {}; verify the repository or path and retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{},\"commit\":{}}}}}",
            location(repository, commit, base_path),
            serde_json::json!(repository),
            serde_json::json!(commit),
        )
    };
    PluginError::Invalid(hint)
}

fn selection_error(
    summary: &str,
    repository: &str,
    commit: &str,
    skills: &[DiscoveredSkill],
    truncated: bool,
) -> PluginError {
    let next_hint = if let Some(first_with_path) = skills.iter().find(|item| item.path.is_some()) {
        format!(
            "; retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{},\"commit\":{},\"path\":{}}}}} (or pass \"skill\":\"<name>\")",
            serde_json::json!(repository),
            serde_json::json!(commit),
            serde_json::json!(first_with_path.path.as_deref().unwrap_or_default()),
        )
    } else {
        let skill = skills.first().map_or("<name>", |item| item.name.as_str());
        format!(
            "; retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{},\"commit\":{},\"skill\":{}}}}}",
            serde_json::json!(repository),
            serde_json::json!(commit),
            serde_json::json!(skill),
        )
    };
    let budget = MAX_ERROR_BYTES.saturating_sub(summary.len() + next_hint.len() + 64);
    let (list_json, omitted) = bounded_list_json(skills, budget);
    let truncation_note = if truncated || omitted {
        " (list truncated; pass `path` to narrow)"
    } else {
        ""
    };
    PluginError::Invalid(format!(
        "{summary}: {list_json}{truncation_note}{next_hint}"
    ))
}

fn bounded_list_json(skills: &[DiscoveredSkill], budget: usize) -> (String, bool) {
    let mut list = String::from("[");
    let mut omitted = false;
    for (index, entry) in skills.iter().take(MAX_LISTED_SKILLS).enumerate() {
        let encoded = serde_json::to_string(entry).unwrap_or_default();
        let sep = if index == 0 { "" } else { "," };
        if list.len() + sep.len() + encoded.len() + 1 > budget {
            omitted = true;
            break;
        }
        list.push_str(sep);
        list.push_str(&encoded);
    }
    if skills.len() > MAX_LISTED_SKILLS {
        omitted = true;
    }
    list.push(']');
    (list, omitted)
}
