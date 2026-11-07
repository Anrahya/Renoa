//! The headers a shared file is served under.

use super::{media_type, percent_encoded};

#[test]
fn a_type_is_passed_on_only_when_plain_and_text_is_read_as_utf8() {
    for (named, sent) in [
        ("text/html", "text/html; charset=utf-8"),
        ("image/svg+xml", "image/svg+xml"),
        ("application/pdf", "application/pdf"),
        ("text/html; charset=latin1", "application/octet-stream"),
        ("Text/HTML", "application/octet-stream"),
        ("text/", "application/octet-stream"),
        ("html", "application/octet-stream"),
        ("text/html\r\nset-cookie: x", "application/octet-stream"),
    ] {
        assert_eq!(media_type(named), sent, "{named:?}");
    }
}

#[test]
fn a_file_name_cannot_break_out_of_its_header() {
    assert_eq!(percent_encoded("my page.html"), "my%20page.html");
    assert_eq!(percent_encoded("a\r\nb;\"c'é"), "a%0D%0Ab%3B%22c%27%C3%A9");
    assert_eq!(percent_encoded("A-z_0.~"), "A-z_0.~");
}
