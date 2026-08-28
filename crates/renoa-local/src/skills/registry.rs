use std::{cmp::Reverse, collections::HashSet};

use super::SkillError;

pub(crate) const SEARCH_RESULT_LIMIT: usize = 200;
const QUERY_BYTES: usize = 256;
const QUERY_TOKENS: usize = 12;
const DESCRIPTION_SUMMARY_CHARS: usize = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillScope {
    Plugin,
    Global,
    Workspace,
}

impl SkillScope {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Global => "global",
            Self::Workspace => "workspace",
        }
    }

    pub(crate) fn from_stored(value: &str) -> Result<Self, SkillError> {
        match value {
            "plugin" => Ok(Self::Plugin),
            "global" => Ok(Self::Global),
            "workspace" => Ok(Self::Workspace),
            _ => Err(SkillError::Invalid(format!(
                "stored skill scope `{value}` is unsupported"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillSummary {
    pub(crate) name: String,
    pub(crate) description: String,
}

pub(crate) fn rank_skills(
    skills: Vec<SkillSummary>,
    query: &str,
) -> Result<Vec<SkillSummary>, SkillError> {
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
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    Ok(scored
        .into_iter()
        .take(SEARCH_RESULT_LIMIT)
        .map(|(_, mut skill)| {
            skill.description = summarize(&skill.description);
            skill
        })
        .collect())
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
            "skill has an invalid content digest".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SEARCH_RESULT_LIMIT, SkillSummary, rank_skills};

    #[test]
    fn search_ranks_matches_and_limits_descriptions() {
        let mut skills = vec![summary("review", "Review code")];
        for index in 0..8 {
            skills.push(summary(&format!("review-{index}"), "Review code"));
        }
        let long_description = "x".repeat(400);
        skills.push(summary("review-long", &long_description));

        let ranked = rank_skills(skills, "review").expect("rank skills");

        assert_eq!(ranked.len(), 10);
        assert_eq!(ranked[0].name, "review");
        assert_eq!(
            ranked
                .iter()
                .find(|skill| skill.name == "review-long")
                .expect("long matching description")
                .description
                .chars()
                .count(),
            320
        );
    }

    #[test]
    fn search_returns_two_hundred_matching_skill_summaries() {
        let skills = (0..201)
            .map(|index| summary(&format!("review-{index:03}"), "Review code."))
            .collect();

        let ranked = rank_skills(skills, "review").expect("rank skill summaries");

        assert_eq!(ranked.len(), SEARCH_RESULT_LIMIT);
        assert_eq!(ranked.first().expect("first match").name, "review-000");
        assert_eq!(ranked.last().expect("last match").name, "review-199");
    }

    fn summary(name: &str, description: &str) -> SkillSummary {
        SkillSummary {
            name: name.to_owned(),
            description: description.to_owned(),
        }
    }
}
