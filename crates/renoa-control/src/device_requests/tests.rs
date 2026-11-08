//! Device requests against a real store: a computer asks, an app approves by
//! code, and only the key's holder collects, once per credential.

use std::time::{Duration, SystemTime};

use renoa_protocol::{PrincipalId, SurfaceRef};
use uuid::Uuid;

use super::{
    CODE_ALPHABET, CODE_LENGTH, Collected, MAX_ACTIVE_REQUESTS, MAX_MISSES, NewRequest,
    REQUEST_LIFETIME, code_digest, new_code,
};
use crate::{
    ControlError, DeviceCode, DeviceId,
    control_schema::open_connection,
    coordinator::ControlErrorKind,
    device_key::{DevicePublicKey, DeviceRegistration},
    device_store::sign_in_device,
    identity_store::timestamp_millis,
    store::ControlStore,
};

// Two keys and their signatures over the request message, made outside this
// crate with `openssl dgst -sha256 -sign`, as a computer's key would make them.
const FIRST: u128 = 0xa1;
const SECOND: u128 = 0xa2;

struct Computer {
    key: &'static str,
    proofs: &'static [(u128, &'static str)],
}

const COMPUTER: Computer = Computer {
    key: "BCTt5QvEGs7ZC__8uHrpq0TMB1vN4N6Oj_uXPaVnRW2X9YNJbnBWYVPO8JdOMT0-GBJ_96iwoJe8_rAx6S-Fbx0",
    proofs: &[
        (
            FIRST,
            "MEQCIGDfIpht28Svysr4vkpKODTzftc77LblqNZ-tPqZYCTCAiA_sM32e6lIMZEHAqB3H6drSMOfDWHDDDeRUKXD5rYBaA",
        ),
        (
            SECOND,
            "MEYCIQDQPTF81IZd_d3CQPTM5fTJVeYxD90qc-SNs_bAn1dIJQIhAMD-eRQb__FIDXUbbieX75kF9vRS5SUIifWKwQaJsnJR",
        ),
    ],
};

const STRANGER: Computer = Computer {
    key: "BLiJXJfsSjadg0aqgXhL9AMv8eIwRNdaNY-Gqr5TU5Ln_D0j6SnMQ5Wgnpgza_YYaSKl3FruFfteY5Ws0VF-_dA",
    proofs: &[(
        FIRST,
        "MEYCIQCAqdclF5Gigmm6qsHmJf23HUde3fZF_H4pS4qr3B1F5gIhAPKnJn2cLE2ExFjx3p_D9zsaFPwJ8EfpH8QPF6FS_KeJ",
    )],
};

impl Computer {
    fn request(&self, request_id: Uuid, label: &str) -> NewRequest {
        NewRequest {
            request_id,
            surface: SurfaceRef::new("mac"),
            registration: DeviceRegistration {
                key: DevicePublicKey::parse(self.key).expect("key"),
                label: label.to_owned(),
            },
        }
    }

    fn proof(&self, request_id: Uuid) -> String {
        self.proofs
            .iter()
            .find(|(id, _)| Uuid::from_u128(*id) == request_id)
            .map(|(_, proof)| (*proof).to_owned())
            .expect("a signature over this request")
    }
}

/// A request with a key distinct per `seed`. Storing a request checks the
/// shape of a key; the HTTP route checks its proof.
fn request_with_key(seed: u16, request_id: Uuid) -> NewRequest {
    let mut point = [0_u8; 65];
    point[0] = 0x04;
    point[1..3].copy_from_slice(&seed.to_be_bytes());
    NewRequest {
        request_id,
        surface: SurfaceRef::new("mac"),
        registration: DeviceRegistration {
            key: DevicePublicKey::from_bytes(&point).expect("key shape"),
            label: "Mac".to_owned(),
        },
    }
}

/// The app every approval in these tests comes from, unless a test signs in its own.
const APP: u128 = 0xaa;

async fn approve(
    store: &ControlStore,
    principal_id: PrincipalId,
    code: &str,
    now: SystemTime,
) -> Result<(SurfaceRef, String), ControlError> {
    approve_by(
        store,
        principal_id,
        DeviceId::from_uuid(Uuid::from_u128(APP)),
        code,
        now,
    )
    .await
}

async fn approve_by(
    store: &ControlStore,
    principal_id: PrincipalId,
    approver: DeviceId,
    code: &str,
    now: SystemTime,
) -> Result<(SurfaceRef, String), ControlError> {
    store
        .approve_device_request(principal_id, approver, &DeviceCode::new(code), now)
        .await
}

/// Signs in an app of `principal_id` directly, as a passkey sign-in would.
fn signed_in_app(store: &ControlStore, principal_id: PrincipalId, seed: u16) -> DeviceId {
    let request = request_with_key(seed, Uuid::new_v4());
    let mut connection = open_connection(&store.path).expect("connection");
    let transaction = connection.transaction().expect("transaction");
    let device = sign_in_device(
        &transaction,
        principal_id,
        &SurfaceRef::new("android"),
        &request.registration,
        timestamp_millis(SystemTime::now()).expect("now"),
    )
    .expect("sign in");
    transaction.commit().expect("commit");
    device.credentials.device_id
}

fn credential_hash(store: &ControlStore, device_id: DeviceId) -> Vec<u8> {
    open_connection(&store.path)
        .expect("connection")
        .query_row(
            "SELECT credential_hash FROM devices WHERE device_id = ?1",
            [device_id.to_string()],
            |row| row.get(0),
        )
        .expect("credential")
}

fn principal(value: u128) -> PrincipalId {
    PrincipalId::from_uuid(Uuid::from_u128(value))
}

fn store() -> (tempfile::TempDir, ControlStore) {
    let directory = tempfile::tempdir().expect("directory");
    let store = ControlStore::open(directory.path().join("control.sqlite3")).expect("store");
    (directory, store)
}

fn count(store: &ControlStore, sql: &str) -> i64 {
    open_connection(&store.path)
        .expect("connection")
        .query_row(sql, [], |row| row.get(0))
        .expect("count")
}

fn kind(result: Result<impl std::fmt::Debug, ControlError>) -> ControlErrorKind {
    result.expect_err("refused").kind()
}

#[test]
fn a_code_is_found_however_a_person_types_it() {
    let code = new_code().expect("code");
    assert_eq!(code.len(), CODE_LENGTH);
    assert!(code.bytes().all(|symbol| CODE_ALPHABET.contains(&symbol)));
    let digest = code_digest("0123ABCD").expect("digest");
    for typed in [
        "0123abcd",
        "0123-ABCD",
        " 0123 abcd ",
        "O123ABCD",
        "oI23ABCD",
        "0l23ABCD",
    ] {
        assert_eq!(code_digest(typed), Some(digest), "{typed}");
    }
    for typed in ["0123ABC", "0123ABCDE", "0123ABCU", "0123ABC\u{d0}", ""] {
        assert_eq!(code_digest(typed), None, "{typed}");
    }
}

#[tokio::test]
async fn an_approved_request_signs_in_only_the_keys_holder_and_converges_on_one_device() {
    let (_directory, store) = store();
    let (computer, owner, now) = (COMPUTER, principal(1), SystemTime::now());
    let request_id = Uuid::from_u128(FIRST);
    let opened = store
        .open_device_request(computer.request(request_id, "MacBook"), now)
        .await
        .expect("open");

    let waiting = store
        .collect_device_request(request_id, computer.proof(request_id), now)
        .await
        .expect("collect");
    assert!(
        matches!(waiting, Collected::Waiting { expires_at_ms } if expires_at_ms == opened.expires_at_ms)
    );
    let stranger = STRANGER;
    assert_eq!(
        kind(
            store
                .collect_device_request(request_id, stranger.proof(request_id), now)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::Authentication
    );

    let (surface, label) = approve(&store, owner, &opened.code, now)
        .await
        .expect("approve");
    assert_eq!((surface.as_str(), label.as_str()), ("mac", "MacBook"));
    approve(&store, owner, &opened.code.to_lowercase(), now)
        .await
        .expect("approving again succeeds");
    assert_eq!(
        kind(approve(&store, principal(2), &opened.code, now).await),
        ControlErrorKind::Conflict
    );

    let Collected::SignedIn(_, first) = store
        .collect_device_request(request_id, computer.proof(request_id), now)
        .await
        .expect("collect")
    else {
        panic!("an approved request signs the computer in");
    };
    assert!(!first.adopted);
    let Collected::SignedIn(_, second) = store
        .collect_device_request(request_id, computer.proof(request_id), now)
        .await
        .expect("collect again")
    else {
        panic!("an approved request signs the computer in again");
    };
    assert!(second.adopted);
    assert_eq!(first.credentials.device_id, second.credentials.device_id);
    assert_ne!(first.credentials.credential, second.credentials.credential);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM device_keys"), 1);
    let devices = store.list_devices(owner, None).await.expect("list");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].label, "MacBook");
}

#[tokio::test]
async fn asking_again_replaces_the_code_and_keeps_the_request() {
    let (_directory, store) = store();
    let (computer, now) = (COMPUTER, SystemTime::now());
    let request_id = Uuid::new_v4();
    let first = store
        .open_device_request(computer.request(request_id, "MacBook"), now)
        .await
        .expect("open");
    let later = now + Duration::from_secs(60);
    let mut second = store
        .open_device_request(computer.request(request_id, "MacBook"), later)
        .await
        .expect("open again");
    while second.code == first.code {
        second = store
            .open_device_request(computer.request(request_id, "MacBook"), later)
            .await
            .expect("open again");
    }
    assert_eq!(
        second.expires_at_ms, first.expires_at_ms,
        "asking again extends nothing"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM device_requests"), 1);
    assert_eq!(
        kind(approve(&store, principal(1), &first.code, later).await),
        ControlErrorKind::NotFound,
        "the replaced code is gone"
    );
    assert_eq!(
        kind(
            store
                .open_device_request(STRANGER.request(request_id, "MacBook"), later)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::Conflict,
        "another key cannot take the id"
    );
    assert_eq!(
        kind(
            store
                .open_device_request(computer.request(request_id, "Other name"), later)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::Conflict
    );
    approve(&store, principal(1), &second.code, later)
        .await
        .expect("approve");
    assert_eq!(
        kind(
            store
                .open_device_request(computer.request(request_id, "MacBook"), later)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::Conflict,
        "an approved request keeps its approval"
    );
}

#[tokio::test]
async fn a_request_ends_at_its_expiry_and_its_row_is_swept() {
    let (_directory, store) = store();
    let (computer, now) = (COMPUTER, SystemTime::now());
    let request_id = Uuid::from_u128(FIRST);
    let opened = store
        .open_device_request(computer.request(request_id, "MacBook"), now)
        .await
        .expect("open");
    let expired = now + REQUEST_LIFETIME;
    assert_eq!(
        kind(approve(&store, principal(1), &opened.code, expired).await),
        ControlErrorKind::NotFound
    );
    assert_eq!(
        kind(
            store
                .collect_device_request(request_id, computer.proof(request_id), expired)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::NotFound
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM device_requests"), 0);
}

#[tokio::test]
async fn waiting_requests_are_bounded_until_one_expires() {
    let (_directory, store) = store();
    let now = SystemTime::now();
    for seed in 0..MAX_ACTIVE_REQUESTS {
        let seed = u16::try_from(seed).expect("small");
        store
            .open_device_request(request_with_key(seed, Uuid::new_v4()), now)
            .await
            .expect("open");
    }
    let refused = store
        .open_device_request(request_with_key(1000, Uuid::new_v4()), now)
        .await
        .map(|_| ());
    assert_eq!(kind(refused), ControlErrorKind::Capacity);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM device_requests"),
        MAX_ACTIVE_REQUESTS
    );
    store
        .open_device_request(
            request_with_key(1001, Uuid::new_v4()),
            now + REQUEST_LIFETIME,
        )
        .await
        .expect("expired requests make room");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM device_requests"), 1);
}

#[tokio::test]
async fn a_refused_approval_leaves_the_request_waiting() {
    let (_directory, store) = store();
    let (computer, owner, now) = (COMPUTER, principal(1), SystemTime::now());
    let first = Uuid::from_u128(FIRST);
    let opened = store
        .open_device_request(computer.request(first, "MacBook"), now)
        .await
        .expect("open");
    approve(&store, owner, &opened.code, now)
        .await
        .expect("approve");
    let Collected::SignedIn(_, device) = store
        .collect_device_request(first, computer.proof(first), now)
        .await
        .expect("collect")
    else {
        panic!("signed in");
    };
    store
        .revoke_owned_device(owner, device.credentials.device_id)
        .await
        .expect("revoke");

    let again = Uuid::from_u128(SECOND);
    let opened = store
        .open_device_request(computer.request(again, "MacBook"), now)
        .await
        .expect("a revoked key may still ask");
    assert_eq!(
        kind(approve(&store, owner, &opened.code, now).await),
        ControlErrorKind::Conflict,
        "a revoked key is never signed in again"
    );
    assert!(matches!(
        store
            .collect_device_request(again, computer.proof(again), now)
            .await
            .expect("collect"),
        Collected::Waiting { .. }
    ));
    assert_eq!(
        kind(approve(&store, owner, "ABC", now).await),
        ControlErrorKind::Invalid
    );
}

#[test]
fn a_schema_17_database_gains_an_empty_request_table() {
    let (directory, store) = store();
    let connection = open_connection(&store.path).expect("connection");
    connection
        .execute_batch("DROP TABLE device_requests; PRAGMA user_version = 17;")
        .expect("a schema 17 database");
    drop(connection);
    let reopened = ControlStore::open(directory.path().join("control.sqlite3")).expect("reopen");
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM device_requests"), 0);
    assert_eq!(count(&reopened, "PRAGMA user_version"), 18);
}

#[tokio::test]
async fn signing_out_the_approving_app_withdraws_its_approval() {
    let (_directory, store) = store();
    let (computer, owner, now) = (COMPUTER, principal(1), SystemTime::now());
    let request_id = Uuid::from_u128(FIRST);
    let opened = store
        .open_device_request(computer.request(request_id, "MacBook"), now)
        .await
        .expect("open");
    let phone = signed_in_app(&store, owner, 1);
    approve_by(&store, owner, phone, &opened.code, now)
        .await
        .expect("approve");
    let other = store
        .open_device_request(request_with_key(9, Uuid::from_u128(SECOND)), now)
        .await
        .expect("open");
    approve(&store, owner, &other.code, now)
        .await
        .expect("another app approves another computer");
    store
        .revoke_owned_device(owner, phone)
        .await
        .expect("revoke");
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM device_requests WHERE principal_id IS NOT NULL"
        ),
        1,
        "only the signed-out app's approval is withdrawn"
    );
    assert!(
        matches!(
            store
                .collect_device_request(request_id, computer.proof(request_id), now)
                .await
                .expect("collect"),
            Collected::Waiting { .. }
        ),
        "the computer waits again"
    );
    let tablet = signed_in_app(&store, owner, 2);
    approve_by(&store, owner, tablet, &opened.code, now)
        .await
        .expect("another app approves");
    assert!(matches!(
        store
            .collect_device_request(request_id, computer.proof(request_id), now)
            .await
            .expect("collect"),
        Collected::SignedIn(principal_id, _) if principal_id == owner
    ));
}

#[tokio::test]
async fn a_principal_gets_five_codes_wrong_per_window_and_no_more() {
    let (_directory, store) = store();
    let (owner, now) = (principal(1), SystemTime::now());
    let opened = store
        .open_device_request(COMPUTER.request(Uuid::from_u128(FIRST), "MacBook"), now)
        .await
        .expect("open");
    let wrong = if opened.code == "ZZZZZZZZ" {
        "YYYYYYYY"
    } else {
        "ZZZZZZZZ"
    };
    for _ in 0..MAX_MISSES {
        assert_eq!(
            kind(approve(&store, owner, wrong, now).await),
            ControlErrorKind::NotFound
        );
    }
    assert_eq!(
        kind(approve(&store, owner, &opened.code, now).await),
        ControlErrorKind::Conflict,
        "even the right code waits"
    );
    assert_eq!(
        kind(approve(&store, principal(2), wrong, now).await),
        ControlErrorKind::NotFound,
        "another principal keeps its own count"
    );
    let later = now + REQUEST_LIFETIME;
    let reopened = store
        .open_device_request(COMPUTER.request(Uuid::from_u128(SECOND), "MacBook"), later)
        .await
        .expect("open");
    approve(&store, owner, &reopened.code, later)
        .await
        .expect("the misses expire with the window");
}

#[tokio::test]
async fn collecting_runs_the_sign_in_rules_again_and_a_wrong_proof_changes_nothing() {
    let (_directory, store) = store();
    let (computer, owner, now) = (COMPUTER, principal(1), SystemTime::now());
    let (first, second) = (Uuid::from_u128(FIRST), Uuid::from_u128(SECOND));
    for request_id in [first, second] {
        let opened = store
            .open_device_request(computer.request(request_id, "MacBook"), now)
            .await
            .expect("open");
        approve(&store, owner, &opened.code, now)
            .await
            .expect("approve");
    }
    let Collected::SignedIn(_, device) = store
        .collect_device_request(first, computer.proof(first), now)
        .await
        .expect("collect")
    else {
        panic!("signed in");
    };
    let device_id = device.credentials.device_id;
    let before = credential_hash(&store, device_id);
    assert_eq!(
        kind(
            store
                .collect_device_request(first, STRANGER.proof(first), now)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::Authentication
    );
    assert_eq!(
        credential_hash(&store, device_id),
        before,
        "nothing rotated"
    );

    store
        .revoke_owned_device(owner, device_id)
        .await
        .expect("revoke");
    assert_eq!(
        kind(
            store
                .collect_device_request(second, computer.proof(second), now)
                .await
                .map(|_| ())
        ),
        ControlErrorKind::Conflict,
        "a key signed out after its approval is refused when it collects"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM devices"), 1);
}

#[tokio::test]
async fn a_key_another_principal_holds_is_refused_at_approval() {
    let (_directory, store) = store();
    let now = SystemTime::now();
    let first = Uuid::from_u128(FIRST);
    let opened = store
        .open_device_request(COMPUTER.request(first, "MacBook"), now)
        .await
        .expect("open");
    approve(&store, principal(2), &opened.code, now)
        .await
        .expect("approve");
    store
        .collect_device_request(first, COMPUTER.proof(first), now)
        .await
        .expect("signed in as the other principal");
    let again = store
        .open_device_request(COMPUTER.request(Uuid::from_u128(SECOND), "MacBook"), now)
        .await
        .expect("open");
    assert_eq!(
        kind(approve(&store, principal(1), &again.code, now).await),
        ControlErrorKind::Conflict
    );
}
