use super::{context_tokens, tool_kind};
use agent_client_protocol::schema::v1::ToolKind;
use renoa_agent::TokenUsage;

#[test]
fn coding_search_tools_use_the_standard_search_kind() {
    assert_eq!(tool_kind("grep"), ToolKind::Search);
    assert_eq!(tool_kind("find"), ToolKind::Search);
}

#[test]
fn context_usage_counts_every_normalized_token_lane() {
    assert_eq!(
        context_tokens(TokenUsage {
            input: 11,
            output: 7,
            cache_read: 5,
            cache_write: 3,
        }),
        Some(26)
    );
}

#[test]
fn context_usage_rejects_overflow_instead_of_wrapping() {
    assert_eq!(
        context_tokens(TokenUsage {
            input: u64::MAX,
            output: 1,
            cache_read: 0,
            cache_write: 0,
        }),
        None
    );
}
