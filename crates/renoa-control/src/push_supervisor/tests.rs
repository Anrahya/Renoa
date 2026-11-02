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
    coordinator::CoordinatorState,
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

/// A token endpoint that answers, and an FCM that records when each wake
/// arrived and never answers it.
async fn google() -> (Url, watch::Receiver<Vec<Instant>>) {
    async fn answer(State(sent): State<Arc<watch::Sender<Vec<Instant>>>>, uri: Uri) -> String {
        if uri.path() == "/token" {
            return serde_json::json!({"access_token": "ya29.unit", "expires_in": 3599})
                .to_string();
        }
        sent.send_modify(|sent| sent.push(Instant::now()));
        std::future::pending().await
    }
    let (sent, seen) = watch::channel(Vec::new());
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

/// Enrolls a phone with a push token and owes its principal a wake.
async fn owe_a_phone_a_wake(state: &CoordinatorState) {
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
}

#[tokio::test]
async fn a_send_cut_short_by_a_dying_dispatcher_ends_its_gap_after_the_restart() {
    // Longer than the one-second restart delay, so a send right after the
    // restart is told apart from one after the gap.
    let gap = Duration::from_secs(3);
    let (origin, seen) = google().await;
    let files = TempDir::new().expect("temporary directory");
    let settings = crate::google_access::tests::push_settings(files.path())
        .with_google_endpoints(origin.join("/token").expect("token endpoint"), origin)
        .with_device_gap(gap);
    let coordinator =
        Coordinator::open(files.path().join("control.sqlite")).expect("open the coordinator");
    let state = coordinator.state();
    owe_a_phone_a_wake(&state).await;
    let delivery = Delivery::Fcm {
        sender: Arc::new(FcmSender::new(&settings).expect("build the sender")),
        gap,
    };

    let died: Arc<Mutex<Option<Instant>>> = Arc::default();
    let starts = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let supervisor = tokio::spawn(supervise(
        {
            let state = Arc::clone(&state);
            let died = Arc::clone(&died);
            let seen = seen.clone();
            let stop = stop.clone();
            move |pacing| {
                let run = dispatch(Arc::clone(&state), delivery.clone(), pacing, stop.clone());
                if starts.fetch_add(1, Ordering::SeqCst) > 0 {
                    return tokio::spawn(run);
                }
                let mut seen = seen.clone();
                let state = Arc::clone(&state);
                let died = Arc::clone(&died);
                tokio::spawn(async move {
                    let outlived = async {
                        let _ = seen.wait_for(|sent| sent.len() == 1).await;
                        tokio::time::sleep(gap).await;
                        // A pass while the send is in flight past its gap.
                        state.push_wakes.notify_one();
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    };
                    tokio::select! {
                        () = run => {}
                        () = outlived => {
                            *died.lock().expect("death") = Some(Instant::now());
                            panic!("the dispatcher dies with its send in flight");
                        }
                    }
                })
            }
        },
        stop.clone(),
    ));
    let mut arrivals = seen.clone();
    // The death comes a gap after the first send, the restart a second
    // later, and the second send a gap after that.
    let sent = timeout(WAIT * 2, arrivals.wait_for(|sent| sent.len() == 2))
        .await
        .expect("the restarted dispatcher sends again")
        .expect("the stub is alive")
        .clone();
    let died = died
        .lock()
        .expect("death")
        .expect("the first dispatcher died");
    assert!(
        sent[1] >= died + gap,
        "the device waits a full gap after the send that was cut short: {:?}",
        sent[1] - died
    );
    let owed = state.store.owed_wakes().await.expect("read owed wakes");
    assert_eq!(
        owed.len(),
        1,
        "a wake whose sends never ended is still owed"
    );
    stop.cancel();
    supervisor.await.expect("the supervisor returns");
}
