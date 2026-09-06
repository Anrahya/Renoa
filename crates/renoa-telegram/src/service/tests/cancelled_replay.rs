use super::cancellation::admit_cancel;
use super::*;
use crate::store::WorkKind;

#[tokio::test]
async fn cancelled_redelivery_replays_settled_work_and_rejects_changed_content_without_a_bridge() {
    for cached in [false, true] {
        for mismatch in [false, true] {
            let mut fixture = service_fixture().await;
            let mut work = admit_work(
                &fixture.store,
                1,
                InboundKind::Prompt("Original".to_owned()),
            )
            .await;
            // Simulate loss after kernel settlement, before the surface result commit.
            assert_eq!(
                fixture
                    .worker
                    .run_agent(&work, Some("Original"))
                    .await
                    .expect("settle kernel"),
                "Arcee completed the real path."
            );
            admit_cancel(&fixture.store, &work).await;
            if !cached {
                fixture.worker.sessions.clear();
            }
            fs::remove_file(fixture.directory.path().join("model-bridge.mjs"))
                .expect("remove execution dependency");
            fs::remove_file(fixture.directory.path().join("stream-called"))
                .expect("reset model marker");
            if mismatch {
                work.kind = WorkKind::Prompt("Changed".to_owned());
            }
            fixture
                .worker
                .execute(work)
                .await
                .expect("settle surface delivery");
            let result = ready_delivery(&fixture.store).await.text;
            if mismatch {
                assert!(result.contains("different content"), "{result}");
            } else {
                assert_eq!(result, "Arcee completed the real path.");
            }
            assert!(!fixture.directory.path().join("stream-called").exists());
            fixture.shutdown().await;
        }
    }
}
