use std::{fs, sync::Arc};

use renoa_agent::{ContentBlock, Tool, ToolCall, ToolResult, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{Find, Grep};
use crate::ripgrep::Ripgrep;

#[tokio::test]
async fn grep_and_find_return_sorted_results_without_ignored_or_hidden_paths() {
    let directory = tempdir().expect("temporary directory");
    let root = directory.path();
    fs::create_dir_all(root.join("src")).expect("create source directory");
    fs::create_dir_all(root.join(".github/workflows")).expect("create hidden directory");
    fs::create_dir(root.join(".git")).expect("mark fixture as a repository");
    fs::write(root.join(".gitignore"), "ignored.rs\n").expect("write ignore file");
    fs::write(root.join("src/a.rs"), "fn alpha() { /* needle */ }\n").expect("write first source");
    fs::write(root.join("src/b.rs"), "fn beta() { /* needle */ }\n").expect("write second source");
    fs::write(root.join("ignored.rs"), "needle\n").expect("write ignored source");
    fs::write(root.join(".hidden.rs"), "needle\n").expect("write hidden file");
    fs::write(root.join(".github/workflows/ci.yml"), "needle\n").expect("write hidden source");
    fs::write(root.join(".git/config"), "needle\n").expect("write git metadata");

    let root = Arc::new(root.to_path_buf());
    let ripgrep = Arc::new(Ripgrep::discover().expect("discover ripgrep"));
    let grep = Grep::new(Arc::clone(&root), Arc::clone(&ripgrep));
    let find = Find::new(Arc::clone(&root), ripgrep);

    let grep_result = call_tool(
        &grep,
        "grep",
        json!({ "pattern": "needle", "glob": "*.rs" }),
    )
    .await;
    assert!(!grep_result.is_error, "{}", result_text(&grep_result));
    assert_eq!(
        result_text(&grep_result),
        "src/a.rs:1:fn alpha() { /* needle */ }\nsrc/b.rs:1:fn beta() { /* needle */ }\n"
    );

    let find_result = call_tool(&find, "find", json!({ "pattern": "*.rs" })).await;
    assert!(!find_result.is_error, "{}", result_text(&find_result));
    assert_eq!(result_text(&find_result), "src/a.rs\nsrc/b.rs\n");

    let hidden_result = call_tool(&find, "find", json!({ "pattern": ".github/**/*.yml" })).await;
    assert!(!hidden_result.is_error, "{}", result_text(&hidden_result));
    assert_eq!(result_text(&hidden_result), "No files found.");
    assert_eq!(
        hidden_result.details,
        Some(json!({ "results": 0, "truncated": false }))
    );

    let all_result = call_tool(&find, "find", json!({ "pattern": "**/*" })).await;
    assert!(!all_result.is_error, "{}", result_text(&all_result));
    assert_eq!(result_text(&all_result), "src/a.rs\nsrc/b.rs\n");
}

#[tokio::test]
async fn grep_and_find_reject_explicit_hidden_search_paths() {
    let directory = tempdir().expect("temporary directory");
    let root = directory.path();
    fs::create_dir(root.join(".hidden")).expect("create hidden directory");
    fs::write(root.join(".hidden/secret.txt"), "needle\n").expect("write hidden source");
    std::os::unix::fs::symlink(root.join(".hidden/secret.txt"), root.join("visible.txt"))
        .expect("create visible symlink to hidden source");

    let root = Arc::new(root.to_path_buf());
    let ripgrep = Arc::new(Ripgrep::discover().expect("discover ripgrep"));
    let grep = Grep::new(Arc::clone(&root), Arc::clone(&ripgrep));
    let find = Find::new(root, ripgrep);

    let grep_result = call_tool(
        &grep,
        "grep",
        json!({ "pattern": "needle", "path": ".hidden/secret.txt" }),
    )
    .await;
    assert!(grep_result.is_error);
    assert!(result_text(&grep_result).contains("use bash"));

    let alias_result = call_tool(
        &grep,
        "grep",
        json!({ "pattern": "needle", "path": "visible.txt" }),
    )
    .await;
    assert!(alias_result.is_error);
    assert!(result_text(&alias_result).contains("use bash"));

    let find_result = call_tool(
        &find,
        "find",
        json!({ "pattern": "*.txt", "path": ".hidden" }),
    )
    .await;
    assert!(find_result.is_error);
    assert!(result_text(&find_result).contains("use bash"));
}

#[tokio::test]
async fn search_limits_are_explicit_and_model_visible() {
    let directory = tempdir().expect("temporary directory");
    let root = directory.path();
    fs::write(root.join("a.txt"), "match\n").expect("write first fixture");
    fs::write(root.join("b.txt"), "match\n").expect("write second fixture");

    let root = Arc::new(root.to_path_buf());
    let ripgrep = Arc::new(Ripgrep::discover().expect("discover ripgrep"));
    let grep = Grep::new(Arc::clone(&root), Arc::clone(&ripgrep));
    let find = Find::new(root, ripgrep);

    let grep_result = call_tool(&grep, "grep", json!({ "pattern": "match", "limit": 1 })).await;
    assert!(!grep_result.is_error, "{}", result_text(&grep_result));
    assert!(result_text(&grep_result).contains("Grep output capped"));
    assert_eq!(
        grep_result.details,
        Some(json!({ "matches": 1, "truncated": true }))
    );

    let find_result = call_tool(&find, "find", json!({ "pattern": "*.txt", "limit": 1 })).await;
    assert!(!find_result.is_error, "{}", result_text(&find_result));
    assert!(result_text(&find_result).contains("Find output capped"));
    assert_eq!(
        find_result.details,
        Some(json!({ "results": 1, "truncated": true }))
    );
}

#[tokio::test]
async fn non_matching_files_do_not_create_a_false_truncation_notice() {
    let directory = tempdir().expect("temporary directory");
    let root = directory.path();
    fs::write(root.join("a.rs"), "match\n").expect("write selected fixture");
    fs::write(root.join("b.txt"), "match\n").expect("write filtered fixture");

    let root = Arc::new(root.to_path_buf());
    let ripgrep = Arc::new(Ripgrep::discover().expect("discover ripgrep"));
    let grep = Grep::new(Arc::clone(&root), Arc::clone(&ripgrep));
    let find = Find::new(root, ripgrep);

    let grep_result = call_tool(
        &grep,
        "grep",
        json!({ "pattern": "match", "glob": "*.rs", "limit": 1 }),
    )
    .await;
    assert_eq!(
        grep_result.details,
        Some(json!({ "matches": 1, "truncated": false }))
    );
    assert!(!result_text(&grep_result).contains("output capped"));

    let find_result = call_tool(&find, "find", json!({ "pattern": "*.rs", "limit": 1 })).await;
    assert_eq!(
        find_result.details,
        Some(json!({ "results": 1, "truncated": false }))
    );
    assert!(!result_text(&find_result).contains("output capped"));
}

#[tokio::test]
async fn search_rejects_workspace_escape_and_reports_bad_regex() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(directory.path().join("secret.txt"), "outside\n").expect("write outside fixture");

    let root = Arc::new(workspace);
    let ripgrep = Arc::new(Ripgrep::discover().expect("discover ripgrep"));
    let grep = Grep::new(Arc::clone(&root), Arc::clone(&ripgrep));
    let find = Find::new(root, ripgrep);

    let escape = call_tool(
        &grep,
        "grep",
        json!({ "pattern": "outside", "path": "../secret.txt" }),
    )
    .await;
    assert!(escape.is_error);
    assert!(result_text(&escape).contains("relative to the workspace"));
    assert!(!result_text(&escape).contains("outside\n"));

    let invalid = call_tool(&grep, "grep", json!({ "pattern": "[" })).await;
    assert!(invalid.is_error);
    assert!(result_text(&invalid).contains("ripgrep exited with code 2"));

    let invalid_glob = call_tool(&find, "find", json!({ "pattern": "[" })).await;
    assert!(invalid_glob.is_error);
    assert!(result_text(&invalid_glob).contains("invalid glob pattern"));
}

async fn call_tool(tool: &dyn Tool, name: &str, arguments: Value) -> ToolResult {
    invoke_tool(
        Some(tool),
        ToolCall {
            id: format!("{name}-call"),
            name: name.to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("search result is definite")
}

fn result_text(result: &ToolResult) -> &str {
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("tool did not return exactly one text block")
    };
    text
}
