use renoa_agent::ModelErrorKind;
use tokio::io::AsyncWriteExt as _;

use super::*;

#[tokio::test]
async fn first_output_deadline_is_enforced_on_the_real_record_reader() {
    let (_writer, reader) = tokio::io::duplex(256);
    let (sender, _receiver) = mpsc::channel(1);
    let now = tokio::time::Instant::now();

    let error = read_records(
        reader,
        &CancellationToken::new(),
        &sender,
        StreamDeadlines {
            first_output: now + Duration::from_millis(5),
            idle: Duration::from_millis(50),
            total: now + Duration::from_millis(100),
        },
    )
    .await
    .expect_err("silent stream must time out");

    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert!(error.to_string().contains("first-output"));
}

#[tokio::test]
async fn diagnostic_traffic_does_not_disable_the_idle_deadline() {
    let (mut writer, reader) = tokio::io::duplex(512);
    writer
        .write_all(b"{\"event\":\"provider_request\",\"payload\":{}}\n")
        .await
        .expect("write diagnostic record");
    let (sender, _receiver) = mpsc::channel(1);
    let now = tokio::time::Instant::now();

    let error = read_records(
        reader,
        &CancellationToken::new(),
        &sender,
        StreamDeadlines {
            first_output: now + Duration::from_millis(100),
            idle: Duration::from_millis(5),
            total: now + Duration::from_millis(200),
        },
    )
    .await
    .expect_err("idle stream must time out");

    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert!(error.to_string().contains("idle"));
}

#[tokio::test]
async fn total_deadline_wins_even_while_diagnostics_keep_arriving() {
    let (mut writer, reader) = tokio::io::duplex(2_048);
    let write = tokio::spawn(async move {
        loop {
            if writer
                .write_all(b"{\"event\":\"provider_response\",\"status\":200,\"headers\":{}}\n")
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    let (sender, mut receiver) = mpsc::channel(1);
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let now = tokio::time::Instant::now();

    let error = read_records(
        reader,
        &CancellationToken::new(),
        &sender,
        StreamDeadlines {
            first_output: now + Duration::from_millis(100),
            idle: Duration::from_millis(50),
            total: now + Duration::from_millis(10),
        },
    )
    .await
    .expect_err("busy stream must still hit its total deadline");
    drop(sender);
    write.await.expect("diagnostic writer");
    drain.await.expect("diagnostic drain");

    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert!(error.to_string().contains("total deadline"));
}
