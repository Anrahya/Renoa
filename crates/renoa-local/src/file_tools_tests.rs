use std::{fs, sync::Arc};

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{EditFile, MAX_FILE_WRITE_BYTES, ReadFileInput, WriteFile, read_page};
use crate::{output::MAX_TOOL_OUTPUT_BYTES, tool_input::decode};

#[tokio::test]
async fn read_page_uses_one_based_offsets_and_returns_a_continuation() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("lines.txt");
    fs::write(&path, "one\ntwo\nthree\n").expect("write fixture");

    let page = read_page(&path, 2, 1, &CancellationToken::new())
        .await
        .expect("read second line");

    assert_eq!(page.lines, 1);
    assert_eq!(page.next_offset, Some(3));
    assert!(page.text.starts_with("two\n"));
    assert!(page.text.contains("Continue with offset=3"));
    assert!(!page.text.contains("three"));
}

#[tokio::test]
async fn read_page_bounds_many_short_lines() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("many-lines.txt");
    fs::write(&path, "line\n".repeat(2_001)).expect("write fixture");

    let page = read_page(&path, 1, 2_000, &CancellationToken::new())
        .await
        .expect("read bounded page");

    assert_eq!(page.lines, 2_000);
    assert_eq!(page.next_offset, Some(2_001));
    assert!(page.text.len() <= MAX_TOOL_OUTPUT_BYTES);
}

#[tokio::test]
async fn read_page_does_not_buffer_or_return_one_giant_line() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("giant-line.txt");
    fs::write(&path, format!("{}\n", "x".repeat(60 * 1_024))).expect("write fixture");

    let page = read_page(&path, 1, 2_000, &CancellationToken::new())
        .await
        .expect("return giant-line guidance");

    assert_eq!(page.lines, 0);
    assert_eq!(page.next_offset, Some(1));
    assert!(page.text.contains("Line 1 exceeds"));
    assert!(page.text.len() <= MAX_TOOL_OUTPUT_BYTES);
}

#[test]
fn typed_tool_inputs_reject_fields_the_schema_does_not_advertise() {
    let error = decode::<ReadFileInput>(serde_json::json!({
        "path": "file.txt",
        "surprise": true
    }))
    .err()
    .expect("unknown field must fail");

    assert!(error.to_string().contains("unknown field `surprise`"));
}

#[tokio::test]
async fn mutation_results_use_relative_paths_and_enforce_the_write_limit() {
    let directory = tempdir().expect("temporary directory");
    let root = Arc::new(directory.path().to_path_buf());
    let write = WriteFile::new(Arc::clone(&root));
    let edit = EditFile::new(root);

    let write_result = invoke_tool(
        Some(&write),
        tool_call(
            "write_file",
            serde_json::json!({ "path": "value.txt", "content": "old\n" }),
        ),
        CancellationToken::new(),
        None,
    )
    .await
    .expect("write result is definite");
    assert_eq!(result_text(&write_result.content), "Wrote value.txt");
    assert_eq!(
        write_result.details,
        Some(serde_json::json!({ "path": "value.txt" }))
    );

    let edit_result = invoke_tool(
        Some(&edit),
        tool_call(
            "edit_file",
            serde_json::json!({
                "path": "value.txt",
                "old_text": "old\n",
                "new_text": "new\n"
            }),
        ),
        CancellationToken::new(),
        None,
    )
    .await
    .expect("edit result is definite");
    assert_eq!(result_text(&edit_result.content), "Edited value.txt");

    let oversized = invoke_tool(
        Some(&edit),
        tool_call(
            "edit_file",
            serde_json::json!({
                "path": "value.txt",
                "old_text": "new\n",
                "new_text": "x".repeat(MAX_FILE_WRITE_BYTES + 1)
            }),
        ),
        CancellationToken::new(),
        None,
    )
    .await
    .expect("edit result is definite");
    assert!(oversized.is_error);
    assert!(result_text(&oversized.content).contains("write limit"));
    assert_eq!(
        fs::read_to_string(directory.path().join("value.txt")).expect("read unchanged fixture"),
        "new\n"
    );
}

fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("{name}-call"),
        name: name.to_owned(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

fn result_text(content: &[ContentBlock]) -> &str {
    let [ContentBlock::Text { text }] = content else {
        panic!("tool did not return exactly one text block")
    };
    text
}
