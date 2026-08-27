use std::{cmp::Reverse, collections::HashSet, fmt, str::FromStr};

use super::{McpHostError, validate_identity};

pub(crate) const SEARCH_RESULT_LIMIT: usize = 5;
pub(crate) const LOAD_REFERENCE_LIMIT: usize = 3;
pub(crate) const LOAD_OUTPUT_BYTES: usize = 64 * 1_024;
const QUERY_BYTES: usize = 256;
const QUERY_TOKENS: usize = 12;
const DESCRIPTION_SUMMARY_CHARS: usize = 320;
const REFERENCE_PREFIX: &str = "mcp";

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct McpToolReference {
    connection_id: String,
    catalog_digest: String,
    tool_name: String,
}

impl McpToolReference {
    pub(crate) fn new(
        connection_id: impl Into<String>,
        catalog_digest: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Result<Self, McpHostError> {
        let reference = Self {
            connection_id: connection_id.into(),
            catalog_digest: catalog_digest.into(),
            tool_name: tool_name.into(),
        };
        validate_identity("connection", &reference.connection_id)?;
        validate_identity("tool", &reference.tool_name)?;
        if reference.catalog_digest.len() != 64
            || !reference
                .catalog_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(McpHostError::Invalid(
                "MCP tool reference has an invalid catalog digest".to_owned(),
            ));
        }
        Ok(reference)
    }

    pub(crate) fn connection_id(&self) -> &str {
        &self.connection_id
    }

    pub(crate) fn catalog_digest(&self) -> &str {
        &self.catalog_digest
    }

    pub(crate) fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

impl fmt::Display for McpToolReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{REFERENCE_PREFIX}:{}:{}:{}",
            self.connection_id, self.catalog_digest, self.tool_name
        )
    }
}

impl FromStr for McpToolReference {
    type Err = McpHostError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        let mut parts = encoded.split(':');
        let prefix = parts.next();
        let connection_id = parts.next();
        let catalog_digest = parts.next();
        let tool_name = parts.next();
        if prefix != Some(REFERENCE_PREFIX)
            || connection_id.is_none()
            || catalog_digest.is_none()
            || tool_name.is_none()
            || parts.next().is_some()
        {
            return Err(McpHostError::Invalid(
                "MCP tool reference must be `mcp:<connection>:<catalog-digest>:<tool>`".to_owned(),
            ));
        }
        Self::new(
            connection_id.unwrap_or_default(),
            catalog_digest.unwrap_or_default(),
            tool_name.unwrap_or_default(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpToolSummary {
    pub(super) integration_id: String,
    pub(super) connection_id: String,
    pub(super) catalog_digest: String,
    pub(super) name: String,
    pub(super) description: String,
}

impl McpToolSummary {
    pub(super) fn reference(&self) -> Result<McpToolReference, McpHostError> {
        McpToolReference::new(
            self.connection_id.clone(),
            self.catalog_digest.clone(),
            self.name.clone(),
        )
    }
}

pub(crate) struct RankedTools {
    pub(super) matches: Vec<McpToolSummary>,
    pub(super) total_matches: usize,
}

pub(crate) fn rank_tools(
    tools: Vec<McpToolSummary>,
    query: &str,
) -> Result<RankedTools, McpHostError> {
    let query = query.trim();
    if query.is_empty() || query.len() > QUERY_BYTES {
        return Err(McpHostError::Invalid(format!(
            "tool search query must be 1-{QUERY_BYTES} UTF-8 bytes"
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
        return Err(McpHostError::Invalid(
            "tool search query must contain a letter or digit, or be `*`".to_owned(),
        ));
    }

    let mut scored = tools
        .into_iter()
        .filter_map(|tool| score_tool(&tool, &phrase, &tokens, browse).map(|score| (score, tool)))
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        Reverse(left.0)
            .cmp(&Reverse(right.0))
            .then_with(|| left.1.connection_id.cmp(&right.1.connection_id))
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    let total_matches = scored.len();
    let matches = scored
        .into_iter()
        .take(SEARCH_RESULT_LIMIT)
        .map(|(_, mut tool)| {
            tool.description = summarize(&tool.description);
            tool
        })
        .collect();
    Ok(RankedTools {
        matches,
        total_matches,
    })
}

fn score_tool(tool: &McpToolSummary, phrase: &str, tokens: &[&str], browse: bool) -> Option<u32> {
    if browse {
        return Some(0);
    }
    let name = tool.name.to_lowercase();
    let connection = tool.connection_id.to_lowercase();
    let integration = tool.integration_id.to_lowercase();
    let description = tool.description.to_lowercase();
    let mut score = 0_u32;
    if name == phrase {
        score += 2_000;
    } else if name.starts_with(phrase) {
        score += 1_200;
    } else if name.contains(phrase) {
        score += 800;
    }
    if connection == phrase || integration == phrase {
        score += 700;
    } else if connection.contains(phrase) || integration.contains(phrase) {
        score += 400;
    }
    if description.contains(phrase) {
        score += 300;
    }
    let mut matched = false;
    for token in tokens {
        let token_score = if name.contains(token) {
            160
        } else if connection.contains(token) || integration.contains(token) {
            100
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

#[cfg(test)]
mod tests {
    use super::{McpToolReference, McpToolSummary, rank_tools};

    #[test]
    fn references_are_exact_and_reject_ambiguous_or_uppercase_digests() {
        let digest = "a".repeat(64);
        let reference =
            McpToolReference::new("github", &digest, "search_code").expect("valid reference");
        let encoded = reference.to_string();

        assert_eq!(
            encoded.parse::<McpToolReference>().expect("round trip"),
            reference
        );
        assert!(
            format!("mcp:github:{}:search_code", "A".repeat(64))
                .parse::<McpToolReference>()
                .is_err()
        );
        assert!(
            format!("mcp:github:{digest}:search:code")
                .parse::<McpToolReference>()
                .is_err()
        );
    }

    #[test]
    fn search_is_bounded_relevant_and_deterministic() {
        let tools = vec![
            summary("work", "read_issue", "Read a GitHub issue"),
            summary("personal", "search_code", "Search code in repositories"),
            summary("work", "search_issues", "Search GitHub issues"),
            summary("work", "unrelated", "Send an email"),
        ];

        let ranked = rank_tools(tools, "issue").expect("rank tools");

        assert_eq!(ranked.total_matches, 2);
        assert_eq!(
            ranked
                .matches
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["read_issue", "search_issues"]
        );
    }

    fn summary(connection: &str, name: &str, description: &str) -> McpToolSummary {
        McpToolSummary {
            integration_id: "github".to_owned(),
            connection_id: connection.to_owned(),
            catalog_digest: "a".repeat(64),
            name: name.to_owned(),
            description: description.to_owned(),
        }
    }
}
