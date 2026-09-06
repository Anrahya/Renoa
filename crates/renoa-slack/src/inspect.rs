use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use crate::{Config, SlackError};

/// Reads retained requests and delivery outcomes without taking the daemon lease.
///
/// # Errors
/// Returns missing, incompatible, or unreadable surface storage.
pub fn inspect(config: &Config) -> Result<Value, SlackError> {
    let connection = Connection::open_with_flags(
        config.data_directory.join("slack.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        return Err(SlackError::Invalid(format!(
            "unsupported Slack schema {version}"
        )));
    }
    let mut query=connection.prepare("SELECT request_id,channel,thread,session_id,state,cancel_requested,result,reply_state FROM requests ORDER BY seq DESC LIMIT 20")?;
    let requests=query.query_map([],|row|Ok(json!({
        "request_id":row.get::<_,String>(0)?,"channel":row.get::<_,String>(1)?,"thread":row.get::<_,String>(2)?,
        "session_id":row.get::<_,String>(3)?,"state":row.get::<_,String>(4)?,"cancel_requested":row.get::<_,bool>(5)?,
        "result":row.get::<_,Option<String>>(6)?,"progress_delivery":row.get::<_,String>(7)?
    })))?.collect::<Result<Vec<_>,_>>()?;
    let mut query=connection.prepare("SELECT r.request_id,d.chunk,CASE WHEN d.state='pending' THEN 'blocked' ELSE d.state END,d.error,d.text FROM deliveries d JOIN requests r ON r.seq=d.request_seq WHERE d.state IN('unknown','failed') OR (d.state='pending' AND EXISTS(SELECT 1 FROM deliveries prior WHERE prior.request_seq=d.request_seq AND prior.chunk<d.chunk AND prior.state IN('unknown','failed'))) ORDER BY r.seq DESC,d.chunk LIMIT 100")?;
    let failures=query.query_map([],|row|Ok(json!({
        "request_id":row.get::<_,String>(0)?,"chunk":row.get::<_,i64>(1)?,"state":row.get::<_,String>(2)?,
        "error":row.get::<_,Option<String>>(3)?,"text":row.get::<_,String>(4)?
    })))?.collect::<Result<Vec<_>,_>>()?;
    Ok(json!({"requests":requests,"delivery_problems":failures}))
}
