use ring::hmac;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{GitHubReviewError, GitHubReviewSkip, GitHubReviewTrigger, validate_target};

/// Transport headers and the exact bytes received, before any JSON re-encoding.
#[derive(Clone, Copy)]
pub struct GitHubReviewWebhook<'a> {
    pub delivery_id: Uuid,
    pub event: &'a str,
    pub signature: &'a str,
    pub body: &'a [u8],
}

pub(super) struct Delivery {
    pub id: Uuid,
    pub digest: Vec<u8>,
    pub event: Event,
}

pub(super) enum Event {
    PullRequest(PullRequestEvent),
    Ignored(GitHubReviewSkip),
}

#[derive(Deserialize)]
pub(super) struct PullRequestEvent {
    pub action: GitHubReviewTrigger,
    pub installation: Installation,
    pub repository: Repository,
    pub pull_request: PullRequest,
}
#[derive(Deserialize)]
pub(super) struct Installation {
    pub id: i64,
}
#[derive(Deserialize)]
pub(super) struct Repository {
    pub id: i64,
}
#[derive(Deserialize)]
pub(super) struct PullRequest {
    pub number: i64,
    pub draft: bool,
    pub state: String,
    pub base: Commit,
    pub head: Commit,
}
#[derive(Deserialize)]
pub(super) struct Commit {
    pub sha: String,
}

pub(super) fn authenticate(
    input: GitHubReviewWebhook<'_>,
    secret: &[u8],
) -> Result<Delivery, GitHubReviewError> {
    if input.body.len() > 1024 * 1024 || input.event.len() > 64 || input.delivery_id.is_nil() {
        return Err(GitHubReviewError::Invalid(
            "webhook exceeds admission limits or lacks a delivery ID".to_owned(),
        ));
    }
    let signature = input
        .signature
        .strip_prefix("sha256=")
        .ok_or(GitHubReviewError::Authentication)?;
    if signature.len() != 64 || secret.is_empty() || secret.len() > 4096 {
        return Err(GitHubReviewError::Authentication);
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in signature.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte: u8| {
            char::from(byte)
                .to_digit(16)
                .and_then(|n| u8::try_from(n).ok())
        };
        bytes[index] = digit(pair[0]).ok_or(GitHubReviewError::Authentication)? * 16
            + digit(pair[1]).ok_or(GitHubReviewError::Authentication)?;
    }
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, secret),
        input.body,
        &bytes,
    )
    .map_err(|_| GitHubReviewError::Authentication)?;
    let event = if input.event == "pull_request" {
        #[derive(Deserialize)]
        struct Action {
            action: String,
        }
        let action: Action = serde_json::from_slice(input.body)?;
        if matches!(
            action.action.as_str(),
            "opened" | "reopened" | "ready_for_review" | "synchronize"
        ) {
            let event: PullRequestEvent = serde_json::from_slice(input.body)?;
            validate_target(
                event.pull_request.number,
                &event.pull_request.base.sha,
                &event.pull_request.head.sha,
            )?;
            if event.installation.id <= 0
                || event.repository.id <= 0
                || !matches!(event.pull_request.state.as_str(), "open" | "closed")
            {
                return Err(GitHubReviewError::Invalid(
                    "invalid GitHub repository, installation or PR state".to_owned(),
                ));
            }
            Event::PullRequest(event)
        } else {
            Event::Ignored(GitHubReviewSkip::UnsupportedEvent)
        }
    } else {
        Event::Ignored(GitHubReviewSkip::UnsupportedEvent)
    };
    // Include the event header in the receipt identity: GitHub's HMAC covers only
    // the body, and retrying a delivery with changed headers is a conflict.
    let mut digest = Sha256::new();
    digest.update(input.event.as_bytes());
    digest.update([0]);
    digest.update(input.body);
    Ok(Delivery {
        id: input.delivery_id,
        digest: digest.finalize().to_vec(),
        event,
    })
}
