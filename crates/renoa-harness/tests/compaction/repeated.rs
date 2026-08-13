use std::{
    num::{NonZeroU32, NonZeroU64},
    sync::Arc,
};

use renoa_agent::ModelRequest;
use renoa_harness::{CompactionPolicy, ContextSizer, Harness, RuntimeProfile, SessionId};
use tempfile::tempdir;

use super::{CheckpointAwareModel, run_prompt};

#[tokio::test]
async fn one_operation_can_advance_through_multiple_bounded_checkpoints() {
    let directory = tempdir().expect("temporary directory");
    let model = Arc::new(CheckpointAwareModel::default());
    let plain = RuntimeProfile::new(
        "plain-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempt limit"),
    );
    let compacting = RuntimeProfile::new(
        "chunked-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempt limit"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(1).expect("one attempt for each checkpoint"),
        )
        .expect("valid compaction policy"),
        Arc::new(OneOperationPerChunk),
    );
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    run_prompt(&harness, session_id, &plain, "one").await;
    run_prompt(&harness, session_id, &plain, "two").await;
    run_prompt(&harness, session_id, &plain, "three").await;

    run_prompt(&harness, session_id, &compacting, "four").await;

    let requests = model.requests();
    let compactions = requests
        .iter()
        .filter(|request| request.system_prompt != "Be precise.")
        .collect::<Vec<_>>();
    assert_eq!(compactions.len(), 3);
    assert!(!encoded(compactions[0]).contains("previous_checkpoint"));
    assert!(encoded(compactions[1]).contains("previous_checkpoint"));
    assert!(encoded(compactions[2]).contains("previous_checkpoint"));
    let final_request = requests.last().expect("final normal request");
    assert_eq!(final_request.messages.len(), 2);
    assert!(encoded(final_request).contains("CONTEXT CHECKPOINT"));
    assert!(encoded(final_request).contains("four"));

    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.messages.len(), 8);
    assert_eq!(snapshot.operations[3].model_usage.attempts, 4);
}

struct OneOperationPerChunk;

impl ContextSizer for OneOperationPerChunk {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt != "Be precise." {
            let input = encoded(request);
            if input.matches("operation=").count() <= 2 {
                40
            } else {
                90
            }
        } else if request.messages.len() <= 2 {
            30
        } else {
            90
        }
    }
}

fn encoded(request: &ModelRequest) -> String {
    serde_json::to_string(request).expect("encode request")
}
