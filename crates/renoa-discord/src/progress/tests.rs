use super::{Command, Step};
use crate::store::ProgressTarget;

fn command() -> Command {
    Command::new(ProgressTarget {
        channel_id: "202".to_owned(),
        reply_to: Some("101".to_owned()),
    })
}

fn tool(call_id: &str, name: &str) -> Step {
    Step::ToolStarted {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
    }
}

#[test]
fn a_command_without_tools_shows_only_typing() {
    let mut command = command();
    command.apply(Step::Working);
    command.apply(Step::Said("Here is the answer.".to_owned()));
    command.apply(Step::Finished);
    assert_eq!(command.render(), None);
}

#[test]
fn intermediate_messages_and_tool_calls_are_listed_until_the_command_finishes() {
    let mut command = command();
    command.apply(Step::Said("Searching the library first.".to_owned()));
    command.apply(tool("one", "plugin_search"));
    assert_eq!(
        command.render().as_deref(),
        Some("**Working…**\n💭 Searching the library first.\n🔧 `plugin_search` …")
    );

    command.apply(Step::ToolFinished {
        call_id: "one".to_owned(),
        is_error: false,
    });
    command.apply(tool("two", "plugin_manage"));
    command.apply(Step::ToolFinished {
        call_id: "two".to_owned(),
        is_error: true,
    });
    command.apply(Step::Said("The final answer.".to_owned()));
    assert_eq!(
        command.render().as_deref(),
        Some(
            "**Working…**\n💭 Searching the library first.\n🔧 `plugin_search` ✓\n🔧 `plugin_manage` ✗"
        )
    );
    command.apply(Step::Finished);
    assert_eq!(
        command.render(),
        None,
        "a finished command shows no progress"
    );
}

#[test]
fn long_progress_keeps_the_latest_steps_within_one_discord_message() {
    let mut command = command();
    for index in 0..200 {
        command.apply(Step::Said(format!("step {index} {}", "x".repeat(300))));
        command.apply(tool(&index.to_string(), "bash"));
    }
    let body = command.render().expect("progress");
    assert!(body.chars().count() <= 2000, "{}", body.len());
    assert!(body.contains("earlier steps"));
    assert!(body.ends_with("🔧 `bash` …"));
    assert!(body.contains("step 199 "));
}

#[test]
fn a_tool_record_is_named_by_the_plugin_tool_it_runs() {
    use renoa_control::TaskEventKind;
    use renoa_protocol::{
        CommandId, ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId,
    };

    let kind = TaskEventKind::ExecutionEvent {
        command_id: CommandId::new(),
        event: ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id: ExecutionId::new(),
            sequence: 3,
            recorded_at_ms: 0,
            kind: ExecutionEventKind::ToolStarted {
                call_id: "one".to_owned(),
                name: "tool_execute".to_owned(),
                arguments: serde_json::json!({
                    "reference": format!("mcp:exa:{}:web_search_exa", "a".repeat(64)),
                }),
            },
        },
    };
    let (_, step) = Step::of(&kind);
    assert_eq!(step, tool("one", "exa.web_search_exa"));
}
