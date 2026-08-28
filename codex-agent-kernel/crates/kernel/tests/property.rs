use proptest::prelude::*;

use codex_agent_kernel::event::{JoinFailurePolicy, JoinKind};
use codex_agent_kernel::ids::AgentId;
use codex_agent_kernel::log::EventLog;
use codex_agent_kernel::state::{prefix_hashes, state_hash};
use codex_agent_kernel::Kernel;

fn proptest_cases() -> u32 {
    std::env::var("CAK_PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1024)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: proptest_cases(),
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_completion_order_never_double_commits_join(
        n in 2u8..=8,
        fail_at in 0u8..=8,
        seed in 0u64..10_000,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::create(dir.path().join("e.cak")).unwrap();
        let mut k = Kernel::from_log(log).unwrap();
        k.create_execution(format!("prop-{seed}")).unwrap();
        let (goal, _) = k.create_goal("fanout", vec![]).unwrap();
        let (parent, _) = k.spawn_agent(None, goal, "parent", None, None).unwrap();
        k.start_agent(parent).unwrap();
        let mut children = Vec::new();
        for i in 0..n {
            let (id, _) = k.spawn_agent(Some(parent), goal, format!("c{i}"), None, None).unwrap();
            k.start_agent(id).unwrap();
            children.push(id);
        }
        let (join, _) = k.create_join(parent, children.clone(), JoinKind::All, JoinFailurePolicy::WaitAll).unwrap();
        let mut order = children.clone();
        // deterministic shuffle from seed
        let mut x = seed.wrapping_add(1);
        for i in (1..order.len()).rev() {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let j = (x as usize) % (i + 1);
            order.swap(i, j);
        }
        for (idx, child) in order.iter().enumerate() {
            if fail_at < n && idx == fail_at as usize {
                k.fail_agent(*child, "injected").unwrap();
            } else {
                k.complete_agent(*child).unwrap();
            }
            k.observe_join_child(join, *child).unwrap();
        }
        let exec = k.execution().unwrap();
        let join_state = exec.joins.get(&join).unwrap();
        prop_assert!(join_state.outcome.is_some());
        let waiter = exec.agents.get(&parent).unwrap();
        prop_assert!(waiter.status.is_terminal());
        let hashes = prefix_hashes(k.log.records()).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for (event_id, hash, _) in &hashes {
            prop_assert!(seen.insert(*event_id), "duplicate event in prefix hash");
            let _ = hash;
        }
        let h1 = state_hash(exec).unwrap();
        let k2 = Kernel::from_log(EventLog::open(k.log.path()).unwrap()).unwrap();
        let h2 = state_hash(k2.execution().unwrap()).unwrap();
        prop_assert_eq!(h1, h2);
    }

    #[test]
    fn duplicate_delivery_of_recorded_events_is_idempotent(n in 1u8..=12) {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::create(dir.path().join("e.cak")).unwrap();
        let mut k = Kernel::from_log(log).unwrap();
        k.create_execution("dup").unwrap();
        let (goal, _) = k.create_goal("g", vec![]).unwrap();
        let mut last = AgentId::new();
        for i in 0..n {
            let (id, _) = k.spawn_agent(None, goal, format!("a{i}"), None, None).unwrap();
            last = id;
        }
        let records = k.log.records().to_vec();
        let mut state = k.state.clone();
        for record in &records {
            codex_agent_kernel::reduce(&mut state, record).unwrap();
        }
        let h1 = state_hash(k.execution().unwrap()).unwrap();
        let exec2 = state.require_execution(k.execution_id().unwrap()).unwrap();
        let h2 = state_hash(exec2).unwrap();
        prop_assert_eq!(h1, h2);
        let _ = last;
    }
}
