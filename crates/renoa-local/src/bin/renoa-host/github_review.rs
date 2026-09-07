use std::{
    error::Error,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
};

use renoa_local::{
    GitHubReviewCommand, GitHubReviewWebhook, LocalHost, TurnObservation,
    credential_file_is_private,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    delivery_id: Uuid,
    event: String,
    signature: String,
    body_file: PathBuf,
    secret_file: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Execution {
    request_id: Uuid,
    app_jwt_file: PathBuf,
}

pub async fn run(
    host: &LocalHost,
    mode: &std::ffi::OsStr,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let path = path.to_owned();
    let input = tokio::task::spawn_blocking(move || read_bounded(&path, 64 * 1024)).await??;
    let now = TurnObservation::now()?.unix_milliseconds();
    if mode == "github-review" {
        let command: GitHubReviewCommand = serde_json::from_slice(&input)?;
        let reply = host
            .manage_github_review(command, now, CancellationToken::new())
            .await?;
        println!("{}", serde_json::to_string(&reply)?);
    } else if mode == "github-execute" {
        let execution: Execution = serde_json::from_slice(&input)?;
        let jwt = tokio::task::spawn_blocking(move || {
            private_credential(&execution.app_jwt_file, 16 * 1024)
        })
        .await??;
        let jwt = String::from_utf8(jwt)?;
        let cancel = CancellationToken::new();
        let run = host.execute_github_review(execution.request_id, jwt.trim(), cancel.clone());
        tokio::pin!(run);
        let result = tokio::select! {
            result=&mut run=>result?,
            signal=tokio::signal::ctrl_c()=>{ signal?; cancel.cancel(); run.await? }
        };
        println!("{}", serde_json::to_string(&result)?);
    } else {
        let envelope: Envelope = serde_json::from_slice(&input)?;
        let body_path = envelope.body_file;
        let secret_path = envelope.secret_file;
        let (body, secret) = tokio::task::spawn_blocking(move || {
            if !body_path.is_absolute() || !secret_path.is_absolute() {
                return Err(std::io::Error::other(
                    "webhook body and secret paths must be absolute",
                ));
            }
            let metadata = std::fs::symlink_metadata(&secret_path)?;
            let credential_directory = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
            if !credential_file_is_private(&secret_path, &metadata, credential_directory.as_deref())
            {
                return Err(std::io::Error::other("webhook secret file must be private"));
            }
            Ok((
                read_bounded(&body_path, 1024 * 1024)?,
                read_bounded(&secret_path, 4096)?,
            ))
        })
        .await??;
        let reply = host
            .admit_github_review_webhook(
                GitHubReviewWebhook {
                    delivery_id: envelope.delivery_id,
                    event: &envelope.event,
                    signature: &envelope.signature,
                    body: &body,
                },
                &secret,
                now,
                CancellationToken::new(),
            )
            .await?;
        println!("{}", serde_json::to_string(&reply)?);
    }
    Ok(())
}

fn private_credential(path: &Path, limit: u64) -> Result<Vec<u8>, std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
    if !path.is_absolute() || !credential_file_is_private(path, &metadata, directory.as_deref()) {
        return Err(std::io::Error::other(
            "GitHub App JWT must be in a private absolute credential file",
        ));
    }
    read_bounded(path, limit)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_err(std::io::Error::other)? > limit {
        return Err(std::io::Error::other(
            "GitHub review input file exceeds its size limit",
        ));
    }
    Ok(bytes)
}
