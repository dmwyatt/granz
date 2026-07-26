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

/// Format a byte count for display.
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
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

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(47185920), "45.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
    }
}
