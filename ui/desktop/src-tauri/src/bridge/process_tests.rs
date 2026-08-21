use nix::{errno::Errno, sys::signal::kill, unistd::Pid};

use super::*;

#[test]
fn stopping_a_child_reaps_it_and_clears_shared_ownership() {
    let mut command = Command::new("sleep");
    command.arg("30");
    configure_process_group(&mut command);
    let process = command.spawn().expect("start sleeping child");
    let child = Arc::new(Mutex::new(Some(process)));

    stop_child(&child).expect("stop child");

    assert!(child.lock().expect("lock child state").is_none());
}

#[test]
fn stopping_a_child_also_stops_its_descendants() {
    let mut command = Command::new("sh");
    command
        .args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(Stdio::piped());
    configure_process_group(&mut command);
    let mut process = command.spawn().expect("start process tree");
    let descendant = read_descendant_pid(&mut process);
    let child = Arc::new(Mutex::new(Some(process)));

    stop_child(&child).expect("stop process tree");

    assert_process_gone(descendant, "descendant survived process-group cleanup");
}

#[test]
fn cleanup_stops_descendants_after_the_group_leader_exits() {
    let mut command = Command::new("sh");
    command
        .args(["-c", "sleep 30 & echo $!; exit 0"])
        .stdout(Stdio::piped());
    configure_process_group(&mut command);
    let mut process = command.spawn().expect("start process tree");
    let descendant = read_descendant_pid(&mut process);
    process.wait().expect("reap exited group leader");

    terminate_child(&mut process).expect("clean process group after leader exit");

    assert_process_gone(descendant, "orphaned descendant survived cleanup");
}

fn read_descendant_pid(process: &mut Child) -> i32 {
    let mut pid = String::new();
    BufReader::new(process.stdout.take().expect("process stdout"))
        .read_line(&mut pid)
        .expect("read descendant pid");
    pid.trim().parse().expect("numeric descendant pid")
}

fn assert_process_gone(pid: i32, message: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match kill(Pid::from_raw(pid), None) {
            Err(Errno::ESRCH) => break,
            _ if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => panic!("{message}: {result:?}"),
        }
    }
}
