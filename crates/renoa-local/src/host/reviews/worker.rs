//! Worker entry, deadline and durable error handoff to the supervisor.
use super::{GitHubReviewError, GitHubReviewOutcome, GitHubReviewRun, catalog, runs};
use crate::{InspectionSandboxConfig, LocalHost, LocalHostError, TurnObservation};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

impl LocalHost {
    pub(super) async fn execute_review_worker(
        &self,
        request_id: Uuid,
        app_jwt: &str,
        workspace: Option<&InspectionSandboxConfig>,
        cancellation: CancellationToken,
        origin: Url,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        let deadline = self
            .begin_github_review(request_id, TurnObservation::now()?.unix_milliseconds())
            .await?;
        let remaining = deadline.saturating_sub(TurnObservation::now()?.unix_milliseconds());
        if remaining <= 0 {
            self.reap_github_review(request_id, TurnObservation::now()?.unix_milliseconds())
                .await?;
            let path = self.config.database.clone();
            return tokio::task::spawn_blocking(move || {
                runs::get(&catalog::open_verified(&path)?, request_id)?
                    .ok_or_else(|| GitHubReviewError::NotFound.into())
            })
            .await?;
        }
        self.start_github_review(request_id, TurnObservation::now()?.unix_milliseconds())
            .await?;
        let run =
            self.execute_review_in(request_id, app_jwt, cancellation.clone(), origin, workspace);
        tokio::pin!(run);
        let result = tokio::select! {
            result = &mut run => result,
            () = tokio::time::sleep(std::time::Duration::from_millis(remaining.unsigned_abs())) => {
                cancellation.cancel();
                // Drain the active effect and its children. The systemd service
                // independently kills the cgroup if cooperative shutdown hangs.
                run.await
            }
        };
        if let Err(error) = &result {
            self.record_review_worker_error(
                request_id,
                error,
                TurnObservation::now()?.unix_milliseconds(),
            )
            .await?;
        }
        result
    }

    async fn record_review_worker_error(
        &self,
        id: Uuid,
        error: &LocalHostError,
        now: i64,
    ) -> Result<(), LocalHostError> {
        let retry = retry_at(error, now);
        let reason = format!("Review worker failed: {error}");
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            // The execution future has released its lease. Never overwrite an
            // active owner's state or replace an already committed outcome.
            let _lease = crate::host::lease::ExecutionLease::acquire(&path.with_file_name(".reviews.lock"))?;
            let db = catalog::open_verified(&path)?;
            let snapshot = match runs::get(&db, id)? {
                Some(GitHubReviewRun::Finished { .. }) => return Ok(()),
                Some(GitHubReviewRun::Prepared { snapshot }) => Some(snapshot),
                None => None,
            };
            let deadline: i64 = db.query_row("SELECT deadline_at_ms FROM host_review_jobs WHERE request_id=?1", [id.to_string()], |row| row.get(0)).map_err(GitHubReviewError::from)?;
            if let Some(until) = retry.filter(|_| now < deadline) {
                // Clearing entry marks this attempt as handed back to dispatch;
                // the original deadline and specific failure survive ExecStopPost.
                db.execute("UPDATE host_review_jobs SET started_at_ms=NULL,retry_after_ms=?2,last_error=?3 WHERE request_id=?1",
                    rusqlite::params![id.to_string(),until.min(deadline),reason]).map_err(GitHubReviewError::from)?;
            } else {
                runs::save(&path, &GitHubReviewRun::Finished { request_id:id,snapshot,outcome:GitHubReviewOutcome::Incomplete {reason} })?;
            }
            Ok::<_,LocalHostError>(())
        }).await?
    }
}

fn retry_at(error: &LocalHostError, now: i64) -> Option<i64> {
    let LocalHostError::GitHubReview(error) = error else {
        return None;
    };
    let hint = match error {
        GitHubReviewError::Api {
            status,
            retry_after,
        } if matches!(status, 408 | 429 | 500..=599)
            || (*status == 403 && retry_after.is_some()) =>
        {
            retry_after.as_deref()
        }
        GitHubReviewError::Http(source)
            if source.is_request() || source.is_timeout() || source.is_body() =>
        {
            None
        }
        GitHubReviewError::MovingPull | GitHubReviewError::Cancelled => None,
        _ => return None,
    };
    let requested = hint.and_then(|value| {
        value
            .parse::<i64>()
            .ok()
            .filter(|seconds| *seconds >= 0)
            .map(|seconds| now.saturating_add(seconds.saturating_mul(1000)))
            .or_else(|| {
                jiff::fmt::rfc2822::parse(value)
                    .ok()
                    .map(|date| date.timestamp().as_millisecond())
            })
    });
    Some(requested.unwrap_or(0).max(now.saturating_add(60_000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_recoverable_errors_retry_and_github_backoff_is_honored() {
        for (status, hint, expected) in [
            (429, Some("120"), Some(121_000)),
            (403, Some("120"), Some(121_000)),
            (503, None, Some(61_000)),
            (408, None, Some(61_000)),
            (429, Some("Thu, 01 Jan 1970 00:03:00 GMT"), Some(180_000)),
            (429, Some("-1"), Some(61_000)),
            (403, None, None),
            (401, Some("60"), None),
            (422, None, None),
        ] {
            let error = GitHubReviewError::Api {
                status,
                retry_after: hint.map(str::to_owned),
            }
            .into();
            assert_eq!(
                retry_at(&error, 1_000),
                expected,
                "status {status}, hint {hint:?}"
            );
        }
        assert_eq!(
            retry_at(
                &LocalHostError::Configuration("invalid model".into()),
                1_000
            ),
            None
        );
        assert_eq!(
            retry_at(&GitHubReviewError::Cancelled.into(), 1_000),
            Some(61_000)
        );
    }
}
