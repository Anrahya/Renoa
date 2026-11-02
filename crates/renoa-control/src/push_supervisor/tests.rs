use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use axum::{Router, extract::State, http::Uri};
use renoa_protocol::{PrincipalId, SurfaceRef};
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::watch,
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{HEALTHY_RUN, MAX_RESTART_DELAY, restart_delay, supervise};
use crate::{
    Coordinator, PeerIdentity,
    control_schema::open_connection,
    fcm::FcmSender,
    push_dispatch::{Delivery, dispatch},
};

const WAIT: Duration = Duration::from_secs(10);

async fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_dispatcher_that_dies_is_started_again_until_shutdown() {
    let starts = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let supervisor = tokio::spawn(supervise(
        {
            let starts = Arc::clone(&starts);
            let stop = stop.clone();
            move |_| {
                let first = starts.fetch_add(1, Ordering::SeqCst) == 0;
                let stop = stop.clone();
                tokio::spawn(async move {
                    assert!(!first, "the first dispatcher dies");
                    stop.cancelled().await;
                })
            }
        },
        stop.clone(),
    ));
    eventually("a second dispatcher starts", || {
        starts.load(Ordering::SeqCst) == 2
    })
    .await;
    stop.cancel();
    timeout(WAIT, supervisor)
        .await
        .expect("the supervisor returns at shutdown")
        .expect("the supervisor does not panic");
    assert_eq!(
        starts.load(Ordering::SeqCst),
        2,
        "none starts after shutdown"
    );
}

#[tokio::test(start_paused = true)]
async fn restarts_back_off_until_a_healthy_run_resets_them() {
    let starts: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let stop = CancellationToken::new();
    let supervisor = tokio::spawn(supervise(
        {
            let starts = Arc::clone(&starts);
            let stop = stop.clone();
            move |_| {
                let mut starts = starts.lock().expect("starts");
                starts.push(Instant::now());
                let started = starts.len();
                let stop = stop.clone();
                tokio::spawn(async move {
                    match started {
                        // Five dispatchers that die at once, then one that
                        // runs healthy first, then one that stays up.
                        1..=5 => {}
                        6 => tokio::time::sleep(HEALTHY_RUN).await,
                        _ => stop.cancelled().await,
                    }
                })
            }
        },
        stop.clone(),
    ));
    for _ in 0..10_000 {
        if starts.lock().expect("starts").len() == 7 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let starts = starts.lock().expect("starts").clone();
    let gaps: Vec<Duration> = starts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let seconds = Duration::from_secs;
    assert_eq!(
        gaps,
        [
            seconds(1),
            seconds(2),
            seconds(4),
            seconds(8),
            seconds(16),
            HEALTHY_RUN + seconds(1),
        ]
    );
    assert_eq!(
        restart_delay(7),
        MAX_RESTART_DELAY,
        "the delay stops at a minute"
    );
    assert_eq!(restart_delay(u32::MAX), MAX_RESTART_DELAY);
    stop.cancel();
    supervisor.await.expect("the supervisor returns");
}

/// Set when the task holding it is dropped, as an aborted task is.
struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_the_supervisor_aborts_its_dispatcher() {
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let supervisor = tokio::spawn(supervise(
        {
            let started = Arc::clone(&started);
            let dropped = Arc::clone(&dropped);
            move |_| {
                let started = Arc::clone(&started);
                let held = Dropped(Arc::clone(&dropped));
                tokio::spawn(async move {
                    let _held = held;
                    started.store(true, Ordering::SeqCst);
                    // A dispatcher that never sees a shutdown.
                    std::future::pending::<()>().await;
                })
            }
        },
        CancellationToken::new(),
    ));
    eventually("the dispatcher starts", || started.load(Ordering::SeqCst)).await;
    supervisor.abort();
    let _ = supervisor.await;
    eventually("the dispatcher is aborted with its supervisor", || {
        dropped.load(Ordering::SeqCst)
    })
    .await;
}

/// A token endpoint that answers, and an FCM that counts each wake and never
/// answers it.
async fn google() -> (Url, watch::Receiver<usize>) {
    async fn answer(State(sent): State<Arc<watch::Sender<usize>>>, uri: Uri) -> String {
        if uri.path() == "/token" {
            return serde_json::json!({"access_token": "ya29.unit", "expires_in": 3599})
                .to_string();
        }
        sent.send_modify(|sent| *sent += 1);
        std::future::pending().await
    }
    let (sent, seen) = watch::channel(0);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stub listener");
    let origin = Url::parse(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ))
    .expect("stub origin");
    let app = Router::new().fallback(answer).with_state(Arc::new(sent));
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve the stub") });
    (origin, seen)
}

#[tokio::test]
async fn a_dispatcher_that_panics_mid_send_leaves_the_device_its_gap() {
    let (origin, seen) = google().await;
    let observed = seen.clone();
    let files = TempDir::new().expect("temporary directory");
    let settings = crate::google_access::tests::push_settings(files.path())
        .with_google_endpoints(origin.join("/token").expect("token endpoint"), origin);
    let coordinator =
        Coordinator::open(files.path().join("control.sqlite")).expect("open the coordinator");
    let state = coordinator.state();
    let owner = PrincipalId::new();
    let enrollment = state
        .store
        .create_enrollment(
            PeerIdentity::Surface {
                principal_id: owner,
                surface: SurfaceRef::new("android"),
            },
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("create an enrollment");
    let device = state
        .store
        .claim_enrollment(enrollment)
        .await
        .expect("enroll the phone")
        .device_id;
    let database = open_connection(&state.store.path).expect("open the database");
    database
        .execute(
            "INSERT INTO push_tokens (device_id, principal_id, provider, token)
             VALUES (?1, ?2, 'fcm', 'fcm:phone')",
            [device.to_string(), owner.to_string()],
        )
        .expect("register the phone's token");
    database
        .execute(
            "INSERT INTO push_wakes (principal_id, generation) VALUES (?1, 1)",
            [owner.to_string()],
        )
        .expect("owe the owner a wake");
    let delivery = Delivery::Fcm {
        sender: Arc::new(FcmSender::new(&settings).expect("build the sender")),
        gap: settings.device_gap(),
    };

    let starts = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let supervisor = tokio::spawn(supervise(
        {
            let starts = Arc::clone(&starts);
            let stop = stop.clone();
            move |last_sent| {
                let run = dispatch(
                    Arc::clone(&state),
                    delivery.clone(),
                    last_sent,
                    stop.clone(),
                );
                if starts.fetch_add(1, Ordering::SeqCst) > 0 {
                    return tokio::spawn(run);
                }
                let mut seen = seen.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        () = run => {}
                        _ = seen.wait_for(|sent| *sent == 1) => {
                            panic!("the dispatcher dies with its send in flight");
                        }
                    }
                })
            }
        },
        stop.clone(),
    ));
    eventually("a second dispatcher starts after the panic", || {
        starts.load(Ordering::SeqCst) == 2
    })
    .await;
    // Long enough for the new dispatcher's first pass.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let owed: i64 = database
        .query_row("SELECT count(*) FROM push_wakes", [], |row| row.get(0))
        .expect("count owed wakes");
    assert_eq!(owed, 1, "the interrupted wake is still owed");
    assert_eq!(
        *observed.borrow(),
        1,
        "the restarted dispatcher waits out the device's gap"
    );
    stop.cancel();
    supervisor.await.expect("the supervisor returns");
}
