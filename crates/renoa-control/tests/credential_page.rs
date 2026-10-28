//! The credential setup page as an owner meets it: the markup, stylesheet and script a coordinator
//! serves, then that script run against that markup and a live coordinator. Node runs the script
//! (`tests/support/credential_page.mjs`), so Node 24 must be on the path, as the adapter tests need.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, JSON_WS_VERSION, NodeId, PeerIdentity,
    ServerMessage,
};
use renoa_credential_relay_protocol::{
    AcknowledgeCredentialRelayRequest, CREDENTIAL_RELAY_VERSION, CreateCredentialRelayRequest,
    CredentialRelayId, CredentialRelayKind, CredentialRelayStatus, DEVICE_ID_HEADER,
    SubmitCredentialRelayRequest,
};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::{net::TcpListener, process::Command, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const HARNESS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/support/credential_page.mjs"
);
const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'";
const ASSET_CSP: &str = "default-src 'none'; frame-ancestors 'none'";
const ISSUER: &str = "https://accounts.example.com";
const SECRET: &str = "sk-live-secret-value";

const TOKENS: &str = include_str!("../../../design/system/tokens.css");
const COMPONENTS: &str = include_str!("../../../design/system/components/bundle.css");
const MARK: &str = include_str!("../../../design/system/assets/Logos/renoa-mark-ink.svg");
const LAYOUT: &str = include_str!("../src/credential_setup.css");

// ---------------------------------------------------------------------------------------------
// What the coordinator serves.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_page_and_its_assets_carry_the_exact_security_headers() {
    let fixture = Fixture::start().await;
    let link = Link::unreserved(1);

    let page = fixture
        .get(&format!("/v1/credential-relays/{}/setup", link.relay_id))
        .await;
    assert_eq!(page.status(), reqwest::StatusCode::OK);
    assert_headers(page.headers(), "text/html; charset=utf-8", PAGE_CSP);
    let style = fixture.get("/v1/credential-setup.css").await;
    assert_eq!(style.status(), reqwest::StatusCode::OK);
    assert_headers(style.headers(), "text/css; charset=utf-8", ASSET_CSP);
    let script = fixture.get("/v1/credential-setup.js").await;
    assert_eq!(script.status(), reqwest::StatusCode::OK);
    assert_headers(
        script.headers(),
        "text/javascript; charset=utf-8",
        ASSET_CSP,
    );

    let malformed = fixture.get("/v1/credential-relays/not-a-relay/setup").await;
    assert_eq!(malformed.status(), reqwest::StatusCode::BAD_REQUEST);
    fixture.stop().await;
}

#[tokio::test]
async fn the_page_loads_only_its_own_script_and_stylesheet() {
    let fixture = Fixture::start().await;
    let page = fixture.page_html().await;

    assert_eq!(page.matches("<script").count(), 1);
    assert!(page.contains(r#"<script src="/v1/credential-setup.js" defer></script>"#));
    assert_eq!(page.matches("<link").count(), 1);
    assert!(page.contains(r#"<link rel="stylesheet" href="/v1/credential-setup.css">"#));
    for forbidden in [
        "<style",
        " style=",
        "javascript:",
        "<iframe",
        "<object",
        "<embed",
        "<img",
        "<base",
        "<meta http-equiv",
        "action=",
        "formaction",
        "srcset",
        "data:",
        // bundle.css draws these from data: images, which the page's policy blocks.
        "rn-board",
        "data-state=\"done\"",
    ] {
        assert!(
            !page.contains(forbidden),
            "the page must not contain {forbidden}"
        );
    }
    let without_svg_namespace = page.replace("http://www.w3.org/2000/svg", "");
    for external in ["http://", "https://", "=\"//", "url("] {
        assert!(
            !without_svg_namespace.contains(external),
            "no external reference: {external}"
        );
    }
    for tag in page
        .split('<')
        .skip(1)
        .filter_map(|segment| segment.split('>').next())
    {
        assert!(
            !tag.split_whitespace()
                .any(|token| token.starts_with("on") && token.contains('=')),
            "no inline event handler in <{tag}>"
        );
    }
    // With scripting off, the page says what it needs instead of staying blank.
    assert!(page.contains("<noscript>") && page.contains("This page needs JavaScript"));
    fixture.stop().await;
}

#[tokio::test]
async fn the_stylesheet_is_the_design_system_and_the_layout_holds_no_colour() {
    let fixture = Fixture::start().await;
    let served = fixture
        .get("/v1/credential-setup.css")
        .await
        .text()
        .await
        .expect("read style");

    // The design system owns tokens and components: the page serves them as they are.
    assert!(
        served.starts_with(TOKENS),
        "tokens.css must come first, unchanged"
    );
    assert!(
        served.contains(COMPONENTS),
        "bundle.css must be served unchanged"
    );
    assert!(served.ends_with(LAYOUT), "the page layout must come last");

    // The layout owns sizes and arrangement only: every colour and space reads a token.
    assert!(
        !LAYOUT.match_indices('#').any(|(index, _)| {
            LAYOUT[index + 1..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .count()
                >= 3
        }),
        "a hex colour belongs in tokens.css"
    );
    for forbidden in [
        "rgb(",
        "rgba(",
        "hsl(",
        "oklch(",
        "color-mix(",
        "url(",
        "@import",
        "@font-face",
    ] {
        assert!(
            !LAYOUT.contains(forbidden),
            "the layout must not use {forbidden}"
        );
    }
    let tokens = quoted_between(LAYOUT, "var(--", ")");
    assert!(!tokens.is_empty());
    for token in tokens {
        assert!(
            TOKENS.contains(&format!("--{token}:")),
            "--{token} is not a design token"
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn the_mark_is_the_design_system_asset_and_the_script_has_no_unsafe_calls() {
    let fixture = Fixture::start().await;
    assert!(
        fixture.page_html().await.contains(MARK),
        "the page must embed the logo asset as is"
    );

    let script = fixture
        .get("/v1/credential-setup.js")
        .await
        .text()
        .await
        .expect("read script");
    for forbidden in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "eval(",
        "new Function",
        "console.",
        "localStorage",
        "sessionStorage",
        "indexedDB",
        "XMLHttpRequest",
        "sendBeacon",
        "WebSocket",
        "location.href",
        "location.hash =",
        "location.search",
        "document.cookie",
    ] {
        assert!(
            !script.contains(forbidden),
            "the script must not use {forbidden}"
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn every_view_and_error_in_the_markup_has_an_entry_in_the_script_and_back() {
    let fixture = Fixture::start().await;
    let page = fixture.page_html().await;
    let script = fixture
        .get("/v1/credential-setup.js")
        .await
        .text()
        .await
        .expect("read script");

    let mut script_views = quoted_between(&script, "show(\"", "\"");
    script_views.extend(quoted_between(&script, "finish(\"", "\""));
    assert_eq!(quoted_between(&page, "data-view=\"", "\""), script_views);
    let mut script_errors = quoted_between(&script, "showError(\"", "\"");
    script_errors.extend(quoted_between(&script, "invalid(\"", "\""));
    assert_eq!(quoted_between(&page, "data-error=\"", "\""), script_errors);
    let ids = quoted_between(&page, " id=\"", "\"");
    for queried in quoted_between(&script, "querySelector(\"#", "\"") {
        assert!(
            ids.contains(&queried),
            "the script reads #{queried}, which the markup lacks"
        );
    }
    fixture.stop().await;
}

// ---------------------------------------------------------------------------------------------
// The script, run as a browser would, against a live coordinator.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_api_key_is_encrypted_in_the_browser_and_only_ciphertext_leaves() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(1, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;

    let report = fixture
        .run_page(
            &link,
            json!({"steps": [{"fill": {"value": format!("  {SECRET}\n")}}, {"submit": true}]}),
        )
        .await;

    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "sent"]);
    // The fragment leaves the address bar before the first request, and only the bare path remains.
    assert_eq!(
        report["events"][0],
        json!({"type": "replaceState", "url": format!("/v1/credential-relays/{}/setup", link.relay_id)})
    );
    assert_eq!(
        report["events"][1],
        json!({"type": "fetch", "kind": "form"})
    );
    let requests = report["requests"].as_array().expect("requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["method"], "POST");
    // Neither the key nor the plaintext is on the wire; the upload token is only in the body.
    let wire = report["requests"].to_string();
    for private in [link.key.as_str(), SECRET] {
        assert!(!wire.contains(private), "{private} must never be sent");
    }
    for request in requests {
        assert!(
            !request["url"]
                .as_str()
                .expect("url")
                .contains(&link.capability)
        );
    }
    assert!(requests[0]["body"].is_null());
    assert!(
        requests[1]["body"]
            .as_str()
            .expect("body")
            .contains(&link.capability)
    );

    let ready = snapshot(&report, 0);
    assert_eq!(
        (&ready["view"], &ready["kind"]),
        (&json!("ready"), &json!("api_token"))
    );
    assert_eq!(ready["subject"], "exa.default");
    assert!(
        ready["expiry"]
            .as_str()
            .is_some_and(|text| text.contains(':'))
    );
    assert_eq!(report["focusLog"][0], "value");
    let done = last_snapshot(&report);
    assert_eq!(done["view"], "sent");
    assert_eq!(
        done["inputs"],
        json!({"value": "", "client-id": "", "client-secret": ""})
    );
    // What the coordinator holds opens with the Host's key and associated data, trimmed of padding.
    assert_eq!(
        fixture.stored_plaintext(&link).await,
        json!({"schema_version": 1, "value": SECRET})
    );
    fixture.stop().await;
}

#[tokio::test]
async fn an_oauth_client_is_sealed_with_its_provider_bound_in() {
    let fixture = Fixture::start().await;
    let full = fixture
        .reserve(
            2,
            "oauth-client.abc",
            CredentialRelayKind::OAuthClient,
            Some(ISSUER),
        )
        .await;
    let bare = fixture
        .reserve(
            3,
            "oauth-client.def",
            CredentialRelayKind::OAuthClient,
            Some(ISSUER),
        )
        .await;

    let report = fixture
        .run_page(&full, json!({"steps": [{"fill": {"client-id": " client-123 ", "client-secret": " s3cret "}}, {"submit": true}]}))
        .await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "sent"]);
    let ready = snapshot(&report, 0);
    assert_eq!(
        (&ready["kind"], &ready["subject"]),
        (&json!("oauth_client"), &json!("accounts.example.com"))
    );
    assert!(!report["requests"].to_string().contains("s3cret"));
    assert_eq!(
        fixture.stored_plaintext(&full).await,
        json!({"schema_version": 1, "issuer": ISSUER, "client_id": "client-123", "client_secret": "s3cret"})
    );

    let report = fixture
        .run_page(
            &bare,
            json!({"steps": [{"fill": {"client-id": "client-123"}}, {"submit": true}]}),
        )
        .await;
    assert_clean(&report);
    assert_eq!(
        fixture.stored_plaintext(&bare).await,
        json!({"schema_version": 1, "issuer": ISSUER, "client_id": "client-123"})
    );
    fixture.stop().await;
}

#[tokio::test]
async fn an_incomplete_link_or_an_unsupported_browser_never_reaches_the_network() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(4, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let cases = [
        ("no fragment", String::new()),
        ("another version", link.fragment().replace("v=1", "v=2")),
        (
            "short key",
            format!("v=1&key={}&token={}", "11".repeat(31), link.capability),
        ),
        (
            "uppercase key",
            format!("v=1&key={}&token={}", "AB".repeat(32), link.capability),
        ),
        ("no token", format!("v=1&key={}", link.key)),
        (
            "token that is not hex",
            format!("v=1&key={}&token={}", link.key, "zz".repeat(32)),
        ),
    ];
    for (name, fragment) in cases {
        let report = fixture.run_page(&link, json!({"fragment": fragment})).await;
        assert_clean(&report);
        assert_eq!(strings(&report["views"]), ["incomplete"], "{name}");
        assert_eq!(report["requests"], json!([]), "{name} must send nothing");
        assert_eq!(report["events"][0]["type"], "replaceState", "{name}");
    }

    let report = fixture.run_page(&link, json!({"webCrypto": false})).await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["browser"]);
    assert_eq!(
        report["requests"],
        json!([]),
        "a browser that cannot encrypt must send nothing"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn an_oauth_link_without_a_usable_provider_is_incomplete() {
    let fixture = Fixture::start().await;
    for (number, issuer) in [(5, None), (6, Some("not a url"))] {
        let link = fixture
            .reserve(
                number,
                "oauth-client.abc",
                CredentialRelayKind::OAuthClient,
                issuer,
            )
            .await;
        let report = fixture.run_page(&link, json!({})).await;
        assert_clean(&report);
        assert_eq!(
            strings(&report["views"]),
            ["loading", "incomplete"],
            "{issuer:?}"
        );
        assert_eq!(report["requests"].as_array().expect("requests").len(), 1);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_link_the_coordinator_no_longer_serves_reports_that_it_expired_or_was_used() {
    let fixture = Fixture::start().await;
    let unknown = Link::unreserved(7);
    let report = fixture.run_page(&unknown, json!({})).await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "expired"]);

    let used = fixture
        .reserve(8, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    assert_eq!(
        fixture
            .submit_directly(&used, &"00".repeat(12), &"cd".repeat(17))
            .await,
        200
    );
    fixture.acknowledge(&used).await;
    let report = fixture.run_page(&used, json!({})).await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "expired"]);
    assert_eq!(last_snapshot(&report)["visibleViews"], json!(["expired"]));
    fixture.stop().await;
}

#[tokio::test]
async fn a_coordinator_outage_offers_try_again_and_the_form_returns_when_it_ends() {
    let fixture = Fixture::start().await;
    let cases = [
        (
            9,
            json!({"path": "form", "attempt": 1, "action": "network"}),
        ),
        (
            10,
            json!({"path": "form", "attempt": 1, "action": "status", "status": 503}),
        ),
        (
            21,
            json!({"path": "form", "attempt": 1, "action": "status", "status": 429}),
        ),
        // An answer that is not a form at all.
        (
            22,
            json!({"path": "form", "attempt": 1, "action": "json", "body": null}),
        ),
    ];
    for (number, fault) in cases {
        let link = fixture
            .reserve(number, "exa.default", CredentialRelayKind::ApiToken, None)
            .await;
        let report = fixture
            .run_page(
                &link,
                json!({"faults": [fault], "steps": [{"click": "retry"}]}),
            )
            .await;
        assert_clean(&report);
        assert_eq!(snapshot(&report, 0)["visibleViews"], json!(["unavailable"]));
        assert_eq!(
            strings(&report["views"]),
            ["loading", "unavailable", "loading", "ready"]
        );
        assert_eq!(snapshot(&report, 1)["visibleViews"], json!(["ready"]));
        assert_eq!(report["requests"].as_array().expect("requests").len(), 2);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_credential_kind_the_page_does_not_know_is_reported_without_a_form() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(11, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let unknown = json!({"version": 1, "relayId": link.relay_id.to_string(), "credentialId": "exa.default", "kind": "ssh_key", "expiresAtMs": 4_102_444_800_000_i64});
    let report = fixture
        .run_page(
            &link,
            json!({"faults": [{"path": "form", "attempt": 1, "action": "json", "body": unknown}]}),
        )
        .await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "kind"]);
    fixture.stop().await;
}

// ---------------------------------------------------------------------------------------------
// Submitting.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_lost_answer_is_retried_without_a_conflict() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(12, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let lost = json!({"path": "submit", "attempt": 1, "action": "lost_answer"});

    let report = fixture
        .run_page(&link, json!({"faults": [lost], "steps": [{"fill": {"value": SECRET}}, {"submit": true}, {"submit": true}]}))
        .await;

    assert_clean(&report);
    let after_loss = snapshot(&report, 2);
    assert_eq!(after_loss["view"], "ready");
    assert_eq!(after_loss["visibleErrors"], json!(["unreachable"]));
    assert_eq!(
        after_loss["inputs"]["value"], SECRET,
        "what the owner typed must stay"
    );
    assert_eq!(
        (&after_loss["sendDisabled"], &after_loss["busy"]),
        (&json!(false), &Value::Null)
    );
    assert_eq!(strings(&report["views"]), ["loading", "ready", "sent"]);
    // The retry sends the same sealed bytes, so the stored submission is accepted again.
    let requests = report["requests"].as_array().expect("requests");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"], requests[2]["body"]);
    assert_eq!(
        fixture.stored_plaintext(&link).await,
        json!({"schema_version": 1, "value": SECRET})
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_changed_value_after_a_lost_answer_reports_the_conflict_with_a_fresh_nonce() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(13, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let lost = json!({"path": "submit", "attempt": 1, "action": "lost_answer"});

    let report = fixture
        .run_page(
            &link,
            json!({"faults": [lost], "steps": [
                {"fill": {"value": SECRET}}, {"submit": true},
                {"fill": {"value": "another-value"}}, {"submit": true},
            ]}),
        )
        .await;

    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "used"]);
    assert_eq!(
        last_snapshot(&report)["inputs"]["value"],
        "",
        "a used-up link keeps nothing typed"
    );
    let requests = report["requests"].as_array().expect("requests");
    let nonces = [1, 2].map(|index| {
        let body: Value = serde_json::from_str(requests[index]["body"].as_str().expect("body"))
            .expect("body is JSON");
        body["nonce"].clone()
    });
    assert_ne!(
        nonces[0], nonces[1],
        "one nonce must never seal two different values"
    );
    // The first value is the one the coordinator kept.
    assert_eq!(
        fixture.stored_plaintext(&link).await,
        json!({"schema_version": 1, "value": SECRET})
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_server_error_keeps_the_form_and_the_next_try_succeeds() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(14, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let outage = json!({"path": "submit", "attempt": 1, "action": "status", "status": 503});

    let report = fixture
        .run_page(&link, json!({"faults": [outage], "steps": [{"fill": {"value": SECRET}}, {"submit": true}, {"submit": true}]}))
        .await;

    assert_clean(&report);
    assert_eq!(
        snapshot(&report, 2)["visibleErrors"],
        json!(["unreachable"])
    );
    assert_eq!(snapshot(&report, 2)["view"], "ready");
    assert_eq!(last_snapshot(&report)["view"], "sent");
    assert_eq!(last_snapshot(&report)["visibleErrors"], json!([]));
    assert_eq!(
        fixture.stored_plaintext(&link).await,
        json!({"schema_version": 1, "value": SECRET})
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_blank_value_is_refused_before_anything_is_encrypted() {
    let fixture = Fixture::start().await;
    let key = fixture
        .reserve(15, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let client = fixture
        .reserve(
            16,
            "oauth-client.abc",
            CredentialRelayKind::OAuthClient,
            Some(ISSUER),
        )
        .await;

    let report = fixture
        .run_page(&key, json!({"steps": [{"submit": true}, {"fill": {"value": "   "}}, {"submit": true}, {"fill": {"value": "k"}}, {"submit": true}]}))
        .await;
    assert_clean(&report);
    for index in [1, 3] {
        let refused = snapshot(&report, index);
        assert_eq!(refused["visibleErrors"], json!(["empty-value"]));
        assert_eq!(refused["ariaInvalid"], json!(["value"]));
        assert_eq!(refused["fieldInvalid"], json!(["field-value"]));
        assert_eq!(refused["requestCount"], 1, "only the form was fetched");
    }
    assert_eq!(
        report["focusLog"][1], "value",
        "the empty field takes focus"
    );
    assert_eq!(last_snapshot(&report)["view"], "sent");
    assert_eq!(
        fixture.stored_plaintext(&key).await,
        json!({"schema_version": 1, "value": "k"})
    );

    let report = fixture
        .run_page(&client, json!({"steps": [{"fill": {"client-secret": "s3cret"}}, {"submit": true}, {"fill": {"client-id": "client-123"}}, {"submit": true}]}))
        .await;
    assert_clean(&report);
    let refused = snapshot(&report, 2);
    assert_eq!(refused["visibleErrors"], json!(["empty-client-id"]));
    assert_eq!(refused["ariaInvalid"], json!(["client-id"]));
    assert_eq!(refused["requestCount"], 1);
    assert_eq!(last_snapshot(&report)["view"], "sent");
    fixture.stop().await;
}

#[tokio::test]
async fn a_link_the_coordinator_rejects_at_submit_clears_what_was_typed() {
    let fixture = Fixture::start().await;
    let mismatched = fixture
        .reserve(17, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let taken = fixture
        .reserve(18, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let lapsed = fixture
        .reserve(19, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let typed = json!({"steps": [{"fill": {"value": SECRET}}, {"submit": true}]});

    // An upload token that is not the relay's: refused as unauthorised.
    let mut altered = mismatched.clone();
    altered.capability = "33".repeat(32);
    let report = fixture.run_page(&altered, typed.clone()).await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "expired"]);
    assert_eq!(last_snapshot(&report)["inputs"]["value"], "");
    assert!(matches!(
        fixture.relay_status(&mismatched).await,
        CredentialRelayStatus::Pending { .. }
    ));

    // A different value already stored: refused as a conflict, and the stored one stays.
    assert_eq!(
        fixture
            .submit_directly(&taken, &"00".repeat(12), &"cd".repeat(17))
            .await,
        200
    );
    let report = fixture.run_page(&taken, typed.clone()).await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "used"]);
    assert_eq!(last_snapshot(&report)["inputs"]["value"], "");
    let CredentialRelayStatus::Submitted { ciphertext, .. } = fixture.relay_status(&taken).await
    else {
        panic!("the first submission must remain stored")
    };
    assert_eq!(ciphertext, "cd".repeat(17));

    // A relay that lapses between loading the form and sending (simulated: only expiry answers 404).
    let gone = json!({"path": "submit", "attempt": 1, "action": "status", "status": 404});
    let mut lapsing = typed;
    lapsing["faults"] = json!([gone]);
    let report = fixture.run_page(&lapsed, lapsing).await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "expired"]);
    fixture.stop().await;
}

#[tokio::test]
async fn a_busy_coordinator_is_a_retry_and_a_refused_request_is_a_dead_link() {
    let fixture = Fixture::start().await;
    let typed = |fault: Value| json!({"faults": [fault], "steps": [{"fill": {"value": SECRET}}, {"submit": true}]});
    let busy = fixture
        .reserve(23, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let report = fixture
        .run_page(
            &busy,
            typed(json!({"path": "submit", "attempt": 1, "action": "status", "status": 429})),
        )
        .await;
    assert_clean(&report);
    assert_eq!(last_snapshot(&report)["view"], "ready");
    assert_eq!(
        last_snapshot(&report)["visibleErrors"],
        json!(["unreachable"])
    );
    assert_eq!(last_snapshot(&report)["inputs"]["value"], SECRET);

    let refused = fixture
        .reserve(24, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;
    let report = fixture
        .run_page(
            &refused,
            typed(json!({"path": "submit", "attempt": 1, "action": "status", "status": 400})),
        )
        .await;
    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready", "expired"]);
    assert_eq!(last_snapshot(&report)["inputs"]["value"], "");
    fixture.stop().await;
}

#[tokio::test]
async fn a_browser_that_fails_to_encrypt_sends_nothing_and_keeps_the_value() {
    let fixture = Fixture::start().await;
    let link = fixture
        .reserve(20, "exa.default", CredentialRelayKind::ApiToken, None)
        .await;

    let report = fixture
        .run_page(&link, json!({"failEncrypt": true, "steps": [{"fill": {"value": SECRET}}, {"submit": true}, {"submit": true}]}))
        .await;

    assert_clean(&report);
    assert_eq!(strings(&report["views"]), ["loading", "ready"]);
    for index in [2, 3] {
        let failed = snapshot(&report, index);
        assert_eq!(failed["visibleErrors"], json!(["encrypt"]));
        assert_eq!(failed["inputs"]["value"], SECRET);
        assert_eq!(
            failed["requestCount"], 1,
            "nothing may be sent when sealing fails"
        );
    }
    assert!(matches!(
        fixture.relay_status(&link).await,
        CredentialRelayStatus::Pending { .. }
    ));
    fixture.stop().await;
}

// ---------------------------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Link {
    relay_id: CredentialRelayId,
    credential_id: String,
    kind: CredentialRelayKind,
    issuer: Option<String>,
    key: String,
    capability: String,
}

impl Link {
    /// A link whose relay the coordinator has never heard of.
    fn unreserved(number: u128) -> Self {
        Self {
            relay_id: CredentialRelayId::from_uuid(Uuid::from_u128(number)),
            credential_id: "exa.default".to_owned(),
            kind: CredentialRelayKind::ApiToken,
            issuer: None,
            key: "11".repeat(32),
            capability: "22".repeat(32),
        }
    }

    fn fragment(&self) -> String {
        let mut fragment = url::form_urlencoded::Serializer::new(String::new());
        fragment
            .append_pair("v", "1")
            .append_pair("key", &self.key)
            .append_pair("token", &self.capability);
        if let Some(issuer) = &self.issuer {
            fragment.append_pair("issuer", issuer);
        }
        fragment.finish()
    }

    /// Opens what the page sealed, with the key and associated data the Host holds for this relay.
    fn open(&self, nonce: &str, ciphertext: &str) -> Value {
        let key = UnboundKey::new(&AES_256_GCM, &unhex(&self.key))
            .map(LessSafeKey::new)
            .expect("valid key");
        let nonce: [u8; 12] = unhex(nonce).try_into().expect("96-bit nonce");
        let kind = match self.kind {
            CredentialRelayKind::ApiToken => "api_token",
            CredentialRelayKind::OAuthClient => "oauth_client",
        };
        let aad = format!(
            "renoa credential relay v2\0{}\0{}\0{kind}\0{}",
            self.relay_id,
            self.credential_id,
            self.issuer.as_deref().unwrap_or_default()
        );
        let mut buffer = unhex(ciphertext);
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut buffer,
            )
            .expect("the ciphertext opens with the Host's key and associated data");
        serde_json::from_slice(plaintext).expect("plaintext is JSON")
    }
}

struct Fixture {
    http: String,
    node: DeviceCredentials,
    client: reqwest::Client,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
    _files: TempDir,
}

impl Fixture {
    async fn start() -> Self {
        let files = TempDir::new().expect("temporary control directory");
        let coordinator = Coordinator::open_with_passkeys(
            files.path().join("control.sqlite"),
            "localhost",
            "http://localhost",
        )
        .expect("open coordinator");
        let enrollment = coordinator
            .create_enrollment(
                PeerIdentity::Node {
                    node_id: NodeId::new(),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create node enrollment");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let shutdown = CancellationToken::new();
        let server_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            coordinator
                .serve(listener, server_shutdown)
                .await
                .expect("serve coordinator");
        });
        let node = enroll(&format!("ws://{address}/connect"), enrollment).await;
        Self {
            http: format!("http://{address}"),
            node,
            client: reqwest::Client::new(),
            shutdown,
            task,
            _files: files,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("coordinator task joins");
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(DEVICE_ID_HEADER, self.node.device_id.to_string())
            .bearer_auth(self.node.credential.expose())
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{path}", self.http))
            .send()
            .await
            .expect("request the coordinator")
    }

    async fn page_html(&self) -> String {
        let link = Link::unreserved(99);
        self.get(&format!("/v1/credential-relays/{}/setup", link.relay_id))
            .await
            .text()
            .await
            .expect("read page")
    }

    /// Reserves a relay as the Host would and returns the link it would hand the owner. The issuer
    /// only ever travels in the link, as it does in production.
    async fn reserve(
        &self,
        number: u128,
        credential_id: &str,
        kind: CredentialRelayKind,
        issuer: Option<&str>,
    ) -> Link {
        let link = Link {
            credential_id: credential_id.to_owned(),
            kind,
            issuer: issuer.map(str::to_owned),
            ..Link::unreserved(number)
        };
        let response = self
            .authorized(
                self.client
                    .post(format!("{}/v1/credential-relays", self.http)),
            )
            .json(&CreateCredentialRelayRequest {
                version: CREDENTIAL_RELAY_VERSION,
                relay_id: link.relay_id,
                credential_id: link.credential_id.clone(),
                kind,
                capability_digest: hex_sha256(link.capability.as_bytes()),
            })
            .send()
            .await
            .expect("reserve relay");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        link
    }

    async fn relay_status(&self, link: &Link) -> CredentialRelayStatus {
        let response = self
            .authorized(self.client.get(format!(
                "{}/v1/credential-relays/{}",
                self.http, link.relay_id
            )))
            .send()
            .await
            .expect("read relay status");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.json().await.expect("decode relay status")
    }

    async fn stored_plaintext(&self, link: &Link) -> Value {
        let CredentialRelayStatus::Submitted {
            nonce, ciphertext, ..
        } = self.relay_status(link).await
        else {
            panic!("the coordinator must hold a submission")
        };
        link.open(&nonce, &ciphertext)
    }

    async fn submit_directly(&self, link: &Link, nonce: &str, ciphertext: &str) -> u16 {
        self.client
            .post(format!(
                "{}/v1/credential-relays/{}/submit",
                self.http, link.relay_id
            ))
            .json(&SubmitCredentialRelayRequest {
                version: CREDENTIAL_RELAY_VERSION,
                capability: link.capability.clone(),
                nonce: nonce.to_owned(),
                ciphertext: ciphertext.to_owned(),
            })
            .send()
            .await
            .expect("submit directly")
            .status()
            .as_u16()
    }

    async fn acknowledge(&self, link: &Link) {
        let response = self
            .authorized(self.client.post(format!(
                "{}/v1/credential-relays/{}/acknowledge",
                self.http, link.relay_id
            )))
            .json(&AcknowledgeCredentialRelayRequest {
                version: CREDENTIAL_RELAY_VERSION,
            })
            .send()
            .await
            .expect("acknowledge relay");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }

    /// Runs the served page and script in Node against this coordinator and returns its report.
    async fn run_page(&self, link: &Link, mut scenario: Value) -> Value {
        scenario["origin"] = json!(self.http);
        scenario["relayId"] = json!(link.relay_id.to_string());
        if scenario.get("fragment").is_none() {
            scenario["fragment"] = json!(link.fragment());
        }
        let output = Command::new("node")
            .arg(HARNESS)
            .arg(scenario.to_string())
            .output()
            .await
            .expect("run node");
        assert!(
            output.status.success(),
            "the page harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("the harness prints one JSON report")
    }
}

async fn enroll(endpoint: &str, token: renoa_control::EnrollmentToken) -> DeviceCredentials {
    let (mut socket, _) = connect_async(endpoint).await.expect("connect enrollment");
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            })
            .expect("encode enrollment")
            .into(),
        ))
        .await
        .expect("send enrollment");
    let Message::Text(text) = socket
        .next()
        .await
        .expect("enrollment response")
        .expect("valid enrollment response")
    else {
        panic!("enrollment response must be text")
    };
    let ServerMessage::Enrolled { credentials, .. } =
        serde_json::from_str(&text).expect("decode enrollment response")
    else {
        panic!("coordinator must issue credentials")
    };
    credentials
}

fn assert_headers(headers: &reqwest::header::HeaderMap, content_type: &str, csp: &str) {
    for (name, expected) in [
        ("content-type", content_type),
        ("cache-control", "no-store"),
        ("pragma", "no-cache"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("content-security-policy", csp),
    ] {
        assert_eq!(
            headers.get(name).and_then(|value| value.to_str().ok()),
            Some(expected),
            "{name}"
        );
    }
}

fn assert_clean(report: &Value) {
    assert_eq!(
        report["scriptError"],
        Value::Null,
        "{}",
        report["scriptError"]
    );
    assert_eq!(report["unhandled"], json!([]));
    assert_eq!(
        report["consoleCalls"],
        json!([]),
        "the page must not write to the console"
    );
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string").to_owned())
        .collect()
}

fn snapshot(report: &Value, index: usize) -> &Value {
    &report["snapshots"][index]
}

fn last_snapshot(report: &Value) -> &Value {
    report["snapshots"]
        .as_array()
        .and_then(|all| all.last())
        .expect("a snapshot")
}

/// Every value found between `opener` and the next `closer`, for reading names out of source text.
fn quoted_between(source: &str, opener: &str, closer: &str) -> BTreeSet<String> {
    source
        .split(opener)
        .skip(1)
        .filter_map(|rest| rest.split(closer).next())
        .map(str::to_owned)
        .collect()
}

fn unhex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).expect("hex digit pair"))
        .collect()
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("write SHA-256 hex");
            output
        })
}
