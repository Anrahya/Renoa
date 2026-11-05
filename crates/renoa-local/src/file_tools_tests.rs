use std::{fs, sync::Arc};

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{EditFile, MAX_FILE_WRITE_BYTES, ReadFile, ReadFileInput, WriteFile, read_page};
use crate::{images::MAX_INLINE_IMAGE_BYTES, output::MAX_TOOL_OUTPUT_BYTES, tool_input::decode};

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

#[cfg(unix)]
#[tokio::test]
async fn a_dangling_symlink_cannot_redirect_a_workspace_write() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let outside = directory.path().join("outside.txt");
    fs::create_dir(&workspace).expect("create workspace");
    std::os::unix::fs::symlink(&outside, workspace.join("link.txt"))
        .expect("create dangling symlink");
    let write = WriteFile::new(Arc::new(workspace));

    let result = invoke_tool(
        Some(&write),
        tool_call(
            "write_file",
            serde_json::json!({ "path": "link.txt", "content": "escaped\n" }),
        ),
        CancellationToken::new(),
        None,
    )
    .await
    .expect("workspace rejection is definite");

    assert!(result.is_error);
    assert!(!outside.exists(), "write followed a dangling symlink");
}

#[tokio::test]
async fn an_image_is_read_as_the_image_itself_and_one_too_large_says_what_to_do() {
    let directory = tempdir().expect("temporary directory");
    let root = Arc::new(directory.path().to_path_buf());
    let read = ReadFile::new(Arc::clone(&root));
    let call = |path: &str| {
        invoke_tool(
            Some(&read),
            tool_call("read_file", serde_json::json!({ "path": path })),
            CancellationToken::new(),
            None,
        )
    };
    // Known by its bytes, whatever its name says.
    let png = [&b"\x89PNG\r\n\x1a\n"[..], &[3; 100]].concat();
    fs::write(directory.path().join("photo.dat"), &png).expect("write image");
    let shown = call("photo.dat").await.expect("image result is definite");
    assert!(!shown.is_error);
    let [
        ContentBlock::Text { text },
        ContentBlock::Image { mime_type, data },
    ] = &shown.content[..]
    else {
        panic!("an image read is a line and the image: {:?}", shown.content)
    };
    assert_eq!(text, "photo.dat is an image (108 bytes), shown here.");
    assert_eq!(mime_type, "image/png");
    assert_eq!(
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).expect("base64"),
        png
    );

    let mut large = vec![0; MAX_INLINE_IMAGE_BYTES + 1];
    large[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    fs::write(directory.path().join("large.jpg"), &large).expect("write large image");
    let refused = call("large.jpg").await.expect("refusal is definite");
    assert!(refused.is_error);
    assert!(
        result_text(&refused.content).contains("Make a smaller copy with bash"),
        "{:?}",
        refused.content
    );

    fs::write(directory.path().join("notes.txt"), "plain\n").expect("write text");
    let text = call("notes.txt").await.expect("text result is definite");
    assert!(result_text(&text.content).starts_with("plain\n"));
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
