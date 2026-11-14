//! What this computer is called: in the copy the owner reads ("This Mac",
//! "Renoa for Linux") and in the name it signs in with, which the phone shows.

/// The kind of computer, as in "this Mac" or "this PC".
#[cfg(target_os = "macos")]
pub const KIND: &str = "Mac";
#[cfg(target_os = "linux")]
pub const KIND: &str = "PC";

/// The platform this build is for, as in "Renoa for Mac".
#[cfg(target_os = "macos")]
pub const PLATFORM: &str = "Mac";
#[cfg(target_os = "linux")]
pub const PLATFORM: &str = "Linux";

/// The longest name a device may carry.
const MAX_LABEL_CHARS: usize = 64;

/// The name this computer signs in with: its name in the system's settings,
/// or the platform when it has none.
pub fn computer_name() -> String {
    label(system_name().as_deref()).unwrap_or_else(|| PLATFORM.to_owned())
}

/// `name` trimmed and cut to a device label, unless nothing is left.
fn label(name: Option<&str>) -> Option<String> {
    let label: String = name?.trim().chars().take(MAX_LABEL_CHARS).collect();
    (!label.is_empty()).then_some(label)
}

/// The Mac's name in System Settings.
#[cfg(target_os = "macos")]
fn system_name() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()
}

/// The pretty hostname Settings shows when one is set, else the hostname.
#[cfg(target_os = "linux")]
fn system_name() -> Option<String> {
    let read = |path| std::fs::read_to_string(path).ok();
    read("/etc/machine-info")
        .and_then(|info| pretty_hostname(&info))
        .or_else(|| label(read("/etc/hostname").as_deref()))
}

/// `PRETTY_HOSTNAME` from `/etc/machine-info`, an environment-style file
/// whose values may be quoted.
#[cfg(target_os = "linux")]
fn pretty_hostname(machine_info: &str) -> Option<String> {
    let value = machine_info
        .lines()
        .find_map(|line| line.trim().strip_prefix("PRETTY_HOSTNAME="))?;
    let unquoted = ['"', '\'']
        .into_iter()
        .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(value);
    label(Some(unquoted))
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn a_label_is_trimmed_cut_short_and_never_empty() {
        assert_eq!(label(Some("  Yash's PC\n")).as_deref(), Some("Yash's PC"));
        assert_eq!(label(Some(&"a".repeat(80))).map(|l| l.len()), Some(64));
        assert_eq!(label(Some(" \n")), None);
        assert_eq!(label(None), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_pretty_hostname_is_read_quoted_or_bare() {
        use super::pretty_hostname;
        let quoted = "CHASSIS=desktop\nPRETTY_HOSTNAME=\"Yash's PC\"\n";
        assert_eq!(pretty_hostname(quoted).as_deref(), Some("Yash's PC"));
        assert_eq!(
            pretty_hostname("PRETTY_HOSTNAME=Anrahya").as_deref(),
            Some("Anrahya")
        );
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME=\"\"\n"), None);
        assert_eq!(pretty_hostname("CHASSIS=laptop\n"), None);
    }
}
