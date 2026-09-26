//! Human durations like `30s`, `10m`, `2h`, `7d`, `2w` or `1h30m`. A bare
//! number means seconds.

use std::time::Duration;

pub fn parse(input: &str) -> Result<Duration, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }

    let mut total: u64 = 0;
    let mut digits = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit = match ch {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            'd' => 24 * 60 * 60,
            'w' => 7 * 24 * 60 * 60,
            _ => return Err(format!("invalid duration `{input}`: unknown unit `{ch}` (use s, m, h, d or w)")),
        };
        if digits.is_empty() {
            return Err(format!("invalid duration `{input}`: expected a number before `{ch}`"));
        }
        let n: u64 = digits.parse().map_err(|_| format!("invalid duration `{input}`"))?;
        total = n
            .checked_mul(unit)
            .and_then(|v| total.checked_add(v))
            .ok_or_else(|| format!("duration `{input}` is too large"))?;
        digits.clear();
    }
    if !digits.is_empty() {
        return Err(format!("invalid duration `{input}`: missing unit after `{digits}`"));
    }
    Ok(Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse("30").unwrap().as_secs(), 30);
        assert_eq!(parse("30s").unwrap().as_secs(), 30);
        assert_eq!(parse("10m").unwrap().as_secs(), 600);
        assert_eq!(parse("2h").unwrap().as_secs(), 7200);
        assert_eq!(parse("7d").unwrap().as_secs(), 7 * 86400);
        assert_eq!(parse("2w").unwrap().as_secs(), 14 * 86400);
        assert_eq!(parse("1h30m").unwrap().as_secs(), 5400);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "abc", "10x", "h", "5m3", "-1d"] {
            assert!(parse(bad).is_err(), "{bad} should fail");
        }
    }
}
