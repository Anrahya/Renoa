use std::sync::Arc;

use renoa_control::{ClientMessage, ErrorCode, NodeId, TargetSummary};
use renoa_kernel::AgentId;
use renoa_local::{AgentPeers, PeerAgent, PeerError, PeerMessage};
use renoa_protocol::TargetRef;
use uuid::Uuid;

use super::{AgentLink, Answer, Peers};
use crate::{agent_targets, bridge::NodeError};

fn summary(node: u128, target: &TargetRef, name: &str) -> TargetSummary {
    TargetSummary {
        node_id: NodeId::from_uuid(Uuid::from_u128(node)),
        target: target.clone(),
        name: name.to_owned(),
    }
}

fn message(route: &str) -> PeerMessage {
    PeerMessage {
        from_command: Uuid::from_u128(7),
        message_id: Uuid::from_u128(8),
        to: PeerAgent {
            name: "Research".to_owned(),
            route: route.to_owned(),
        },
        text: "Summarise.".to_owned(),
    }
}

fn research_route() -> String {
    format!("{}/agent:research", NodeId::from_uuid(Uuid::from_u128(2)))
}

#[tokio::test]
async fn a_request_with_no_session_fails_at_once_and_nothing_waits_for_the_next() {
    let link = Arc::new(AgentLink::default());
    let peers = Peers(Arc::clone(&link));
    let own = AgentId::new();

    let error = peers.roster(own).await.expect_err("no session is open");
    assert!(
        matches!(&error, PeerError::Unavailable(reason) if reason.contains("lost its connection")),
        "{error:?}"
    );
    let error = peers
        .send(message(&research_route()))
        .await
        .expect_err("no session is open");
    assert!(matches!(error, PeerError::Unavailable(_)), "{error:?}");

    // Neither failed request is waiting for the session that opens now.
    let mut session = link.open();
    let first = tokio::spawn(async move { Peers(link).roster(own).await });
    let ClientMessage::ListTargets { request_id } = session.next_message().await else {
        panic!("the first request of a session is the new one");
    };
    assert_eq!(request_id, 1);
    session
        .answer(request_id, Answer::Targets(Vec::new()))
        .expect("answer");
    assert_eq!(first.await.expect("task"), Ok(Vec::new()));
}

#[tokio::test]
async fn the_roster_leaves_out_the_asking_agent_and_is_ordered_by_name() {
    let link = Arc::new(AgentLink::default());
    let mut session = link.open();
    let own = AgentId::new();
    let asking = tokio::spawn({
        let link = Arc::clone(&link);
        async move { Peers(link).roster(own).await }
    });
    let ClientMessage::ListTargets { request_id } = session.next_message().await else {
        panic!("expected a roster request");
    };
    session
        .answer(
            request_id,
            Answer::Targets(vec![
                summary(1, &agent_targets::target(own), "Desk"),
                summary(2, &TargetRef::new("agent:research"), "Research"),
                summary(3, &TargetRef::new("agent:archive"), "Archive"),
            ]),
        )
        .expect("answer");
    let roster = asking.await.expect("task").expect("a roster");
    assert_eq!(
        roster
            .iter()
            .map(|agent| agent.name.as_str())
            .collect::<Vec<_>>(),
        ["Archive", "Research"]
    );
    assert_eq!(roster[1].route, research_route());
}

#[tokio::test]
async fn a_send_carries_its_identities_and_each_refusal_is_told_to_the_agent_as_it_is() {
    let cases = [
        (ErrorCode::NodeOffline, true, "machine is offline"),
        (ErrorCode::Internal, true, "problem delivering"),
        (ErrorCode::NotFound, false, "no such agent"),
        (ErrorCode::InvalidMessage, false, "no such agent"),
        (ErrorCode::Conflict, false, "no such agent"),
    ];
    for (code, retryable, text) in cases {
        let link = Arc::new(AgentLink::default());
        let mut session = link.open();
        let sending = tokio::spawn({
            let link = Arc::clone(&link);
            async move { Peers(link).send(message(&research_route())).await }
        });
        let ClientMessage::SendAgentMessage {
            request_id,
            from_command,
            message_id,
            node_id,
            target,
            text: sent,
        } = session.next_message().await
        else {
            panic!("expected a message");
        };
        assert_eq!(from_command.as_uuid(), Uuid::from_u128(7));
        assert_eq!(message_id.as_uuid(), Uuid::from_u128(8));
        assert_eq!(node_id, NodeId::from_uuid(Uuid::from_u128(2)));
        assert_eq!(target, TargetRef::new("agent:research"));
        assert_eq!(sent, "Summarise.");
        let reason = match code {
            ErrorCode::NodeOffline => "that agent's machine is offline right now",
            ErrorCode::Internal => "SQLITE_BUSY at /var/lib/renoa/control.sqlite",
            _ => "no such agent",
        };
        session
            .answer(
                request_id,
                Answer::Refused {
                    code,
                    message: reason.to_owned(),
                },
            )
            .expect("answer");
        let error = sending.await.expect("task").expect_err("refused");
        match (&error, retryable) {
            (PeerError::Unavailable(told), true) => {
                assert!(told.contains(text), "{told}");
                assert!(!told.contains("SQLITE"), "{told}");
            }
            (PeerError::Refused(told), false) => assert_eq!(told, reason),
            _ => panic!("{code:?} was reported as {error:?}"),
        }
    }
}

#[tokio::test]
async fn an_accepted_send_succeeds() {
    let link = Arc::new(AgentLink::default());
    let mut session = link.open();
    let sending = tokio::spawn({
        let link = Arc::clone(&link);
        async move { Peers(link).send(message(&research_route())).await }
    });
    let ClientMessage::SendAgentMessage { request_id, .. } = session.next_message().await else {
        panic!("expected a message");
    };
    session
        .answer(request_id, Answer::Accepted)
        .expect("answer");
    assert_eq!(sending.await.expect("task"), Ok(()));
}

#[tokio::test]
async fn a_session_that_ends_while_a_send_waits_leaves_its_outcome_unknown() {
    let link = Arc::new(AgentLink::default());
    let mut session = link.open();
    let sending = tokio::spawn({
        let link = Arc::clone(&link);
        async move { Peers(link).send(message(&research_route())).await }
    });
    let ClientMessage::SendAgentMessage { .. } = session.next_message().await else {
        panic!("expected a message");
    };
    drop(session);
    let error = sending.await.expect("task").expect_err("the session ended");
    assert!(
        matches!(&error, PeerError::Unconfirmed(reason) if reason.contains("not known whether")),
        "{error:?}"
    );

    // The link is closed until the next session opens.
    let error = Peers(link)
        .roster(AgentId::new())
        .await
        .expect_err("closed");
    assert!(matches!(error, PeerError::Unavailable(_)), "{error:?}");
}

#[tokio::test]
async fn an_older_session_ending_does_not_close_the_newer_one() {
    let link = Arc::new(AgentLink::default());
    let older = link.open();
    let mut newer = link.open();
    drop(older);
    let asking = tokio::spawn({
        let link = Arc::clone(&link);
        async move { Peers(link).roster(AgentId::new()).await }
    });
    let ClientMessage::ListTargets { request_id } = newer.next_message().await else {
        panic!("expected a roster request");
    };
    newer
        .answer(request_id, Answer::Targets(Vec::new()))
        .expect("answer");
    assert_eq!(asking.await.expect("task"), Ok(Vec::new()));
}

#[tokio::test]
async fn an_answer_to_a_request_the_session_never_made_is_a_protocol_error() {
    let link = Arc::new(AgentLink::default());
    let mut session = link.open();
    let error = session
        .answer(99, Answer::Accepted)
        .expect_err("nothing asked");
    assert!(matches!(error, NodeError::Protocol(_)), "{error:?}");
}

#[tokio::test]
async fn an_address_the_node_did_not_issue_is_refused_without_a_request() {
    let link = Arc::new(AgentLink::default());
    let _session = link.open();
    for route in ["no-slash", "not-a-uuid/agent:research"] {
        let error = Peers(Arc::clone(&link))
            .send(message(route))
            .await
            .expect_err("a foreign address");
        assert!(matches!(error, PeerError::Refused(_)), "{error:?}");
    }
}
