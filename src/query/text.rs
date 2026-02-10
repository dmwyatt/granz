/// Case-insensitive substring check.
pub fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Strip the "Chat with Granola" footer that appears at the end of panel markdown.
/// Removes the footer line and any preceding `---` separator.
pub fn strip_panel_footer(content: &str) -> &str {
    let trimmed = content.trim_end();
    if let Some(pos) = trimmed.rfind("\nChat with") {
        let before = trimmed[..pos].trim_end();
        if before.ends_with("---") {
            before[..before.len() - 3].trim_end()
        } else {
            before
        }
    } else if trimmed.starts_with("Chat with") {
        ""
    } else {
        trimmed
    }
}

/// Split markdown content on the most frequent header level.
/// Returns (heading, body) tuples. Preamble before first heading gets `heading = None`.
pub fn split_markdown_sections(content: &str) -> Vec<(Option<&str>, &str)> {
    let mut sections = Vec::new();

    let level = match detect_section_header_level(content) {
        Some(l) => l,
        None => {
            if !content.trim().is_empty() {
                sections.push((None, content.trim()));
            }
            return sections;
        }
    };

    // Build the marker string: "# ", "## ", "### ", etc.
    let marker: String = "#".repeat(level) + " ";
    let newline_marker: String = format!("\n{}", marker);
    let marker_len = level + 1; // hashes + space

    // Pass 1: collect byte positions of each marker
    let mut heading_starts: Vec<usize> = Vec::new();

    if content.starts_with(&marker) {
        heading_starts.push(0);
    }
    for (i, _) in content.match_indices(&newline_marker) {
        heading_starts.push(i + 1); // point at `#`, not `\n`
    }

    if heading_starts.is_empty() {
        if !content.trim().is_empty() {
            sections.push((None, content.trim()));
        }
        return sections;
    }

    // Preamble before first heading
    if heading_starts[0] > 0 {
        let preamble = content[..heading_starts[0]].trim();
        if !preamble.is_empty() {
            sections.push((None, preamble));
        }
    }

    // Pass 2: extract heading + body for each section
    for (idx, &start) in heading_starts.iter().enumerate() {
        let after_marker = &content[start + marker_len..];
        let line_end = after_marker.find('\n').unwrap_or(after_marker.len());
        let heading = after_marker[..line_end].trim();

        let body_start = start + marker_len + line_end;
        let body_end = heading_starts.get(idx + 1).copied().unwrap_or(content.len());

        if body_start <= body_end {
            let body = content[body_start..body_end].trim();
            if !body.is_empty() {
                sections.push((Some(heading), body));
            }
        }
    }

    sections
}

/// Return the header level (1-6) if a line starts with 1-6 `#` followed by a space.
fn header_level_of_line(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes >= 1 && hashes <= 6 && line.as_bytes().get(hashes) == Some(&b' ') {
        Some(hashes)
    } else {
        None
    }
}

/// Scan content for markdown headers and return the most frequent header level.
/// Tie-break: prefer deeper (more hashes), since shallow headers are more likely titles.
fn detect_section_header_level(content: &str) -> Option<usize> {
    let mut counts = [0u32; 7]; // index 0 unused, 1-6 for header levels
    for line in content.lines() {
        if let Some(level) = header_level_of_line(line) {
            counts[level] += 1;
        }
    }

    let max_count = *counts[1..].iter().max().unwrap_or(&0);
    if max_count == 0 {
        return None;
    }

    // Among levels with max count, pick the deepest (highest number)
    (1..=6)
        .rev()
        .find(|&level| counts[level] == max_count)
}

/// Split text into paragraphs (on `\n\n`), trimming whitespace and filtering empties.
pub fn split_into_paragraphs(content: &str) -> Vec<&str> {
    content
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // header_level_of_line tests

    #[test]
    fn test_header_level_h1() {
        assert_eq!(header_level_of_line("# Heading"), Some(1));
    }

    #[test]
    fn test_header_level_h3() {
        assert_eq!(header_level_of_line("### Heading"), Some(3));
    }

    #[test]
    fn test_header_level_h6() {
        assert_eq!(header_level_of_line("###### Heading"), Some(6));
    }

    #[test]
    fn test_header_level_no_space() {
        assert_eq!(header_level_of_line("###Heading"), None);
    }

    #[test]
    fn test_header_level_too_many_hashes() {
        assert_eq!(header_level_of_line("####### Heading"), None);
    }

    #[test]
    fn test_header_level_plain_text() {
        assert_eq!(header_level_of_line("Just some text"), None);
    }

    #[test]
    fn test_header_level_empty() {
        assert_eq!(header_level_of_line(""), None);
    }

    // detect_section_header_level tests

    #[test]
    fn test_detect_h3_only() {
        let content = "### A\n\nbody\n\n### B\n\nbody";
        assert_eq!(detect_section_header_level(content), Some(3));
    }

    #[test]
    fn test_detect_h1_only() {
        let content = "# A\n\nbody\n\n# B\n\nbody";
        assert_eq!(detect_section_header_level(content), Some(1));
    }

    #[test]
    fn test_detect_mixed_h1_h3() {
        // 1 h1 + 3 h3 → h3 is most frequent
        let content = "# Title\n\n### A\n\nbody\n\n### B\n\nbody\n\n### C\n\nbody";
        assert_eq!(detect_section_header_level(content), Some(3));
    }

    #[test]
    fn test_detect_tie_prefers_deeper() {
        // 2 h1 + 2 h3 → tie, prefer h3 (deeper)
        let content = "# A\n\n# B\n\n### C\n\n### D";
        assert_eq!(detect_section_header_level(content), Some(3));
    }

    #[test]
    fn test_detect_no_headers() {
        let content = "Just plain text\n\nNo headers here";
        assert_eq!(detect_section_header_level(content), None);
    }

    // contains_ignore_case tests

    #[test]
    fn test_contains_ignore_case_match() {
        assert!(contains_ignore_case("Hello World", "hello"));
        assert!(contains_ignore_case("Hello World", "WORLD"));
        assert!(contains_ignore_case("Hello World", "lo Wo"));
    }

    #[test]
    fn test_contains_ignore_case_no_match() {
        assert!(!contains_ignore_case("Hello World", "xyz"));
    }

    // strip_panel_footer tests

    #[test]
    fn test_strip_panel_footer_no_footer() {
        let content = "### Action Items\n\n- Do thing 1\n- Do thing 2";
        assert_eq!(strip_panel_footer(content), content);
    }

    #[test]
    fn test_strip_panel_footer_with_chat_line() {
        let content = "### Action Items\n\n- Do thing 1\n\nChat with Granola for more details.";
        assert_eq!(
            strip_panel_footer(content),
            "### Action Items\n\n- Do thing 1"
        );
    }

    #[test]
    fn test_strip_panel_footer_with_separator_and_chat() {
        let content =
            "### Action Items\n\n- Do thing 1\n\n---\nChat with Granola for more details.";
        assert_eq!(
            strip_panel_footer(content),
            "### Action Items\n\n- Do thing 1"
        );
    }

    #[test]
    fn test_strip_panel_footer_only_footer() {
        assert_eq!(strip_panel_footer("Chat with Granola"), "");
    }

    // split_markdown_sections tests

    #[test]
    fn test_split_markdown_sections_single_section() {
        let content = "### Action Items\n\n- Do thing 1\n- Do thing 2";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, Some("Action Items"));
        assert!(sections[0].1.contains("Do thing 1"));
    }

    #[test]
    fn test_split_markdown_sections_multiple() {
        let content = "### Action Items\n\n- Do thing 1\n\n### Key Decisions\n\nWe decided to ship.";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, Some("Action Items"));
        assert!(sections[0].1.contains("Do thing 1"));
        assert_eq!(sections[1].0, Some("Key Decisions"));
        assert!(sections[1].1.contains("decided to ship"));
    }

    #[test]
    fn test_split_markdown_sections_with_preamble() {
        let content = "Some preamble text here.\n\n### Action Items\n\n- Do thing 1";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, None);
        assert!(sections[0].1.contains("preamble"));
        assert_eq!(sections[1].0, Some("Action Items"));
    }

    #[test]
    fn test_split_markdown_sections_empty() {
        let sections = split_markdown_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn test_split_markdown_sections_many_sections_no_preamble() {
        let content = "### Timeline\n\n\
            - Date 1: Jan\n\
            - Date 2: Feb\n\n\
            ### Decisions\n\n\
            We decided to ship early.\n\n\
            ### Action Items\n\n\
            - Alice: write docs\n\
            - Bob: deploy";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].0, Some("Timeline"));
        assert!(sections[0].1.contains("Date 1"));
        assert_eq!(sections[1].0, Some("Decisions"));
        assert!(sections[1].1.contains("ship early"));
        assert_eq!(sections[2].0, Some("Action Items"));
        assert!(sections[2].1.contains("Bob: deploy"));
    }

    // split_markdown_sections tests — h1 headers

    #[test]
    fn test_split_markdown_sections_h1_only_multi() {
        let content = "# Announcements\n\nNew hire starting Monday.\n\n# Updates\n\nProject on track.\n\n# Action Items\n\n- Send welcome email";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].0, Some("Announcements"));
        assert!(sections[0].1.contains("New hire"));
        assert_eq!(sections[1].0, Some("Updates"));
        assert!(sections[1].1.contains("on track"));
        assert_eq!(sections[2].0, Some("Action Items"));
        assert!(sections[2].1.contains("welcome email"));
    }

    #[test]
    fn test_split_markdown_sections_h1_title_h3_sections() {
        // h1 title + h3 sections → should split on h3 (more frequent)
        let content = "# Meeting Title\n\n### Action Items\n\n- Do thing\n\n### Decisions\n\nWe decided.";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].0, None); // "# Meeting Title" is preamble
        assert!(sections[0].1.contains("Meeting Title"));
        assert_eq!(sections[1].0, Some("Action Items"));
        assert_eq!(sections[2].0, Some("Decisions"));
    }

    #[test]
    fn test_split_markdown_sections_h2_only() {
        let content = "## First\n\nBody one.\n\n## Second\n\nBody two.";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, Some("First"));
        assert!(sections[0].1.contains("Body one"));
        assert_eq!(sections[1].0, Some("Second"));
        assert!(sections[1].1.contains("Body two"));
    }

    #[test]
    fn test_split_markdown_sections_h1_with_preamble() {
        let content = "Some intro text.\n\n# Section One\n\nContent here.";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, None);
        assert!(sections[0].1.contains("intro text"));
        assert_eq!(sections[1].0, Some("Section One"));
        assert!(sections[1].1.contains("Content here"));
    }

    #[test]
    fn test_split_markdown_sections_single_h1() {
        let content = "# Only Section\n\nJust one section with some body text.";
        let sections = split_markdown_sections(content);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, Some("Only Section"));
        assert!(sections[0].1.contains("Just one section"));
    }

    // split_into_paragraphs tests

    #[test]
    fn test_split_into_paragraphs_basic() {
        let content = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let paras = split_into_paragraphs(content);
        assert_eq!(paras.len(), 3);
        assert_eq!(paras[0], "First paragraph.");
        assert_eq!(paras[1], "Second paragraph.");
        assert_eq!(paras[2], "Third paragraph.");
    }

    #[test]
    fn test_split_into_paragraphs_empty() {
        assert!(split_into_paragraphs("").is_empty());
    }

    #[test]
    fn test_split_into_paragraphs_whitespace_only() {
        assert!(split_into_paragraphs("   \n\n   ").is_empty());
    }

    #[test]
    fn test_split_into_paragraphs_single() {
        let paras = split_into_paragraphs("Just one paragraph.");
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0], "Just one paragraph.");
    }

    #[test]
    fn test_split_into_paragraphs_trims_whitespace() {
        let content = "  First  \n\n  Second  ";
        let paras = split_into_paragraphs(content);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0], "First");
        assert_eq!(paras[1], "Second");
    }

    #[test]
    fn test_split_into_paragraphs_filters_empty() {
        let content = "First\n\n\n\n\n\nSecond";
        let paras = split_into_paragraphs(content);
        assert_eq!(paras.len(), 2);
    }
}
