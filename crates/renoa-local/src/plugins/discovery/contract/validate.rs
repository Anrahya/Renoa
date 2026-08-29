use std::collections::HashSet;

use super::{
    EMPTY_SEARCH_NEXT_ACTION, LOOKUP_NEXT_ACTION, PackageBlocker, PublisherVerification,
    RegistryCandidate, RegistryError, RegistryLookupResult, RegistryPackage, RegistryPublisher,
    RegistryRemote, RegistryRepository, RegistrySearchResult, RegistryTransport, RegistryTrust,
    RemoteBlocker, SEARCH_NEXT_ACTION, UnverifiedClaim, VerifiedClaim,
};

const MAX_SOURCE_RECORDS: usize = 8 * 3 * 100;

pub(super) fn search(result: &RegistrySearchResult) -> Result<(), RegistryError> {
    require_bytes("Registry search query", &result.query, 1, 256)?;
    if result.normalized_queries.is_empty() || result.normalized_queries.len() > 8 {
        return Err(protocol(
            "Registry search must contain one to eight normalized queries",
        ));
    }
    let mut queries = HashSet::new();
    for query in &result.normalized_queries {
        require_identity("Registry normalized query", query, 1, 256)?;
        if !queries.insert(query) {
            return Err(protocol("Registry normalized queries contain a duplicate"));
        }
    }
    if result.candidates.len() > 100 {
        return Err(protocol(
            "Registry search returned more than 100 candidates",
        ));
    }
    let mut identities = HashSet::new();
    for candidate in &result.candidates {
        validate_candidate(candidate)?;
        if !identities.insert((&candidate.registry_name, &candidate.registry_version)) {
            return Err(protocol("Registry search repeated an exact server version"));
        }
    }
    if result.coverage.returned != result.candidates.len()
        || result.coverage.unique_seen < result.coverage.returned
        || result.coverage.unique_seen > MAX_SOURCE_RECORDS
        || result.coverage.rejected_records > MAX_SOURCE_RECORDS
        || result.coverage.filtered_records > MAX_SOURCE_RECORDS
        || result.coverage.output_truncated
            != (result.coverage.returned < result.coverage.unique_seen)
    {
        return Err(protocol("Registry search coverage is inconsistent"));
    }
    validate_trust(&result.trust)?;
    let expected = if result.candidates.is_empty() {
        EMPTY_SEARCH_NEXT_ACTION
    } else {
        SEARCH_NEXT_ACTION
    };
    require_fixed("Registry search next action", &result.next_action, expected)
}

pub(super) fn lookup(result: &RegistryLookupResult) -> Result<(), RegistryError> {
    let record = &result.record;
    validate_identity(&record.registry_name, &record.registry_version)?;
    validate_common(
        &record.registry_name,
        record.title.as_deref(),
        &record.publisher_description,
        &record.publisher,
        record.website_url.as_deref(),
        record.repository.as_ref(),
    )?;
    if record.remotes.len() > 64 || record.packages.len() > 64 {
        return Err(protocol(
            "Registry lookup returned more than 64 remotes or packages",
        ));
    }
    for remote in &record.remotes {
        validate_remote(remote)?;
    }
    for package in &record.packages {
        validate_package(package)?;
    }
    validate_source_record(&record.source_record)?;
    validate_trust(&result.trust)?;
    require_fixed(
        "Registry lookup next action",
        &result.next_action,
        LOOKUP_NEXT_ACTION,
    )
}

fn validate_candidate(candidate: &RegistryCandidate) -> Result<(), RegistryError> {
    validate_identity(&candidate.registry_name, &candidate.registry_version)?;
    validate_common(
        &candidate.registry_name,
        candidate.title.as_deref(),
        &candidate.publisher_description,
        &candidate.publisher,
        candidate.website_url.as_deref(),
        candidate.repository.as_ref(),
    )?;
    if candidate.remote_count > 64
        || candidate.package_count > 64
        || candidate.streamable_http_count > candidate.remote_count
    {
        return Err(protocol("Registry candidate counts exceed their bounds"));
    }
    Ok(())
}

fn validate_common(
    name: &str,
    title: Option<&str>,
    description: &str,
    publisher: &RegistryPublisher,
    website: Option<&str>,
    repository: Option<&RegistryRepository>,
) -> Result<(), RegistryError> {
    if let Some(title) = title {
        require_identity("Registry title", title, 1, 512)?;
    }
    require_identity("Registry publisher description", description, 1, 1_024)?;
    let namespace = name
        .split_once('/')
        .map(|(namespace, _)| namespace)
        .ok_or_else(|| protocol("Registry server name has no publisher namespace"))?;
    if publisher.namespace != namespace {
        return Err(protocol(
            "Registry publisher namespace does not match the server name",
        ));
    }
    let expected_verification = if namespace.starts_with("io.github.") {
        PublisherVerification::Github
    } else {
        PublisherVerification::Domain
    };
    if publisher.verification != expected_verification {
        return Err(protocol(
            "Registry publisher verification does not match its namespace",
        ));
    }
    if let Some(website) = website {
        require_https_url("Registry website URL", website)?;
    }
    if let Some(repository) = repository {
        require_https_url("Registry repository URL", &repository.url)?;
        require_identity("Registry repository source", &repository.source, 1, 64)?;
        if let Some(id) = &repository.id {
            require_identity("Registry repository id", id, 1, 256)?;
        }
    }
    Ok(())
}

fn validate_identity(name: &str, version: &str) -> Result<(), RegistryError> {
    if name.len() > 200 {
        return Err(protocol("Registry server name exceeds 200 bytes"));
    }
    let Some((namespace, leaf)) = name.split_once('/') else {
        return Err(protocol("Registry server name is malformed"));
    };
    if namespace.is_empty()
        || leaf.is_empty()
        || name.matches('/').count() != 1
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(protocol("Registry server name is malformed"));
    }
    require_identity("Registry server version", version, 1, 255)?;
    if version == "latest" || version.contains('/') {
        return Err(protocol("Registry server version must be exact"));
    }
    Ok(())
}

fn validate_remote(remote: &RegistryRemote) -> Result<(), RegistryError> {
    if remote.headers.len() > 64 {
        return Err(protocol("Registry remote has more than 64 headers"));
    }
    let (has_template, is_https) = endpoint_shape(&remote.url)?;
    let expected_blocker = match remote.transport {
        RegistryTransport::Unknown => Some(RemoteBlocker::UnsupportedTransport),
        RegistryTransport::Sse => Some(RemoteBlocker::SseTransportUnsupported),
        RegistryTransport::StreamableHttp if has_template => {
            Some(RemoteBlocker::EndpointTemplateUnsupported)
        }
        RegistryTransport::StreamableHttp if !is_https => Some(RemoteBlocker::HttpsRequired),
        RegistryTransport::StreamableHttp => None,
    };
    if remote.blocker != expected_blocker
        || remote.transport_supported != expected_blocker.is_none()
    {
        return Err(protocol(
            "Registry remote support status does not match its transport and URL",
        ));
    }
    match (remote.transport, remote.declared_transport.as_deref()) {
        (RegistryTransport::Unknown, Some(declared)) => {
            require_identity("Registry declared remote transport", declared, 1, 64)?;
            if declared == "streamable-http" || declared == "sse" {
                return Err(protocol(
                    "Registry known remote transport was mislabeled as unknown",
                ));
            }
        }
        (RegistryTransport::Unknown, None) => {
            return Err(protocol(
                "Registry unknown remote transport has no declared value",
            ));
        }
        (RegistryTransport::StreamableHttp | RegistryTransport::Sse, None) => {}
        (RegistryTransport::StreamableHttp | RegistryTransport::Sse, Some(_)) => {
            return Err(protocol(
                "Registry known remote transport has an unexpected declared value",
            ));
        }
    }
    for header in &remote.headers {
        require_identity("Registry header name", &header.name, 1, 256)?;
        if let Some(description) = &header.description {
            require_identity("Registry header description", description, 1, 512)?;
        }
    }
    Ok(())
}

fn validate_package(package: &RegistryPackage) -> Result<(), RegistryError> {
    require_identity("Registry package type", &package.registry_type, 1, 64)?;
    require_identity("Registry package identifier", &package.identifier, 1, 2_048)?;
    let identifier_lower = package.identifier.to_ascii_lowercase();
    if identifier_lower.starts_with("http://") || identifier_lower.starts_with("https://") {
        require_https_url("Registry package identifier URL", &package.identifier)?;
    }
    if let Some(version) = &package.version {
        require_identity("Registry package version", version, 1, 255)?;
    }
    if package.supported_by_renoa
        || package.blocker != PackageBlocker::LocalPackageExecutionNotSupported
    {
        return Err(protocol(
            "Registry package must remain explicitly unsupported by Renoa",
        ));
    }
    Ok(())
}

fn validate_source_record(value: &str) -> Result<(), RegistryError> {
    let url = require_https_url("Registry source record", value)?;
    if url.host_str() != Some("registry.modelcontextprotocol.io")
        || !url.path().starts_with("/v0.1/servers/")
        || !url.path().contains("/versions/")
        || url.query().is_some()
    {
        return Err(protocol(
            "Registry source record is not an exact official Registry version URL",
        ));
    }
    Ok(())
}

fn validate_trust(trust: &RegistryTrust) -> Result<(), RegistryError> {
    let expected = [
        UnverifiedClaim::ProviderEndorsement,
        UnverifiedClaim::MetadataAccuracy,
        UnverifiedClaim::ServerSafety,
        UnverifiedClaim::EndpointBehavior,
    ];
    if trust.verified != VerifiedClaim::PublisherNamespaceControl || trust.not_verified != expected
    {
        return Err(protocol(
            "Registry trust statement is not the fixed Host policy",
        ));
    }
    Ok(())
}

fn endpoint_shape(value: &str) -> Result<(bool, bool), RegistryError> {
    require_bytes("Registry endpoint URL", value, 1, 8 * 1_024)?;
    if value.chars().any(char::is_whitespace) || value.chars().any(char::is_control) {
        return Err(protocol(
            "Registry endpoint URL contains whitespace or control characters",
        ));
    }
    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars();
    let mut has_template = false;
    while let Some(character) = characters.next() {
        if character == '}' {
            return Err(protocol("Registry endpoint URL has a malformed template"));
        }
        if character != '{' {
            normalized.push(character);
            continue;
        }
        has_template = true;
        let mut name = String::new();
        loop {
            let next = characters
                .next()
                .ok_or_else(|| protocol("Registry endpoint URL has a malformed template"))?;
            if next == '}' {
                break;
            }
            if next == '{' || !next.is_ascii_alphanumeric() && !"._-".contains(next) {
                return Err(protocol("Registry endpoint URL has a malformed template"));
            }
            name.push(next);
        }
        if name.is_empty() {
            return Err(protocol("Registry endpoint URL has a malformed template"));
        }
        normalized.push_str("template-value");
    }
    let url = url::Url::parse(&normalized)
        .map_err(|error| protocol(format!("Registry endpoint URL is invalid: {error}")))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host().is_none()
    {
        return Err(protocol(
            "Registry endpoint URL contains credentials, a fragment, or no host",
        ));
    }
    if !has_template && url.query().is_some() {
        return Err(protocol(
            "Registry endpoint URL contains concrete query parameters",
        ));
    }
    Ok((has_template, url.scheme() == "https"))
}

fn require_https_url(field: &str, value: &str) -> Result<url::Url, RegistryError> {
    require_bytes(field, value, 1, 8 * 1_024)?;
    let url =
        url::Url::parse(value).map_err(|error| protocol(format!("{field} is invalid: {error}")))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || url.host().is_none()
    {
        return Err(protocol(format!(
            "{field} must be HTTPS without credentials or a fragment"
        )));
    }
    Ok(url)
}

fn require_identity(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), RegistryError> {
    require_bytes(field, value, minimum, maximum)?;
    if value.chars().any(char::is_control) {
        Err(protocol(format!("{field} contains control characters")))
    } else {
        Ok(())
    }
}

fn require_bytes(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), RegistryError> {
    if (minimum..=maximum).contains(&value.len()) {
        Ok(())
    } else {
        Err(protocol(format!(
            "{field} must contain {minimum}-{maximum} UTF-8 bytes"
        )))
    }
}

fn require_fixed(field: &str, value: &str, expected: &str) -> Result<(), RegistryError> {
    if value == expected {
        Ok(())
    } else {
        Err(protocol(format!(
            "{field} does not match the fixed Host policy"
        )))
    }
}

fn protocol(message: impl Into<String>) -> RegistryError {
    RegistryError::Protocol(message.into())
}
