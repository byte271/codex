use super::command_allowed;

fn allow(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn allowlist_matches_executable_boundary_only() {
    let echo = allow(&["echo"]);
    assert!(command_allowed("echo", &echo));
    assert!(command_allowed("/bin/echo", &echo));
    assert!(command_allowed("/usr/bin/echo", &echo));
    assert!(
        !command_allowed("/tmp/malicious-echo", &echo),
        "suffix of a path component must not authorize a different binary"
    );
    assert!(!command_allowed("my-echo", &echo));
    assert!(!command_allowed("bash", &echo));
    assert!(!command_allowed("/bin/bash", &echo));

    let sh = allow(&["sh"]);
    assert!(command_allowed("sh", &sh));
    assert!(command_allowed("/bin/sh", &sh));
    assert!(
        !command_allowed("bash", &sh),
        "`sh` must not suffix-match `bash`"
    );
    assert!(!command_allowed("/bin/bash", &sh));
    assert!(!command_allowed("/tmp/malicious-sh", &sh));

    let exact = allow(&["/bin/echo"]);
    assert!(command_allowed("/bin/echo", &exact));
    assert!(
        !command_allowed("echo", &exact),
        "exact path entries do not match a bare name"
    );
    assert!(!command_allowed("/usr/bin/echo", &exact));
    assert!(!command_allowed("/tmp/malicious-echo", &exact));
    assert!(!command_allowed("/bin/echo-extra", &exact));

    let win = allow(&["echo"]);
    assert!(!command_allowed(r"C:\tmp\malicious-echo", &win));
    assert!(!command_allowed(r"C:\tmp\my-echo", &win));
    assert!(command_allowed(r"C:\Windows\echo", &win));
}

#[test]
fn scheduler_blocks_suffix_matching_bypass() {
    use super::{schedule, SchedulerConfig};
    use crate::event::NetworkScope;
    use crate::kernel::Kernel;
    use crate::log::EventLog;

    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::create(dir.path().join("e.cak")).unwrap();
    let mut k = Kernel::from_log(log).unwrap();
    k.create_execution("allow").unwrap();
    let (goal, _) = k.create_goal("g", vec![]).unwrap();
    let (parent, _) = k.spawn_agent(None, goal, "p", None, None).unwrap();
    let (cap, _) = k
        .grant_capability(
            parent,
            None,
            "/",
            NetworkScope::Deny,
            vec!["echo".into()],
            "test",
        )
        .unwrap();
    let (agent, _) = k
        .spawn_agent(Some(parent), goal, "child", None, Some(cap))
        .unwrap();
    let (evil, _, _) = k
        .create_operation(agent, vec!["/tmp/malicious-echo".into()], "/", None)
        .unwrap();
    let (ok, _, _) = k
        .create_operation(agent, vec!["/bin/echo".into(), "hi".into()], "/", None)
        .unwrap();
    let decision = schedule(k.execution().unwrap(), &SchedulerConfig::default()).unwrap();
    assert!(
        decision.blocked_operations.contains(&evil),
        "malicious-echo must be blocked, got {decision:?}"
    );
    assert!(
        decision.runnable_operations.contains(&ok),
        "/bin/echo must be allowed for a bare echo allowlist, got {decision:?}"
    );
}
