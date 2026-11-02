//! Claims and the opening of a session's shared instance, with no session behind them.

use std::{sync::Arc, time::Duration};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{Claims, SURFACE_WAIT, Waited};
use crate::{AgentSession, LocalHostError};

fn none() -> CancellationToken {
    CancellationToken::new()
}

#[tokio::test]
async fn a_surface_cancels_the_pass_and_waits_for_it_and_a_pass_never_takes_a_held_session() {
    let claims = Claims::default();
    let session = Uuid::new_v4();
    let pass = CancellationToken::new();
    let held = claims.note_pass(session, &pass).expect("a free session");
    assert!(
        claims.note_pass(session, &none()).is_none(),
        "one pass at a time"
    );
    let waiting = none();
    let surface = claims.surface(session, &waiting);
    tokio::pin!(surface);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), surface.as_mut())
            .await
            .is_err(),
        "the surface waits while the pass holds the session"
    );
    assert!(pass.is_cancelled(), "and has cancelled it");
    drop(held);
    let first = surface.await.expect("the pass let go");
    let second = claims
        .surface(session, &none())
        .await
        .expect("surfaces share");
    assert!(
        claims.note_pass(session, &none()).is_none(),
        "a pass never takes a session a surface holds"
    );
    drop(first);
    assert!(claims.note_pass(session, &none()).is_none());
    drop(second);
    assert!(
        claims.note_pass(session, &none()).is_some(),
        "free once every surface let go"
    );
}

#[tokio::test(start_paused = true)]
async fn a_surface_gives_up_on_a_pass_that_does_not_let_go_and_claims_nothing() {
    let claims = Claims::default();
    let session = Uuid::new_v4();
    let pass = CancellationToken::new();
    let held = claims.note_pass(session, &pass).expect("a free session");
    let started = tokio::time::Instant::now();
    let refused = tokio::time::timeout(SURFACE_WAIT * 2, claims.surface(session, &none()))
        .await
        .expect("the surface gives up by itself");
    assert!(pass.is_cancelled());
    assert_eq!(refused.err(), Some(Waited::TimedOut));
    assert_eq!(started.elapsed(), SURFACE_WAIT);
    assert!(matches!(
        LocalHostError::from(Waited::TimedOut),
        LocalHostError::PassStillRunning
    ));
    drop(held);
    assert!(
        claims.note_pass(session, &none()).is_some(),
        "the refused surface holds nothing"
    );
}

#[tokio::test(start_paused = true)]
async fn a_surface_cancelled_while_it_waits_stops_waiting_and_claims_nothing() {
    let claims = Claims::default();
    let session = Uuid::new_v4();
    let held = claims.note_pass(session, &none()).expect("a free session");
    let waiting = none();
    let surface = claims.surface(session, &waiting);
    tokio::pin!(surface);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), surface.as_mut())
            .await
            .is_err()
    );
    waiting.cancel();
    let started = tokio::time::Instant::now();
    assert_eq!(surface.await.err(), Some(Waited::Cancelled));
    assert_eq!(started.elapsed(), Duration::ZERO, "at once");
    drop(held);
    assert!(claims.note_pass(session, &none()).is_some());
}

#[tokio::test(start_paused = true)]
async fn a_surface_opening_a_session_a_pass_is_loading_cancels_the_pass_first() {
    let claims = Arc::new(Claims::default());
    let session = Uuid::new_v4();
    let pass = CancellationToken::new();
    let held = claims.note_pass(session, &pass).expect("a free session");
    // The pass is loading the session: it holds the slot until cancelled.
    let loading = tokio::spawn({
        let (claims, pass) = (Arc::clone(&claims), pass.clone());
        async move {
            claims
                .open(session, async move {
                    pass.cancelled().await;
                    Err::<Arc<AgentSession>, _>(LocalHostError::InvalidRequest(
                        "the pass's load".to_owned(),
                    ))
                })
                .await
                .err()
        }
    });
    tokio::task::yield_now().await;
    let opened = tokio::time::timeout(
        SURFACE_WAIT / 2,
        claims.open_for_surface(session, async {
            Err::<Arc<AgentSession>, _>(LocalHostError::InvalidRequest(
                "the surface's load".to_owned(),
            ))
        }),
    )
    .await
    .expect("the surface does not wait out the pass's load");
    assert!(pass.is_cancelled());
    assert!(
        opened
            .err()
            .is_some_and(|error| error.to_string().contains("the surface's load"))
    );
    assert!(loading.await.expect("the pass's load").is_some());
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn a_surface_waits_for_a_load_no_longer_than_the_bound() {
    let claims = Arc::new(Claims::default());
    let session = Uuid::new_v4();
    let _loading = tokio::spawn({
        let claims = Arc::clone(&claims);
        async move {
            claims
                .open(
                    session,
                    std::future::pending::<Result<Arc<AgentSession>, _>>(),
                )
                .await
        }
    });
    tokio::task::yield_now().await;
    let opened = tokio::time::timeout(
        SURFACE_WAIT * 2,
        claims.open_for_surface(session, std::future::pending()),
    )
    .await
    .expect("the surface gives up by itself");
    assert!(matches!(opened, Err(LocalHostError::PassStillRunning)));
}

#[tokio::test]
async fn stopping_passes_cancels_the_running_one_and_refuses_the_next() {
    let claims = Claims::default();
    let (running, idle) = (Uuid::new_v4(), Uuid::new_v4());
    let pass = CancellationToken::new();
    let held = claims.note_pass(running, &pass).expect("a free session");
    claims.stop_passes();
    assert!(pass.is_cancelled());
    assert!(claims.passes_stopped());
    assert!(
        claims.note_pass(idle, &none()).is_none(),
        "no pass starts once stopped"
    );
    drop(held);
    assert!(
        claims.surface(running, &none()).await.is_ok(),
        "surfaces go on"
    );
}
