use super::{Settings, auth::AppAuth};
use renoa_local::{
    GitHubReviewPublication, GitHubReviewWork, LocalHost, LocalHostError, TurnObservation,
};
use std::{error::Error, path::Path, time::Duration};
use tokio::{io::AsyncWriteExt as _, process::Command};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(test)]
#[path = "dispatcher_tests.rs"]
mod tests;

pub(super) async fn run(
    host: &LocalHost,
    data: &Path,
    config: &Settings,
    auth: &AppAuth,
    stop: &CancellationToken,
) -> Result<(), Box<dyn Error>> {
    loop {
        if stop.is_cancelled() {
            return Ok(());
        }
        if let Err(error) = tick(host, data, config, auth, stop).await {
            eprintln!("GitHub dispatcher: {error}");
        }
        tokio::select! { ()=stop.cancelled()=>return Ok(()), ()=tokio::time::sleep(Duration::from_secs(15))=>{} }
    }
}

async fn tick(
    host: &LocalHost,
    data: &Path,
    config: &Settings,
    auth: &AppAuth,
    stop: &CancellationToken,
) -> Result<(), Box<dyn Error>> {
    let work = host.github_review_work().await?;
    // Inspect every known unit before dispatching. A service restart must not
    // create a competing owner, and systemd failure is not evidence of inactivity.
    let workers = observe_workers(work, stop, unit_active).await;
    if stop.is_cancelled() {
        return Ok(());
    }
    let mut waiting = Vec::new();
    let can_launch = !workers.active && !workers.uncertain;
    for job in workers.stopped {
        let observed = now()?;
        if job.retry_after_ms > observed {
            continue;
        }
        if !job.finished
            && job.started_at_ms.is_none()
            && job
                .deadline_at_ms
                .is_none_or(|deadline| deadline > observed)
        {
            waiting.push(job.request_id);
            continue;
        }
        if job.deadline_at_ms.is_some() {
            // An older finished review may await publication while another
            // worker owns the global checkout lease. Do not disturb that owner.
            if workers.active {
                continue;
            }
            if !cleanup_stopped(host, data, job.request_id, observed).await? {
                continue;
            }
            if job.publish_after_ms > now()? {
                continue;
            }
            match host
                .publish_github_review(
                    job.request_id,
                    &auth.jwt()?,
                    &config.bot_login,
                    stop.clone(),
                )
                .await
            {
                Ok(state) => match state {
                    GitHubReviewPublication::Published { review_id, url } => {
                        eprintln!("Review {} published as {review_id}: {url}", job.request_id);
                    }
                    other => eprintln!("Review {} publication: {other:?}", job.request_id),
                },
                Err(error) => {
                    let now = now()?;
                    let retry = if let LocalHostError::GitHubReview(ref source) = error {
                        super::recovery::retry_at(source, now)
                    } else {
                        now.saturating_add(300_000)
                    };
                    host.defer_github_publication(job.request_id, retry).await?;
                    eprintln!(
                        "Review {} publication failed: {error}; retry at {retry}",
                        job.request_id
                    );
                }
            }
        } else {
            waiting.push(job.request_id);
        }
    }
    if can_launch
        && let Some(id) = waiting.first()
        && let Err(error) = launch(host, data, config, auth, *id).await
    {
        // A timed-out dispatch may still have started. The next tick queries
        // the stable unit before deciding whether this job is retryable.
        host.defer_github_review(*id, now()?.saturating_add(60_000), error.to_string())
            .await?;
        eprintln!("Review {id} launch deferred: {error}");
    }
    Ok(())
}

/// Unknown units cannot authorize launches or cleanup of their own work. Other
/// confirmed-stopped units remain eligible for cleanup under the checkout lease.
#[derive(Default)]
struct WorkerObservations {
    active: bool,
    uncertain: bool,
    stopped: Vec<GitHubReviewWork>,
}

async fn observe_workers<F, Fut>(
    work: Vec<GitHubReviewWork>,
    stop: &CancellationToken,
    mut query: F,
) -> WorkerObservations
where
    F: FnMut(Uuid) -> Fut,
    Fut: std::future::Future<Output = Result<bool, Box<dyn Error>>>,
{
    let mut result = WorkerObservations::default();
    for job in work {
        if stop.is_cancelled() {
            result.uncertain = true;
            break;
        }
        match query(job.request_id).await {
            Ok(true) => result.active = true,
            Ok(false) => result.stopped.push(job),
            Err(error) => {
                result.uncertain = true;
                eprintln!("Review {} unit state unavailable: {error}", job.request_id);
            }
        }
    }
    result
}

/// False defers only this job. Uncertain ownership or catalog failure remains
/// an error so the caller cannot dispatch another worker on that evidence.
async fn cleanup_stopped(
    host: &LocalHost,
    data: &Path,
    id: Uuid,
    observed: i64,
) -> Result<bool, Box<dyn Error>> {
    let result = match host.reap_github_review(id, observed).await {
        Ok(()) => remove_launch(data, id).await.map_err(LocalHostError::from),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => Ok(true),
        Err(LocalHostError::Io(error)) if error.kind() != std::io::ErrorKind::WouldBlock => {
            host.defer_github_review(id, observed.saturating_add(60_000), error.to_string())
                .await?;
            eprintln!("Review {id} cleanup deferred: {error}");
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

async fn unit_active(id: Uuid) -> Result<bool, Box<dyn Error>> {
    let output = bounded_command(Command::new("/usr/bin/systemctl").args([
        "--user",
        "show",
        "--property=LoadState,ActiveState",
        &unit(id),
    ]))
    .await?;
    if !output.status.success() {
        return Err("cannot query the systemd user manager; refusing to infer worker death".into());
    }
    let output = String::from_utf8(output.stdout)?;
    let state = output.lines().find_map(|l| l.strip_prefix("ActiveState="));
    match state {
        Some("active" | "activating" | "deactivating" | "reloading") => Ok(true),
        Some("inactive" | "failed") => Ok(false),
        _ => Err("systemd returned an unknown review worker state".into()),
    }
}

fn unit(id: Uuid) -> String {
    format!("renoa-review-{id}.service")
}
fn now() -> Result<i64, Box<dyn Error>> {
    Ok(TurnObservation::now()?.unix_milliseconds())
}

async fn launch(
    host: &LocalHost,
    data: &Path,
    config: &Settings,
    auth: &AppAuth,
    id: Uuid,
) -> Result<(), Box<dyn Error>> {
    let deadline = host.begin_github_review(id, now()?).await?;
    let remaining = deadline.saturating_sub(now()?);
    if remaining <= 0 {
        host.reap_github_review(id, now()?).await?;
        return Ok(());
    }
    // The stable unit was confirmed stopped before launch. A previous failed
    // dispatch may have left create_new credential files behind; replace them
    // only after that ownership check, and retain the original job deadline.
    remove_launch(data, id).await?;
    let root = data.join("github-executions").join(id.to_string());
    tokio::fs::create_dir_all(&root).await?;
    let jwt = root.join("app.jwt");
    private_write(&jwt, auth.jwt()?.as_bytes()).await?;
    let execution = root.join("execution.json");
    private_write(
        &execution,
        &serde_json::to_vec(&serde_json::json!({
            "request_id":id, "app_jwt_file":jwt, "workspace":config.workspace
        }))?,
    )
    .await?;
    let binary = std::env::current_exe()?;
    let cleanup = format!(
        "ExecStopPost={} {} github-cleanup {id}",
        binary.display(),
        config.host_config.display()
    );
    let output = bounded_command(
        Command::new("/usr/bin/systemd-run")
            .args([
                "--user",
                "--collect",
                "--quiet",
                "--unit",
                &unit(id),
                "--property=Type=exec",
                "--property=KillMode=control-group",
                "--property=TimeoutStopSec=30s",
                "--property=TimeoutStartSec=30s",
                "--property=UMask=0077",
                "--property=StandardOutput=null",
                "--property=StandardError=journal",
                "--property=MemoryAccounting=yes",
                "--property=CPUAccounting=yes",
            ])
            .arg(format!("--property=RuntimeMaxSec={remaining}ms"))
            .arg(format!("--property={cleanup}"))
            .arg(&binary)
            .arg(&config.host_config)
            .arg("github-execute")
            .arg(execution),
    )
    .await?;
    if !output.status.success() {
        return Err("systemd rejected review worker dispatch".into());
    }
    eprintln!("Review {id} dispatched; absolute deadline {deadline}");
    Ok(())
}

async fn private_write(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await
}

async fn remove_launch(data: &Path, id: Uuid) -> Result<(), std::io::Error> {
    match tokio::fs::remove_dir_all(data.join("github-executions").join(id.to_string())).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

async fn bounded_command(command: &mut Command) -> Result<std::process::Output, Box<dyn Error>> {
    command.kill_on_drop(true);
    Ok(tokio::time::timeout(Duration::from_secs(15), command.output()).await??)
}
