use pretty_assertions::assert_eq;

use super::LoopCommandAction;
use super::parse_loop_command;

#[test]
fn parses_loop_create_stop_and_list_commands() {
    assert_eq!(
        parse_loop_command("15m reconcile the mock task board"),
        Ok(LoopCommandAction::Create {
            every_seconds: 900,
            prompt: "reconcile the mock task board".to_string(),
        })
    );
    assert_eq!(
        parse_loop_command("stop 0198-test"),
        Ok(LoopCommandAction::Stop {
            id: "0198-test".to_string(),
        })
    );
    assert_eq!(parse_loop_command("list"), Ok(LoopCommandAction::List));
}

#[test]
fn rejects_zero_and_overflowing_intervals() {
    assert_eq!(
        parse_loop_command("0s run"),
        Err("Loop interval must be greater than zero.".to_string())
    );
    assert_eq!(
        parse_loop_command("18446744073709551615d run"),
        Err("Loop interval is too large.".to_string())
    );
}
