use super::{process_liveness, process_start_key, ProcessLiveness};
use pretty_assertions::assert_eq;

#[test]
fn current_process_liveness_is_platform_correct() {
    let pid = std::process::id();
    let key = process_start_key(pid);
    let liveness = process_liveness(&key);
    #[cfg(target_os = "linux")]
    {
        assert_eq!(liveness, ProcessLiveness::Alive);
        assert!(
            key.contains(':'),
            "linux start_key must include starttime, got {key}"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        assert_eq!(liveness, ProcessLiveness::Unknown);
        assert!(
            !key.contains(':'),
            "non-linux start_key is pid-only so identity stays unknown, got {key}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_starttime_mismatch_is_dead_not_alive() {
    let pid = std::process::id();
    assert_eq!(
        process_liveness(&format!("{pid}:0")),
        ProcessLiveness::Dead,
        "wrong starttime must not be treated as the same process"
    );
    assert_eq!(
        process_liveness(&pid.to_string()),
        ProcessLiveness::Unknown,
        "pid-only keys cannot prove identity against reuse"
    );
    assert_eq!(process_liveness("999999:1"), ProcessLiveness::Dead);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_does_not_assume_proc_and_does_not_invent_death() {
    let pid = std::process::id();
    // Old code checked Path::new("/proc/{pid}"), which is missing on macOS
    // and Windows, so every live PID was reported dead and recovery committed -1.
    assert_eq!(process_liveness(&pid.to_string()), ProcessLiveness::Unknown);
    assert_eq!(
        process_liveness(&format!("{pid}:0")),
        ProcessLiveness::Unknown
    );
}

#[test]
fn missing_pid_is_dead() {
    assert_eq!(process_liveness("999999"), ProcessLiveness::Dead);
}
