//! The texts a model reads when an owner tool refuses a call: each names the
//! field, what was sent and what is expected, and shows a correct call.

use serde_json::{Value, json};

use super::input::{Input, Kind, parse};
use crate::{host::feed::Urgency, plugins::host::HostPluginId};

fn refused(kind: Kind, arguments: &Value) -> String {
    parse(kind, arguments).expect_err("refused").to_string()
}

#[test]
fn each_refusal_says_what_was_wrong_and_shows_a_correct_call() {
    let cases = [
        (
            Kind::Ask,
            json!({"question": "Book the 6pm table?"}),
            r#"ask_owner needs "urgency": "whenever", "soon" or "now". You sent none. Use "now" only if waiting costs the owner something within minutes. Example: {"question": "Book the 6pm table?", "choices": ["Yes", "No"], "urgency": "soon"}"#,
        ),
        (
            Kind::Notify,
            json!({"message": "The build failed.", "urgency": "urgent"}),
            r#""urgency" must be "whenever", "soon" or "now"; you sent "urgent". Example: {"message": "The build failed twice.", "urgency": "soon"}"#,
        ),
        (
            Kind::Ask,
            json!({"question": "Book it?", "choices": ["Yes"], "urgency": "soon"}),
            r#""choices" needs 2 to 6 options; you sent 1. Leave "choices" out when the owner should answer in their own words."#,
        ),
        (
            Kind::Ask,
            json!({"question": "Book it?", "options": ["Yes", "No"], "urgency": "soon"}),
            r#"ask_owner has no field "options". Its fields are question, choices, urgency. Did you mean "choices"?"#,
        ),
        (
            Kind::Ask,
            json!({"question": "a".repeat(310), "urgency": "soon"}),
            r#""question" is 310 characters; the limit is 240. Shorten it to the one decision you need, and write any background in the message before this call."#,
        ),
        (
            Kind::Notify,
            json!({"urgency": "soon"}),
            r#"notify_owner needs "message": what the owner should know, at most 240 characters. Example: {"message": "The build failed twice.", "urgency": "soon"}"#,
        ),
        (
            Kind::Ask,
            json!({"question": "Book it?", "choices": ["Yes", "x".repeat(41)], "urgency": "now"}),
            "choice 2 is 41 characters; the limit is 40. Shorten it to the answer itself.",
        ),
        (
            Kind::Ask,
            json!({"question": "Book it?", "choices": ["Yes", "Yes"], "urgency": "now"}),
            r#"choice 2 repeats "Yes". Give each option once."#,
        ),
        (
            Kind::Notify,
            json!({"text": "Done.", "urgency": "soon"}),
            r#"notify_owner has no field "text". Its fields are message, urgency. Did you mean "message"?"#,
        ),
    ];
    for (kind, arguments, expected) in cases {
        assert_eq!(refused(kind, &arguments), expected);
    }
}

#[test]
fn a_well_formed_call_is_trimmed_and_keeps_its_choices_in_order() {
    assert_eq!(
        parse(
            Kind::Ask,
            &json!({"question": " Book it? ", "choices": ["No", " Yes"], "urgency": "now"})
        )
        .expect("ask"),
        Input::Ask {
            question: "Book it?".to_owned(),
            choices: Some(vec!["No".to_owned(), "Yes".to_owned()]),
            urgency: Urgency::Now,
        }
    );
    assert_eq!(
        parse(
            Kind::Notify,
            &json!({"message": "Done.", "urgency": "whenever"})
        )
        .expect("notify"),
        Input::Notify {
            message: "Done.".to_owned(),
            urgency: Urgency::Whenever,
        }
    );
}

#[test]
fn a_turned_off_owner_tool_says_nothing_was_sent() {
    assert_eq!(
        HostPluginId::Owner.turned_off("ask_owner"),
        "ask_owner is turned off for this agent, so nothing was sent. Say it in your reply instead."
    );
    assert_eq!(
        HostPluginId::Owner.turned_off("share_file"),
        "share_file is turned off for this agent, so nothing changed. Tell the person in your reply that you cannot share files right now."
    );
}
