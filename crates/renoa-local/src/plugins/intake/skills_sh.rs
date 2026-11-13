use std::path::Path;

use url::Url;

use super::github::is_valid_segment;
use crate::plugins::PluginError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SkillsShTarget {
    pub(super) repository: String,
    pub(super) skill: String,
}

pub(super) fn parse(raw: &str) -> Result<SkillsShTarget, PluginError> {
    let url = Url::parse(raw).map_err(|_| invalid_url())?;
    if url.scheme() != "https"
        || url.host_str() != Some("skills.sh")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_url());
    }
    let parts = url
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if parts.first() == Some(&"site") {
        return Err(PluginError::Invalid(format!(
            "`{raw}` is not hosted on GitHub (skills.sh /site/ pages are not GitHub repositories); pass a GitHub repository directly with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":\"https://github.com/<owner>/<repo>\"}}}}"
        )));
    }
    let [owner, repo, skill] = parts.as_slice() else {
        return Err(invalid_url());
    };
    if !is_valid_segment(owner)
        || !is_valid_segment(repo)
        || Path::new(repo)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("git"))
        || skill.is_empty()
        || *skill == "."
        || *skill == ".."
        || skill.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(invalid_url());
    }
    Ok(SkillsShTarget {
        repository: format!("https://github.com/{owner}/{repo}"),
        skill: (*skill).to_owned(),
    })
}

fn invalid_url() -> PluginError {
    PluginError::Invalid(
        "skills_sh url must be https://skills.sh/<owner>/<repo>/<skill> without credentials, query, or fragment; retry inspect with {\"action\":\"inspect\",\"source\":{\"kind\":\"skills_sh\",\"url\":\"https://skills.sh/<owner>/<repo>/<skill>\"}}".to_owned(),
    )
}
