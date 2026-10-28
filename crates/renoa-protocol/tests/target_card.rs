//! A node builds every card it advertises with `TargetCard::new`, because the
//! coordinator rejects a whole advertisement when one card is unclean.

use renoa_protocol::{TargetCard, TargetRef};

fn target() -> TargetRef {
    TargetRef::new("agent:research")
}

#[test]
fn a_clean_name_is_kept_and_surrounding_space_is_trimmed() {
    assert_eq!(TargetCard::new(target(), "Research").name, "Research");
    assert_eq!(TargetCard::new(target(), "  X Desk \t").name, "X Desk");
}

#[test]
fn a_control_character_becomes_a_space() {
    let card = TargetCard::new(target(), "two\nlines\u{7}here");
    assert_eq!(card.name, "two lines here");
    assert!(TargetCard::label_is_valid(&card.name));
}

#[test]
fn an_overlong_name_is_cut_at_a_character_boundary() {
    // 300 two-byte characters: the cut lands inside none of them.
    let card = TargetCard::new(target(), &"é".repeat(300));
    assert_eq!(card.name.len(), TargetCard::MAX_LABEL_BYTES);
    assert_eq!(card.name, "é".repeat(TargetCard::MAX_LABEL_BYTES / 2));

    // 255 bytes then a two-byte character: the character does not fit, so it
    // is dropped whole.
    let card = TargetCard::new(target(), &format!("{}é", "n".repeat(255)));
    assert_eq!(card.name, "n".repeat(255));
    assert!(TargetCard::label_is_valid(&card.name));
}

#[test]
fn a_blank_name_is_the_target_itself() {
    for name in ["", "   ", "\n\t", "\u{0}\u{1}"] {
        assert_eq!(
            TargetCard::new(target(), name).name,
            "agent:research",
            "{name:?}"
        );
    }
}

#[test]
fn every_card_the_constructor_makes_is_a_valid_label() {
    let long = "x".repeat(1_000);
    for name in ["", "  ", "Research", "a\nb", "é", long.as_str()] {
        let card = TargetCard::new(target(), name);
        assert!(TargetCard::label_is_valid(&card.name), "{name:?}");
        assert!(TargetCard::label_is_valid(card.target.as_str()), "{name:?}");
    }
}
