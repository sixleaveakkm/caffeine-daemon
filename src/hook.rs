/// The Claude Code hook script, handed out by `caffeine-daemon print-script`.
pub const SCRIPT: &str = include_str!("../hooks/claude-code.sh");

/// The lines the script ships with, replaced so a printed copy defaults to the
/// port and key the daemon is actually configured for.
const PORT_LINE: &str = r#"PORT="${CAFFEINE_PORT:-8787}""#;
const KEY_LINE: &str = "DEFAULT_KEY=''";

pub fn script(port: u16, api_key: Option<&str>) -> String {
    let script = SCRIPT.replace(PORT_LINE, &format!(r#"PORT="${{CAFFEINE_PORT:-{port}}}""#));
    match api_key {
        // Single quotes keep the key literal, so only a quote of its own needs
        // escaping.
        Some(key) => script.replace(
            KEY_LINE,
            &format!("DEFAULT_KEY='{}'", key.replace('\'', r"'\''")),
        ),
        None => script,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_embedded_and_runnable() {
        assert!(SCRIPT.starts_with("#!/bin/bash\n"));
        assert!(SCRIPT.contains("/hook"));
        assert!(SCRIPT.ends_with('\n'));
    }

    #[test]
    fn the_configured_port_becomes_the_default() {
        assert!(SCRIPT.contains(PORT_LINE), "the port line drifted");

        let printed = script(9000, None);
        assert!(printed.contains(r#"PORT="${CAFFEINE_PORT:-9000}""#));
        assert!(!printed.contains("8787"));
        assert_eq!(script(8787, None), SCRIPT);
    }

    #[test]
    fn the_configured_key_becomes_the_default() {
        assert!(SCRIPT.contains(KEY_LINE), "the key line drifted");

        let printed = script(8787, Some("0syuaz9f"));
        assert!(printed.contains("DEFAULT_KEY='0syuaz9f'"));
        assert!(!printed.contains(KEY_LINE));

        let quoted = script(8787, Some("it's-a-key"));
        assert!(quoted.contains(r"DEFAULT_KEY='it'\''s-a-key'"));
    }
}
