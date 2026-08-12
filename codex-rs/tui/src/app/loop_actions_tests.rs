use codex_app_server_protocol::LoopAutomation;
use pretty_assertions::assert_eq;

use super::format_interval;
use super::format_loop_list;

#[test]
fn formats_loop_intervals_and_list() {
    let loops = vec![LoopAutomation {
        id: "loop-1".to_string(),
        name: "review".to_string(),
        prompt: "Review the fixture.".to_string(),
        every_seconds: 900,
        enabled: true,
        created_at: 1,
        next_due_at: 2,
        last_run_at: None,
    }];

    assert_eq!(format_interval(86_400), "1d");
    assert_eq!(format_interval(7_200), "2h");
    assert_eq!(format_interval(900), "15m");
    assert_eq!(format_interval(30), "30s");
    insta::assert_snapshot!(format_loop_list(&loops));
}
