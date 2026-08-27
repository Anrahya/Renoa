use std::collections::BTreeMap;

use serde::Deserialize;

use super::{CapturedFile, SkillMetadata};
use crate::skills::{SkillError, registry::validate_name};

const MAX_LICENSE_CHARS: usize = 1_024;
const MAX_METADATA_ENTRIES: usize = 64;
const MAX_METADATA_KEY_CHARS: usize = 256;
const MAX_METADATA_VALUE_CHARS: usize = 1_024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

pub(super) fn parse(
    files: &[CapturedFile],
    expected_name: Option<&str>,
) -> Result<(SkillMetadata, String), SkillError> {
    let skill_md = files
        .iter()
        .find(|file| file.relative == "SKILL.md")
        .ok_or_else(|| SkillError::Invalid("skill has no root SKILL.md".to_owned()))?;
    let content = std::str::from_utf8(&skill_md.bytes)
        .map_err(|error| SkillError::Invalid(format!("SKILL.md is not UTF-8: {error}")))?;
    let (frontmatter, body) = split_frontmatter(content)?;
    let parsed = serde_saphyr::from_str::<Frontmatter>(frontmatter).map_err(|error| {
        SkillError::Invalid(format!("SKILL.md frontmatter is invalid: {error}"))
    })?;
    validate(&parsed, expected_name)?;
    Ok((
        SkillMetadata {
            name: parsed.name,
            description: parsed.description,
            license: parsed.license,
            compatibility: parsed.compatibility,
        },
        body.to_owned(),
    ))
}

fn validate(frontmatter: &Frontmatter, expected_name: Option<&str>) -> Result<(), SkillError> {
    validate_name(&frontmatter.name)?;
    if expected_name.is_some_and(|expected| expected != frontmatter.name) {
        return Err(SkillError::Invalid(format!(
            "skill name `{}` does not match directory `{}`",
            frontmatter.name,
            expected_name.unwrap_or_default()
        )));
    }
    let description_chars = frontmatter.description.chars().count();
    if frontmatter.description.trim().is_empty() || description_chars > 1_024 {
        return Err(SkillError::Invalid(
            "skill description must contain 1-1024 characters".to_owned(),
        ));
    }
    if frontmatter
        .compatibility
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 500)
    {
        return Err(SkillError::Invalid(
            "skill compatibility must contain 1-500 characters".to_owned(),
        ));
    }
    if frontmatter
        .license
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > MAX_LICENSE_CHARS)
    {
        return Err(SkillError::Invalid(format!(
            "skill license must contain 1-{MAX_LICENSE_CHARS} characters"
        )));
    }
    if frontmatter.allowed_tools.is_some() {
        return Err(SkillError::Invalid(
            "skill allowed-tools is unsupported because Renoa skills cannot grant tool permission"
                .to_owned(),
        ));
    }
    if frontmatter.metadata.len() > MAX_METADATA_ENTRIES
        || frontmatter.metadata.iter().any(|(key, value)| {
            key.is_empty()
                || key.chars().count() > MAX_METADATA_KEY_CHARS
                || value.chars().count() > MAX_METADATA_VALUE_CHARS
        })
    {
        return Err(SkillError::Invalid(
            "skill metadata exceeds the Host boundary".to_owned(),
        ));
    }
    Ok(())
}

fn split_frontmatter(content: &str) -> Result<(&str, &str), SkillError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let start = if content.starts_with("---\n") {
        4
    } else if content.starts_with("---\r\n") {
        5
    } else {
        return Err(SkillError::Invalid(
            "SKILL.md must start with YAML frontmatter".to_owned(),
        ));
    };
    let mut offset = start;
    for line in content[start..].split_inclusive('\n') {
        let value = line.trim_end_matches(['\r', '\n']);
        if value == "---" {
            let frontmatter = &content[start..offset];
            let body = &content[offset + line.len()..];
            return Ok((frontmatter, body));
        }
        offset += line.len();
    }
    Err(SkillError::Invalid(
        "SKILL.md frontmatter has no closing delimiter".to_owned(),
    ))
}
