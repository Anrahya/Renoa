//! Calls to the owner's Host, for the session's own reads and for the views.

use std::time::Duration;

use renoa_rcp_client::{ClientError, Connection};
use renoa_rcp_wire::HostMethod;
use serde::de::DeserializeOwned;
use tokio::{sync::oneshot, time::timeout};

/// How long a Host call may go unanswered. The coordinator answers within
/// 60 seconds whether or not the Host does (`HOST_CALL_TIMEOUT` in
/// renoa-control), so this waits longer, as the Android app's `hostReply`
/// does.
const HOST_REPLY_WITHIN: Duration = Duration::from_secs(75);

/// How the Host answered a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    Said(T),
    /// There is no such thing.
    Missing,
    /// At odds with how things stand: what was asked is already under way,
    /// or changed meanwhile.
    Conflict,
    /// Refused or not understood: worth asking again later.
    Later,
    /// The call failed or went unanswered, or the session is gone.
    Lost,
}

impl Answer<String> {
    /// The answer's body read as a `T`; a body that does not read is
    /// [`Answer::Later`].
    pub fn read<T: DeserializeOwned>(self) -> Answer<T> {
        match self {
            Self::Said(body) => serde_json::from_str(&body).map_or(Answer::Later, Answer::Said),
            Self::Missing => Answer::Missing,
            Self::Conflict => Answer::Conflict,
            Self::Later => Answer::Later,
            Self::Lost => Answer::Lost,
        }
    }
}

/// A Host call a view asks the session to make, answered on `answered`.
#[derive(Debug)]
pub struct HostCall {
    pub method: HostMethod,
    pub path: String,
    pub body: Option<String>,
    pub answered: oneshot::Sender<Answer<String>>,
}

/// Calls the Host over `connection` and says how it answered.
pub(super) async fn ask(
    connection: &Connection,
    method: HostMethod,
    path: &str,
    body: Option<String>,
) -> Answer<String> {
    let called = timeout(HOST_REPLY_WITHIN, connection.call_host(method, path, body)).await;
    let answer = match called {
        Ok(Ok(answer)) => answer,
        Ok(Err(ClientError::Rejected { .. } | ClientError::Protocol(_))) => return Answer::Later,
        Ok(Err(_)) | Err(_) => return Answer::Lost,
    };
    by_status(answer.status, answer.body)
}

/// How the Host answered, by its HTTP status.
fn by_status(status: u16, body: String) -> Answer<String> {
    match status {
        200..=299 => Answer::Said(body),
        404 => Answer::Missing,
        409 => Answer::Conflict,
        _ => Answer::Later,
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{Answer, by_status};

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    struct Count {
        n: u8,
    }

    #[test]
    fn a_body_reads_as_its_type_and_one_that_does_not_is_asked_again() {
        assert_eq!(
            Answer::Said(r#"{"n":3}"#.to_owned()).read(),
            Answer::Said(Count { n: 3 })
        );
        assert_eq!(Answer::Said("{}".to_owned()).read::<Count>(), Answer::Later);
        assert_eq!(Answer::<String>::Missing.read::<Count>(), Answer::Missing);
        assert_eq!(Answer::<String>::Lost.read::<Count>(), Answer::Lost);
    }

    #[test]
    fn a_status_says_how_the_host_answered() {
        let body = || "{}".to_owned();
        assert_eq!(by_status(201, body()), Answer::Said(body()));
        assert_eq!(by_status(404, body()), Answer::Missing);
        assert_eq!(by_status(409, body()), Answer::Conflict);
        assert_eq!(by_status(503, body()), Answer::Later);
        assert_eq!(by_status(400, body()), Answer::Later);
    }
}
