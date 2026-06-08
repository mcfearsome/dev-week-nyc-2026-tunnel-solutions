use std::time::Duration;

/// Parse `--ttl` values like `2m`, `15m`, `90s`, `1h30m`.
pub fn parse_ttl(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| format!("invalid duration '{s}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minutes() {
        assert_eq!(parse_ttl("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_ttl("15m").unwrap(), Duration::from_secs(900));
    }

    #[test]
    fn parses_seconds_and_compound() {
        assert_eq!(parse_ttl("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_ttl("1h30m").unwrap(), Duration::from_secs(5400));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_ttl("soon").is_err());
    }
}
