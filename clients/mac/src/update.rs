//! Updating the installed app to the newest release renoa.live serves:
//! `scripts/publish-mac-app` attaches it to a release, `renoa-deploy` places
//! it at `/app/mac.json` and the zip that file names, and this finds it,
//! checks it, puts it in place of the running Renoa.app and relaunches.
//!
//! Only a published build updates: one built with its release's version in
//! `RENOA_VERSION`, running from a Renoa.app bundle. `cargo run` and
//! `scripts/install-app.sh` builds do not.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
};

use ring::digest;
use serde::Deserialize;

pub mod updater;

pub use updater::{Updater, Updates};

/// The release this build was published as.
pub const VERSION: Option<&str> = option_env!("RENOA_VERSION");

/// The largest description read: four short fields.
const MAX_DESCRIPTION_BYTES: usize = 4096;

/// The largest app a description may name: eight times the 33 MB the
/// unzipped release binary was when this was written.
const MAX_APP_BYTES: u64 = 256 * 1024 * 1024;

/// The bundle identifier `scripts/bundle-app.sh` gives the app.
const BUNDLE_ID: &str = "live.renoa.mac";

/// The newest Mac app renoa.live serves, as `renoa-deploy` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub version_name: String,
    pub zip: String,
    pub sha256: String,
    pub size: u64,
}

/// A release version: MAJOR.MINOR.PATCH, each one to three digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u16, u16, u16);

fn version(text: &str) -> Option<Version> {
    let mut parts = text.split('.').map(|part| {
        (1..=3)
            .contains(&part.len())
            .then_some(part)
            .filter(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|part| part.parse().ok())
    });
    let parsed = Version(parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

impl Release {
    /// The release, when it is newer than `running` and describes a file
    /// `renoa-deploy` would have placed; an error when it is not well formed.
    fn newer_than(self, running: &str) -> Result<Option<Self>, String> {
        let offered = version(&self.version_name).ok_or_else(|| {
            format!(
                "the update's version {:?} is not a release",
                self.version_name
            )
        })?;
        let running = version(running)
            .ok_or_else(|| format!("this build's version {running:?} is not a release"))?;
        if self.zip != format!("renoa-mac-{}.zip", self.version_name) {
            return Err(format!("the update is named {:?}", self.zip));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("the update's digest is not a SHA-256".to_owned());
        }
        if self.size == 0 || self.size > MAX_APP_BYTES {
            return Err(format!("the update's size {} is out of range", self.size));
        }
        Ok((offered > running).then_some(self))
    }
}

/// Where `origin` describes its newest Mac app.
pub fn description_url(origin: &str) -> String {
    format!("{}/app/mac.json", origin.trim_end_matches('/'))
}

/// The release at `url` when it is newer than `running`; none when it is not,
/// or the server serves no Mac app.
pub async fn newer(
    client: &reqwest::Client,
    url: &str,
    running: &str,
) -> Result<Option<Release>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("checking for an update failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "checking for an update failed with {}",
            response.status()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("reading the update failed: {error}"))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_DESCRIPTION_BYTES {
            return Err("the update's description is too long".to_owned());
        }
    }
    let release: Release = serde_json::from_slice(&body)
        .map_err(|error| format!("the update's description is malformed: {error}"))?;
    release.newer_than(running)
}

/// The Renoa.app this process runs from, if it runs from one.
pub fn running_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let bundle = executable.parent()?.parent()?.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// Downloads `release` from beside the description at `url`, checks it, and
/// puts it in place of `bundle`. Everything is checked before the swap, and
/// any failure removes what was downloaded and leaves `bundle` as it was.
pub async fn install(
    client: &reqwest::Client,
    url: &str,
    release: &Release,
    bundle: &Path,
) -> Result<(), String> {
    let parent = bundle.parent().ok_or("the app has no folder")?;
    // Beside the bundle, so the swap is two renames on one volume.
    let staging = parent.join(format!(".renoa-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)
        .map_err(|error| format!("can't prepare the update in {}: {error}", parent.display()))?;
    let result = stage_and_swap(client, url, release, bundle, &staging).await;
    if !matches!(result, Err(Swap::Stranded(_))) {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result.map_err(|failure| match failure {
        Swap::Refused(reason) => reason,
        Swap::Stranded(reason) => reason,
    })
}

enum Swap {
    /// Nothing changed.
    Refused(String),
    /// The app was moved aside and could not be put back; the staging folder
    /// holds it.
    Stranded(String),
}

async fn stage_and_swap(
    client: &reqwest::Client,
    url: &str,
    release: &Release,
    bundle: &Path,
    staging: &Path,
) -> Result<(), Swap> {
    let zip = staging.join(&release.zip);
    download(client, &beside(url, &release.zip), release, &zip)
        .await
        .map_err(Swap::Refused)?;
    let unpacked = staging.join("unpacked");
    run(Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(&zip)
        .arg(&unpacked))
    .map_err(Swap::Refused)?;
    let staged = unpacked.join("Renoa.app");
    check_bundle(&staged, &release.version_name, bundle).map_err(Swap::Refused)?;
    let aside = staging.join("previous.app");
    std::fs::rename(bundle, &aside)
        .map_err(|error| Swap::Refused(format!("can't move the app aside: {error}")))?;
    if let Err(error) = std::fs::rename(&staged, bundle) {
        return match std::fs::rename(&aside, bundle) {
            Ok(()) => Err(Swap::Refused(format!(
                "can't put the update in place: {error}"
            ))),
            Err(_) => Err(Swap::Stranded(format!(
                "can't put the update in place ({error}); the previous app is at {}",
                aside.display()
            ))),
        };
    }
    Ok(())
}

/// The URL of `name` beside the description at `url`.
fn beside(url: &str, name: &str) -> String {
    let folder = url.rsplit_once('/').map_or(url, |(folder, _)| folder);
    format!("{folder}/{name}")
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    release: &Release,
    into: &Path,
) -> Result<(), String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("downloading the update failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "downloading the update failed with {}",
            response.status()
        ));
    }
    let mut file = std::fs::File::create_new(into)
        .map_err(|error| format!("can't save the update: {error}"))?;
    let mut digested = digest::Context::new(&digest::SHA256);
    let mut written: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("downloading the update failed: {error}"))?
    {
        written += chunk.len() as u64;
        if written > release.size {
            return Err("the update is larger than described".to_owned());
        }
        digested.update(&chunk);
        file.write_all(&chunk)
            .map_err(|error| format!("can't save the update: {error}"))?;
    }
    if written != release.size || hex(digested.finish().as_ref()) != release.sha256 {
        return Err("the update does not match its description".to_owned());
    }
    file.sync_all()
        .map_err(|error| format!("can't save the update: {error}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Refuses an unpacked app that is not Renoa at `version`, or that the key
/// which signed the running app did not sign.
fn check_bundle(staged: &Path, version: &str, running: &Path) -> Result<(), String> {
    let plist = staged.join("Contents/Info.plist");
    let read = |key: &str| {
        run(Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(&plist))
    };
    if read("CFBundleIdentifier")?.trim() != BUNDLE_ID {
        return Err("the update is not Renoa".to_owned());
    }
    if read("CFBundleShortVersionString")?.trim() != version {
        return Err(format!("the update is not Renoa {version}"));
    }
    let requirement = designated_requirement(running)?;
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(format!("-R={requirement}"))
        .arg(staged))
    .map_err(|_| "the update is not signed by the key that signed this app".to_owned())?;
    Ok(())
}

/// What a code signature must satisfy to stand in for the app at `bundle`.
fn designated_requirement(bundle: &Path) -> Result<String, String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--requirements", "-"])
        .arg(bundle)
        .output()
        .map_err(|error| format!("can't read this app's signature: {error}"))?;
    // codesign writes the requirement as `designated => <req>`. An ad hoc
    // signature has only an implied one, `# designated => cdhash ...`, which
    // admits nothing but the same code.
    [&output.stdout, &output.stderr]
        .into_iter()
        .flat_map(|printed| {
            String::from_utf8_lossy(printed)
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .find_map(|line| {
            let line = line.strip_prefix("# ").unwrap_or(&line);
            line.strip_prefix("designated => ").map(str::to_owned)
        })
        .ok_or_else(|| "this app is not signed, so it can't check an update's signature".to_owned())
}

/// Runs `command` and returns what it printed, or what went wrong.
fn run(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("can't run {:?}: {error}", command.get_program()))?;
    if !output.status.success() {
        return Err(format!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Opens `bundle` once this process has exited.
pub fn relaunch(bundle: &Path) -> std::io::Result<()> {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$0" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$1""#)
        .arg(std::process::id().to_string())
        .arg(bundle)
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests;
