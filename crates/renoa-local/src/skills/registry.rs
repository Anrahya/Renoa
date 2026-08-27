use std::{cmp::Reverse, collections::HashSet, fmt, str::FromStr};

use super::SkillError;

pub(crate) const SEARCH_RESULT_LIMIT: usize = 5;
const QUERY_BYTES: usize = 256;
const QUERY_TOKENS: usize = 12;
const DESCRIPTION_SUMMARY_CHARS: usize = 320;
const REFERENCE_PREFIX: &str = "skill";

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct SkillReference {
    name: String,
    digest: String,
}

impl SkillReference {
    pub(crate) fn new(
        name: impl Into<String>,
        digest: impl Into<String>,
    ) -> Result<Self, SkillError> {
        let reference = Self {
            name: name.into(),
            digest: digest.into(),
        };
        validate_name(&reference.name)?;
        validate_digest(&reference.digest)?;
        Ok(reference)
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
}

impl fmt::Display for SkillReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{REFERENCE_PREFIX}:{}:{}",
            self.name, self.digest
        )
    }
}

impl FromStr for SkillReference {
    type Err = SkillError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        let mut parts = encoded.split(':');
        let prefix = parts.next();
        let name = parts.next();
        let digest = parts.next();
        if prefix != Some(REFERENCE_PREFIX)
            || name.is_none()
            || digest.is_none()
            || parts.next().is_some()
        {
            return Err(SkillError::Invalid(
                "skill reference must be `skill:<name>:<content-digest>`".to_owned(),
            ));
        }
        Self::new(name.unwrap_or_default(), digest.unwrap_or_default())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillScope {
    Global,
    Workspace,
}

impl SkillScope {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
        }
    }

    pub(crate) fn from_stored(value: &str) -> Result<Self, SkillError> {
        match value {
            "global" => Ok(Self::Global),
            "workspace" => Ok(Self::Workspace),
            _ => Err(SkillError::Invalid(format!(
                "stored skill scope `{value}` is unsupported"
            ))),
        }
    }

    const fn rank(self) -> u8 {
        match self {
            Self::Workspace => 1,
            Self::Global => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillSummary {
    pub(crate) scope: SkillScope,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) digest: String,
}

impl SkillSummary {
    pub(crate) fn reference(&self) -> Result<SkillReference, SkillError> {
        SkillReference::new(self.name.clone(), self.digest.clone())
    }
}

pub(crate) struct RankedSkills {
    pub(crate) matches: Vec<SkillSummary>,
    pub(crate) total_matches: usize,
}

pub(crate) fn rank_skills(
    skills: Vec<SkillSummary>,
    query: &str,
) -> Result<RankedSkills, SkillError> {
    let query = query.trim();
    if query.is_empty() || query.len() > QUERY_BYTES {
        return Err(SkillError::Invalid(format!(
            "skill search query must be 1-{QUERY_BYTES} UTF-8 bytes"
        )));
    }
    let browse = query == "*";
    let phrase = query.to_lowercase();
    let mut observed = HashSet::new();
    let tokens = phrase
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .filter(|token| observed.insert((*token).to_owned()))
        .take(QUERY_TOKENS)
        .collect::<Vec<_>>();
    if !browse && tokens.is_empty() {
        return Err(SkillError::Invalid(
            "skill search query must contain a letter or digit, or be `*`".to_owned(),
        ));
    }

    let mut scored = skills
        .into_iter()
        .filter_map(|skill| score(&skill, &phrase, &tokens, browse).map(|score| (score, skill)))
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        Reverse(left.0)
            .cmp(&Reverse(right.0))
            .then_with(|| Reverse(left.1.scope.rank()).cmp(&Reverse(right.1.scope.rank())))
            .then_with(|| left.1.name.cmp(&right.1.name))
            .then_with(|| left.1.digest.cmp(&right.1.digest))
    });
    let total_matches = scored.len();
    let matches = scored
        .into_iter()
        .take(SEARCH_RESULT_LIMIT)
        .map(|(_, mut skill)| {
            skill.description = summarize(&skill.description);
            skill
        })
        .collect();
    Ok(RankedSkills {
        matches,
        total_matches,
    })
}

fn score(skill: &SkillSummary, phrase: &str, tokens: &[&str], browse: bool) -> Option<u32> {
    if browse {
        return Some(0);
    }
    let name = skill.name.to_lowercase();
    let description = skill.description.to_lowercase();
    let mut score = 0;
    if name == phrase {
        score += 2_000;
    } else if name.starts_with(phrase) {
        score += 1_200;
    } else if name.contains(phrase) {
        score += 800;
    }
    if description.contains(phrase) {
        score += 300;
    }
    let mut matched = false;
    for token in tokens {
        let token_score = if name.contains(token) {
            160
        } else if description.contains(token) {
            20
        } else {
            0
        };
        matched |= token_score > 0;
        score += token_score;
    }
    matched.then_some(score)
}

fn summarize(description: &str) -> String {
    let collapsed = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= DESCRIPTION_SUMMARY_CHARS {
        return collapsed;
    }
    let mut summary = collapsed
        .chars()
        .take(DESCRIPTION_SUMMARY_CHARS.saturating_sub(1))
        .collect::<String>();
    summary.push('…');
    summary
}

pub(crate) fn validate_name(name: &str) -> Result<(), SkillError> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(SkillError::Invalid(format!(
            "skill name `{name}` does not satisfy the Agent Skills naming rules"
        )));
    }
    Ok(())
}

pub(super) fn validate_digest(digest: &str) -> Result<(), SkillError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SkillError::Invalid(
            "skill reference has an invalid content digest".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SkillReference, SkillScope, SkillSummary, rank_skills};

    #[test]
    fn exact_references_reject_ambiguous_names_or_digests() {
        let reference = SkillReference::new("code-review", "a".repeat(64)).expect("reference");
        assert_eq!(
            reference.to_string().parse::<SkillReference>().unwrap(),
            reference
        );
        assert!(
            "skill:Code:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse::<SkillReference>()
                .is_err()
        );
        assert!("skill:code-review:a:b".parse::<SkillReference>().is_err());
    }

    #[test]
    fn search_is_bounded_and_prefers_workspace_matches() {
        let mut skills = vec![summary(SkillScope::Global, "review", "Review code")];
        skills.push(summary(
            SkillScope::Workspace,
            "review",
            "Review this project",
        ));
        for index in 0..8 {
            skills.push(summary(
                SkillScope::Global,
                &format!("review-{index}"),
                "Review code",
            ));
        }

        let ranked = rank_skills(skills, "review").expect("rank skills");

        assert_eq!(ranked.total_matches, 10);
        assert_eq!(ranked.matches.len(), 5);
        assert_eq!(ranked.matches[0].scope, SkillScope::Workspace);
    }

    fn summary(scope: SkillScope, name: &str, description: &str) -> SkillSummary {
        SkillSummary {
            scope,
            name: name.to_owned(),
            description: description.to_owned(),
            digest: "a".repeat(64),
        }
    }
}
