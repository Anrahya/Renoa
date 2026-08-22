use serde_json::Value;

pub(crate) fn assert_equivalent_prompt_outcomes(
    first: &(Value, Value),
    replay: &(Value, Value),
    request_id: &str,
) {
    assert_eq!(
        first.0["params"]["update"]["content"],
        replay.0["params"]["update"]["content"]
    );
    assert_eq!(first.1, replay.1);
    for update in [&first.0, &replay.0] {
        let message_id = update["params"]["update"]["messageId"]
            .as_str()
            .expect("assistant message id");
        uuid::Uuid::parse_str(message_id).expect("assistant message id is a UUID");
        assert_ne!(message_id, request_id);
    }
}
