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
    /// Refused, in conflict, or not understood: worth asking again later.
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
    match answer.status {
        200..=299 => Answer::Said(answer.body),
        404 => Answer::Missing,
        _ => Answer::Later,
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::Answer;

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
}
