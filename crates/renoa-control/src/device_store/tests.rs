use std::time::{Duration, SystemTime};

use renoa_protocol::{PrincipalId, SurfaceRef};
use tempfile::TempDir;
use uuid::Uuid;

use super::{MAX_DEVICES_PER_PRINCIPAL, SignedInDevice, sign_in_device};
use crate::{
    ControlError, DeviceId, PeerIdentity,
    control_schema::open_connection,
    coordinator::ControlErrorKind,
    device_key::{DevicePublicKey, DeviceRegistration},
    store::ControlStore,
};

fn principal(value: u128) -> PrincipalId {
    PrincipalId::from_uuid(Uuid::from_u128(value))
}

fn surface(name: &str) -> SurfaceRef {
    SurfaceRef::new(name)
}

/// A registration whose key is distinct per `seed`. Storage checks the shape of
/// a key, not that it lies on the curve; the sign-in proof does that.
fn registration(seed: u8) -> DeviceRegistration {
    let mut point = [0_u8; 65];
    point[0] = 0x04;
    point[1] = seed;
    DeviceRegistration {
        key: DevicePublicKey::from_bytes(&point).expect("key shape"),
        label: format!("Device {seed}"),
    }
}

struct Fixture {
    _files: TempDir,
    store: ControlStore,
}

impl Fixture {
    fn new() -> Self {
        let files = TempDir::new().expect("temporary directory");
        let store = ControlStore::open(files.path().join("control.sqlite")).expect("open store");
        Self {
            _files: files,
            store,
        }
    }

    fn sign_in(
        &self,
        owner: PrincipalId,
        surface_name: &str,
        registration: &DeviceRegistration,
    ) -> Result<SignedInDevice, ControlError> {
        let connection = open_connection(&self.store.path).expect("open control database");
        sign_in_device(&connection, owner, &surface(surface_name), registration, 1)
    }

    fn count(&self, table: &str) -> i64 {
        open_connection(&self.store.path)
            .expect("open control database")
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count rows")
    }
}

fn kind(result: Result<SignedInDevice, ControlError>) -> ControlErrorKind {
    result.expect_err("the sign-in is refused").kind()
}

#[tokio::test]
async fn signing_in_again_with_the_same_key_adopts_the_device_and_replaces_its_credential() {
    let fixture = Fixture::new();
    let key = registration(1);
    let first = fixture
        .sign_in(principal(1), "android", &key)
        .expect("first");
    let renamed = DeviceRegistration {
        label: "Pixel 9".to_owned(),
        ..key
    };
    let second = fixture
        .sign_in(principal(1), "android", &renamed)
        .expect("a repeat sign-in adopts the device");

    assert!(!first.adopted);
    assert!(second.adopted);
    assert_eq!(first.credentials.device_id, second.credentials.device_id);
    assert_ne!(first.credentials.credential, second.credentials.credential);
    assert_eq!(fixture.count("devices"), 1, "no second device row");
    assert_eq!(fixture.count("device_keys"), 1, "no second key row");

    fixture
        .store
        .authenticate_device(first.credentials)
        .await
        .expect_err("the credential the adopted sign-in replaced no longer works");
    let device = fixture
        .store
        .authenticate_device(second.credentials)
        .await
        .expect("the new credential works");
    assert_eq!(device.key, Some(renamed.key));
    let listed = fixture
        .store
        .list_devices(principal(1), None)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].label, "Pixel 9");
}

#[tokio::test]
async fn a_device_accepts_only_the_credential_it_holds_until_it_is_revoked() {
    let fixture = Fixture::new();
    let key = registration(1);
    let first = fixture
        .sign_in(principal(1), "android", &key)
        .expect("first");
    let device_id = first.credentials.device_id;
    let first_credential = first.credentials.credential.digest().expect("digest");
    let second = fixture
        .sign_in(principal(1), "android", &key)
        .expect("a repeat sign-in replaces the credential");
    let second_credential = second.credentials.credential.digest().expect("digest");

    assert!(
        !fixture
            .store
            .device_accepts(device_id, first_credential)
            .await
            .expect("accepts"),
        "a connection authenticated with the replaced credential is not activated"
    );
    assert!(
        fixture
            .store
            .device_accepts(device_id, second_credential)
            .await
            .expect("accepts")
    );
    fixture
        .store
        .revoke_owned_device(principal(1), device_id)
        .await
        .expect("revoke");
    assert!(
        !fixture
            .store
            .device_accepts(device_id, second_credential)
            .await
            .expect("accepts"),
        "a revoked device accepts no credential"
    );
}

#[tokio::test]
async fn a_revoked_key_and_a_key_of_another_sign_in_are_conflicts_that_write_nothing() {
    let fixture = Fixture::new();
    let key = registration(1);
    let device = fixture
        .sign_in(principal(1), "android", &key)
        .expect("first");
    let (devices, keys) = (fixture.count("devices"), fixture.count("device_keys"));

    assert_eq!(
        kind(fixture.sign_in(principal(2), "android", &key)),
        ControlErrorKind::Conflict,
        "another principal cannot take the key"
    );
    assert_eq!(
        kind(fixture.sign_in(principal(1), "desktop", &key)),
        ControlErrorKind::Conflict,
        "the same principal cannot move the key to another surface"
    );
    fixture
        .store
        .revoke_owned_device(principal(1), device.credentials.device_id)
        .await
        .expect("revoke");
    assert_eq!(
        kind(fixture.sign_in(principal(1), "android", &key)),
        ControlErrorKind::Conflict,
        "a revoked key is never revived by a sign-in"
    );
    assert_eq!(
        (fixture.count("devices"), fixture.count("device_keys")),
        (devices, keys)
    );
    fixture
        .store
        .authenticate_device(device.credentials)
        .await
        .expect_err("the revoked device stays revoked");
    fixture
        .store
        .check_device_registration(principal(1), surface("android"), key)
        .await
        .expect_err("the pre-check refuses the same sign-in without writing");
}

#[test]
fn the_cap_counts_active_devices_only() {
    let fixture = Fixture::new();
    let cap = u8::try_from(MAX_DEVICES_PER_PRINCIPAL).expect("cap fits in a byte");
    let mut first = None;
    for seed in 0..cap {
        let device = fixture
            .sign_in(principal(1), "android", &registration(seed))
            .expect("within the cap");
        first.get_or_insert(device.credentials.device_id);
    }
    assert_eq!(
        kind(fixture.sign_in(principal(1), "android", &registration(cap))),
        ControlErrorKind::Conflict
    );
    fixture
        .sign_in(principal(2), "android", &registration(cap))
        .expect("the cap is per principal");

    let connection = open_connection(&fixture.store.path).expect("open control database");
    connection
        .execute(
            "UPDATE devices SET revoked = 1 WHERE device_id = ?1",
            [first.expect("a first device").to_string()],
        )
        .expect("revoke one");
    fixture
        .sign_in(principal(1), "android", &registration(cap + 1))
        .expect("a revoked device frees its place");
}

#[tokio::test]
async fn devices_are_listed_and_revoked_only_by_their_own_principal() {
    let fixture = Fixture::new();
    let mine = fixture
        .sign_in(principal(1), "android", &registration(1))
        .expect("mine")
        .credentials
        .device_id;
    let other_sign_in = fixture
        .sign_in(principal(2), "android", &registration(2))
        .expect("other");
    let other = other_sign_in.credentials.device_id;
    let other_credential = other_sign_in
        .credentials
        .credential
        .digest()
        .expect("digest");

    let listed = fixture
        .store
        .list_devices(principal(1), Some(mine))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].device_id, mine);
    assert!(listed[0].current);
    assert_eq!(listed[0].surface, surface("android"));
    let not_current = fixture
        .store
        .list_devices(principal(1), Some(other))
        .await
        .expect("list");
    assert!(!not_current[0].current);

    let refused = fixture
        .store
        .revoke_owned_device(principal(1), other)
        .await
        .expect_err("another principal's device is not found");
    assert_eq!(refused.kind(), ControlErrorKind::NotFound);
    let unknown = fixture
        .store
        .revoke_owned_device(principal(1), DeviceId::from_uuid(Uuid::from_u128(99)))
        .await
        .expect_err("an unknown device is not found");
    assert_eq!(unknown.kind(), ControlErrorKind::NotFound);
    assert!(
        fixture
            .store
            .device_accepts(other, other_credential)
            .await
            .expect("accepts")
    );

    assert!(
        fixture
            .store
            .revoke_owned_device(principal(1), mine)
            .await
            .expect("revoke")
    );
    assert!(
        !fixture
            .store
            .revoke_owned_device(principal(1), mine)
            .await
            .expect("a retry succeeds"),
        "the retry changes nothing"
    );
    assert!(
        fixture
            .store
            .list_devices(principal(1), None)
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn a_schema_12_database_upgrades_in_place_and_keeps_its_devices() {
    let fixture = Fixture::new();
    let owner = principal(1);
    let token = fixture
        .store
        .create_enrollment(
            PeerIdentity::Surface {
                principal_id: owner,
                surface: surface("discord"),
            },
            SystemTime::now() + Duration::from_mins(5),
        )
        .await
        .expect("enrollment");
    let credentials = fixture.store.claim_enrollment(token).await.expect("claim");
    open_connection(&fixture.store.path)
        .expect("open control database")
        .execute_batch(
            "DROP TABLE device_keys; DROP TABLE device_sign_ins; PRAGMA user_version = 12;",
        )
        .expect("rewind to schema 12");

    let upgraded = ControlStore::open(fixture.store.path.as_ref()).expect("upgrade");

    let connection = open_connection(&upgraded.path).expect("open control database");
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 13);
    let device = upgraded
        .authenticate_device(credentials)
        .await
        .expect("a device from schema 12 still authenticates");
    assert_eq!(device.key, None, "it stays a bearer-only device");
    upgraded
        .list_devices(owner, None)
        .await
        .expect("the new tables are usable");
}
