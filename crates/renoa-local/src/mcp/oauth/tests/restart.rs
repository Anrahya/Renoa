use super::*;

#[tokio::test]
async fn replay_of_a_waiting_restart_keeps_its_original_callback_and_state() {
    let mut fixture = Fixture::new();
    let cancellation = CancellationToken::new();
    let task = {
        let resolver = fixture.resolver.clone();
        let auth = fixture.auth.clone();
        let token = cancellation.clone();
        tokio::spawn(async move {
            resolver
                .authorize(
                    authorization_request(&auth, "restart-operation", true),
                    token,
                )
                .await
        })
    };
    wait_for_phase(&fixture, OAuthPhase::AwaitingCallback).await;
    let original = fixture.secret_bundle().await;
    cancellation.cancel();
    assert!(task.await.expect("joined").is_err());
    fixture.enable_callback_browser();
    fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, "restart-operation", true),
            CancellationToken::new(),
        )
        .await
        .expect("resume same restart");
    let completed = fixture.secret_bundle().await;
    assert_eq!(
        original.adapter_state["csrf_state"],
        completed.adapter_state["csrf_state"]
    );
    assert_eq!(
        original.adapter_state["redirect_uri"],
        completed.adapter_state["redirect_uri"]
    );
    assert_eq!(fixture.action_count("oauth_begin"), 1);
    assert_eq!(fixture.action_count("oauth_exchange"), 1);
}
