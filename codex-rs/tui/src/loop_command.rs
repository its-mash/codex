#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LoopCommandAction {
    List,
    Stop { id: String },
    Create { every_seconds: u64, prompt: String },
}

pub(crate) const LOOP_USAGE: &str =
    "Usage: /loop [list | stop <id> | <interval> <prompt>] (interval examples: 30s, 15m, 2h)";

pub(crate) fn parse_loop_command(args: &str) -> Result<LoopCommandAction, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
        return Ok(LoopCommandAction::List);
    }
    if let Some(id) = trimmed.strip_prefix("stop ").map(str::trim) {
        if id.is_empty() {
            return Err(LOOP_USAGE.to_string());
        }
        return Ok(LoopCommandAction::Stop { id: id.to_string() });
    }
    let Some((interval, prompt)) = trimmed.split_once(char::is_whitespace) else {
        return Err(LOOP_USAGE.to_string());
    };
    let every_seconds = parse_interval_seconds(interval)?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(LOOP_USAGE.to_string());
    }
    Ok(LoopCommandAction::Create {
        every_seconds,
        prompt: prompt.to_string(),
    })
}

fn parse_interval_seconds(value: &str) -> Result<u64, String> {
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .ok_or_else(|| LOOP_USAGE.to_string())?;
    let (amount, unit) = value.split_at(split);
    let amount = amount.parse::<u64>().map_err(|_| LOOP_USAGE.to_string())?;
    if amount == 0 {
        return Err("Loop interval must be greater than zero.".to_string());
    }
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => return Err(LOOP_USAGE.to_string()),
    };
    amount
        .checked_mul(multiplier)
        .ok_or_else(|| "Loop interval is too large.".to_string())
}

#[cfg(test)]
#[path = "loop_command_tests.rs"]
mod tests;
