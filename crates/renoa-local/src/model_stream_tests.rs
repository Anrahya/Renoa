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

#[tokio::test]
async fn cancellation_after_a_terminal_success_keeps_that_result() {
    let (mut writer, reader) = tokio::io::duplex(1_024);
    writer
        .write_all(
            br#"{"event":"completed","response":{"content":[{"type":"text","text":"definite"}],"stop_reason":"stop","usage":null,"metadata":{}}}
"#,
        )
        .await
        .expect("write terminal success");
    writer.flush().await.expect("flush terminal success");
    let (sender, _receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    let now = tokio::time::Instant::now();
    let read = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            read_records(
                reader,
                &cancellation,
                &sender,
                StreamDeadlines {
                    first_output: now + Duration::from_secs(2),
                    idle: Duration::from_secs(2),
                    total: now + Duration::from_secs(2),
                },
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancellation.cancel();
    let exit = read
        .await
        .expect("join reader")
        .expect("terminal must survive");
    match exit {
        ReadExit::Finished(terminal) => {
            let response = terminal.expect("success record");
            assert_eq!(
                response.content,
                vec![renoa_agent::AssistantContent::text("definite")]
            );
        }
        other => panic!("expected finished terminal, got {other:?}"),
    }
}

#[tokio::test]
async fn cancellation_after_a_terminal_error_keeps_that_error() {
    let (mut writer, reader) = tokio::io::duplex(1_024);
    writer
        .write_all(
            br#"{"event":"error","error":"structured invalid request","error_kind":"invalid_request","inference_outcome":"known_not_started"}
"#,
        )
        .await
        .expect("write terminal error");
    writer.flush().await.expect("flush terminal error");
    let (sender, _receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    let now = tokio::time::Instant::now();
    let read = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            read_records(
                reader,
                &cancellation,
                &sender,
                StreamDeadlines {
                    first_output: now + Duration::from_secs(2),
                    idle: Duration::from_secs(2),
                    total: now + Duration::from_secs(2),
                },
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancellation.cancel();
    let exit = read
        .await
        .expect("join reader")
        .expect("terminal must survive");
    match exit {
        ReadExit::Finished(terminal) => {
            let error = terminal.expect_err("error record");
            assert_eq!(error.kind(), ModelErrorKind::InvalidRequest);
            assert!(error.to_string().contains("structured invalid request"));
        }
        other => panic!("expected finished terminal error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_terminal_record_finishes_without_waiting_for_eof() {
    let (mut writer, reader) = tokio::io::duplex(1_024);
    writer
        .write_all(
            br#"{"event":"completed","response":{"content":[{"type":"text","text":"done"}],"stop_reason":"stop","usage":null,"metadata":{}}}
"#,
        )
        .await
        .expect("write terminal success");
    writer.flush().await.expect("flush terminal success");
    let (sender, _receiver) = mpsc::channel(1);
    let now = tokio::time::Instant::now();
    let exit = tokio::time::timeout(
        Duration::from_millis(200),
        read_records(
            reader,
            &CancellationToken::new(),
            &sender,
            StreamDeadlines {
                first_output: now + Duration::from_secs(2),
                idle: Duration::from_millis(30),
                total: now + Duration::from_secs(2),
            },
        ),
    )
    .await
    .expect("terminal must finish without waiting for EOF or idle timeout")
    .expect("terminal must succeed");
    // Keep the writer alive so stdout never reaches EOF.
    match exit {
        ReadExit::Finished(terminal) => {
            let response = terminal.expect("success record");
            assert_eq!(
                response.content,
                vec![renoa_agent::AssistantContent::text("done")]
            );
        }
        other => panic!("expected finished terminal, got {other:?}"),
    }
    drop(writer);
}
