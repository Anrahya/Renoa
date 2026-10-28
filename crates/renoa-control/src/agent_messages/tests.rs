use uuid::Uuid;

use super::derived_id;

/// The Host derives its identities the same way (`renoa_local::stable_id`)
/// and pins the same vector, so neither copy can change alone: thread tasks
/// and answers already stored keep the identities they were derived under.
#[test]
fn a_derived_identity_is_the_pinned_digest_of_its_domain_value() {
    assert_eq!(
        derived_id("renoa.test.vector.v1:pinned"),
        Uuid::parse_str("21c4becb-66ac-d5f6-757c-8bef3a727439").expect("uuid")
    );
}
