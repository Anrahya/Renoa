use std::collections::HashSet;

use sha2::{Digest as _, Sha256};

use super::{MAX_ACTIVE_SKILL_INSTRUCTION_BYTES, MAX_ACTIVE_SKILLS};
use super::{SkillError, package::OwnedSkill, registry::SkillReference};

const FILE_SAMPLE_LIMIT: usize = 20;

pub(crate) struct ActiveSkillContext {
    pub(crate) instructions: String,
    pub(crate) references: HashSet<String>,
    pub(crate) revision: String,
}

pub(super) fn one(skill: &OwnedSkill) -> Result<String, SkillError> {
    let reference = reference(skill)?;
    let mut output = String::new();
    output.push_str("<skill_content name=\"");
    output.push_str(&skill.metadata.name);
    output.push_str("\" reference=\"");
    output.push_str(&reference.to_string());
    output.push_str("\">\n");
    output.push_str("Base directory: ");
    output.push_str(skill.root.to_str().ok_or_else(|| {
        SkillError::Invalid(format!(
            "installed skill path `{}` is not UTF-8",
            skill.root.display()
        ))
    })?);
    output.push_str("\nRelative resource paths are resolved from this directory.\n");
    if let Some(compatibility) = &skill.metadata.compatibility {
        output.push_str("Compatibility: ");
        output.push_str(compatibility);
        output.push('\n');
    }
    output.push_str("Files (bounded sample):\n");
    for file in skill.files.iter().take(FILE_SAMPLE_LIMIT) {
        output.push_str("- ");
        output.push_str(file);
        output.push('\n');
    }
    if skill.files.len() > FILE_SAMPLE_LIMIT {
        output.push_str("- … ");
        output.push_str(&(skill.files.len() - FILE_SAMPLE_LIMIT).to_string());
        output.push_str(" more files\n");
    }
    output.push('\n');
    output.push_str(&skill.body);
    if !skill.body.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("</skill_content>");
    Ok(output)
}

pub(crate) fn active(skills: &[OwnedSkill]) -> Result<Option<ActiveSkillContext>, SkillError> {
    if skills.is_empty() {
        return Ok(None);
    }
    if skills.len() > MAX_ACTIVE_SKILLS {
        return Err(SkillError::Conflict(format!(
            "session exceeds the {MAX_ACTIVE_SKILLS}-skill activation limit"
        )));
    }
    let mut instructions = String::from(
        "<active_skills>\nThese exact skill revisions are active for this session. Follow their instructions when relevant.\n\n",
    );
    let mut references = HashSet::with_capacity(skills.len());
    let mut hasher = Sha256::new();
    hasher.update(b"renoa.active-skills.v1\0");
    let mut instruction_bytes = 0_usize;
    for skill in skills {
        let reference = reference(skill)?.to_string();
        hasher.update((reference.len() as u64).to_be_bytes());
        hasher.update(reference.as_bytes());
        references.insert(reference);
        let rendered = one(skill)?;
        instruction_bytes = instruction_bytes
            .checked_add(rendered.len())
            .ok_or_else(|| SkillError::Conflict("active skill size overflowed".to_owned()))?;
        if instruction_bytes > MAX_ACTIVE_SKILL_INSTRUCTION_BYTES {
            return Err(SkillError::Conflict(format!(
                "active skill instructions exceed {MAX_ACTIVE_SKILL_INSTRUCTION_BYTES} bytes"
            )));
        }
        instructions.push_str(&rendered);
        instructions.push_str("\n\n");
    }
    instructions.push_str("</active_skills>");
    Ok(Some(ActiveSkillContext {
        instructions,
        references,
        revision: hex(hasher.finalize().as_slice()),
    }))
}

pub(super) fn reference(skill: &OwnedSkill) -> Result<SkillReference, SkillError> {
    SkillReference::new(skill.metadata.name.clone(), skill.digest.clone())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}
