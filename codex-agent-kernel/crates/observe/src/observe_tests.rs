use super::{
    run_goal_complete_experiment, run_wrapper_complete_experiment, tool_response_process_id,
};
use pretty_assertions::assert_eq;

#[test]
fn yield_keeps_process_id_while_alive() {
    assert_eq!(tool_response_process_id(false, 7), Some(7));
    assert_eq!(tool_response_process_id(true, 7), None);
}

#[test]
fn wrapper_complete_experiment_detects_disagreement() {
    let report = run_wrapper_complete_experiment().unwrap();
    assert_eq!(report.scenario, "wrapper_complete_while_process_running");
    assert_eq!(report.baseline["process_still_alive"], true);
    assert_eq!(report.baseline["process_id_in_tool_response"], 7);
    assert_eq!(report.failure_reproduced, false);
    assert_eq!(report.disagreements.len(), 1);
    assert_eq!(
        report.disagreements[0].code,
        "wrapper_complete_process_running"
    );
    let status = report.kernel["operation_status"].as_str().unwrap();
    assert!(
        status.contains("Running"),
        "kernel should still be running, got {status}"
    );
}

#[test]
fn goal_complete_experiment_rejects_model_turn() {
    let report = run_goal_complete_experiment().unwrap();
    assert_eq!(report.failure_reproduced, false);
    assert_eq!(report.disagreements.len(), 1);
    assert_eq!(
        report.disagreements[0].code,
        "goal_complete_unfinished_work"
    );
}
