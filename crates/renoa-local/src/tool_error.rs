use std::io;

use renoa_agent::ToolError;

pub(crate) fn io_error(
    action: &str,
    error: &io::Error,
    partial_changes_possible: bool,
) -> ToolError {
    let message = format!("cannot {action}: {error}");
    match error.kind() {
        io::ErrorKind::NotFound => ToolError::not_found(message),
        io::ErrorKind::PermissionDenied => ToolError::permission_denied(message),
        io::ErrorKind::AlreadyExists => ToolError::conflict(message),
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => {
            ToolError::invalid_input(message)
        }
        io::ErrorKind::TimedOut => ToolError::timeout(message, partial_changes_possible),
        _ => ToolError::io(message, partial_changes_possible),
    }
}
