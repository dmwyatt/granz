/// Output mode determines how results are formatted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputMode {
    Tty,
    Json,
}

/// Detect the appropriate output mode.
pub fn detect_output_mode(json_flag: bool) -> OutputMode {
    if json_flag {
        return OutputMode::Json;
    }
    OutputMode::Tty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_flag_gives_json() {
        assert_eq!(detect_output_mode(true), OutputMode::Json);
    }

    #[test]
    fn test_no_json_flag_gives_tty() {
        assert_eq!(detect_output_mode(false), OutputMode::Tty);
    }
}
