/// Convert a TipTap JSON document to Markdown.
///
/// Handles the node types observed in Granola's AI-generated panels:
/// doc, heading, paragraph, bulletList, orderedList, listItem,
/// horizontalRule, text, and marks (bold, link).
pub fn tiptap_to_markdown(doc: &serde_json::Value) -> String {
    let mut output = String::new();
    if let Some(content) = doc.get("content").and_then(|c| c.as_array()) {
        render_nodes(content, &mut output, 0);
    }
    output.trim_end().to_string()
}

fn render_nodes(nodes: &[serde_json::Value], output: &mut String, depth: usize) {
    for node in nodes {
        render_node(node, output, depth);
    }
}

fn render_node(node: &serde_json::Value, output: &mut String, depth: usize) {
    let node_type = node.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match node_type {
        "doc" => {
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                render_nodes(content, output, depth);
            }
        }
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(|l| l.as_u64())
                .unwrap_or(1) as usize;
            let prefix = "#".repeat(level.min(6));
            output.push_str(&prefix);
            output.push(' ');
            render_inline_content(node, output);
            output.push_str("\n\n");
        }
        "paragraph" => {
            let indent = list_indent(depth);
            output.push_str(&indent);
            render_inline_content(node, output);
            output.push_str("\n\n");
        }
        "bulletList" => {
            if let Some(items) = node.get("content").and_then(|c| c.as_array()) {
                for item in items {
                    render_list_item(item, output, depth, None);
                }
            }
            if depth == 0 {
                output.push('\n');
            }
        }
        "orderedList" => {
            let start = node
                .get("attrs")
                .and_then(|a| a.get("start"))
                .and_then(|s| s.as_u64())
                .unwrap_or(1) as usize;
            if let Some(items) = node.get("content").and_then(|c| c.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    render_list_item(item, output, depth, Some(start + i));
                }
            }
            if depth == 0 {
                output.push('\n');
            }
        }
        "listItem" => {
            // listItems are handled by bulletList/orderedList
            render_inline_content(node, output);
        }
        "horizontalRule" => {
            output.push_str("---\n\n");
        }
        "text" => {
            let text = node.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let marked = apply_marks(text, node);
            output.push_str(&marked);
        }
        _ => {
            // Unknown node: try to extract text content as fallback
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                render_nodes(content, output, depth);
            } else if let Some(text) = node.get("text").and_then(|t| t.as_str()) {
                output.push_str(text);
            }
        }
    }
}

fn render_list_item(
    item: &serde_json::Value,
    output: &mut String,
    depth: usize,
    number: Option<usize>,
) {
    let indent = "  ".repeat(depth);
    let marker = match number {
        Some(n) => format!("{}. ", n),
        None => "- ".to_string(),
    };

    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
        for (i, child) in content.iter().enumerate() {
            let child_type = child.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match child_type {
                "paragraph" => {
                    if i == 0 {
                        output.push_str(&indent);
                        output.push_str(&marker);
                    } else {
                        output.push_str(&indent);
                        output.push_str(&" ".repeat(marker.len()));
                    }
                    render_inline_content(child, output);
                    output.push('\n');
                }
                "bulletList" | "orderedList" => {
                    render_node(child, output, depth + 1);
                }
                _ => {
                    if i == 0 {
                        output.push_str(&indent);
                        output.push_str(&marker);
                    }
                    render_inline_content(child, output);
                    output.push('\n');
                }
            }
        }
    }
}

fn render_inline_content(node: &serde_json::Value, output: &mut String) {
    if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
        for child in content {
            render_node(child, output, 0);
        }
    }
}

fn apply_marks(text: &str, node: &serde_json::Value) -> String {
    let marks = match node.get("marks").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return text.to_string(),
    };

    let mut result = text.to_string();
    // Track link href separately since it wraps differently
    let mut link_href: Option<String> = None;
    let mut is_bold = false;

    for mark in marks {
        let mark_type = mark.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match mark_type {
            "bold" => is_bold = true,
            "link" => {
                link_href = mark
                    .get("attrs")
                    .and_then(|a| a.get("href"))
                    .and_then(|h| h.as_str())
                    .map(|s| s.to_string());
            }
            _ => {}
        }
    }

    if is_bold {
        result = format!("**{}**", result);
    }
    if let Some(href) = link_href {
        result = format!("[{}]({})", result, href);
    }

    result
}

fn list_indent(depth: usize) -> String {
    if depth > 0 {
        "  ".repeat(depth)
    } else {
        String::new()
    }
}

/// Extract a Granola chat URL from panel markdown content.
///
/// Looks for a markdown link pointing to `notes.granola.ai` and returns
/// the cleaned markdown (with the link line removed) and the extracted URL.
pub fn extract_chat_url(markdown: &str) -> (String, Option<String>) {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut url: Option<String> = None;
    let mut link_line_idx: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if let Some(extracted) = extract_granola_chat_url_from_line(line) {
            url = Some(extracted);
            link_line_idx = Some(i);
            break;
        }
    }

    let Some(idx) = link_line_idx else {
        return (markdown.to_string(), None);
    };

    // Build cleaned output: skip the link line and any preceding blank line
    let skip_from = if idx > 0 && lines[idx - 1].trim().is_empty() {
        idx - 1
    } else {
        idx
    };

    let cleaned: Vec<&str> = lines[..skip_from]
        .iter()
        .chain(lines[idx + 1..].iter())
        .copied()
        .collect();

    let result = cleaned.join("\n").trim_end().to_string();
    (result, url)
}

/// Extract a `notes.granola.ai` URL from a markdown link within a line.
fn extract_granola_chat_url_from_line(line: &str) -> Option<String> {
    let marker = "](https://notes.granola.ai/";
    let start = line.find(marker)?;
    // Find the opening `(` of the link href
    let url_start = start + 2; // skip `](`
    let rest = &line[url_start..];
    let end = rest.find(')')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_empty_doc() {
        let doc = json!({"type": "doc", "content": []});
        assert_eq!(tiptap_to_markdown(&doc), "");
    }

    #[test]
    fn test_no_content_key() {
        let doc = json!({"type": "doc"});
        assert_eq!(tiptap_to_markdown(&doc), "");
    }

    #[test]
    fn test_heading_levels() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Title"}]},
                {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Subtitle"}]},
                {"type": "heading", "attrs": {"level": 3}, "content": [{"type": "text", "text": "Section"}]}
            ]
        });
        assert_eq!(
            tiptap_to_markdown(&doc),
            "# Title\n\n## Subtitle\n\n### Section"
        );
    }

    #[test]
    fn test_paragraph() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Hello world"}]}
            ]
        });
        assert_eq!(tiptap_to_markdown(&doc), "Hello world");
    }

    #[test]
    fn test_bullet_list() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "bulletList",
                "content": [
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Item one"}]}]},
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Item two"}]}]}
                ]
            }]
        });
        assert_eq!(tiptap_to_markdown(&doc), "- Item one\n- Item two");
    }

    #[test]
    fn test_ordered_list() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "orderedList",
                "attrs": {"start": 1},
                "content": [
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "First"}]}]},
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Second"}]}]}
                ]
            }]
        });
        assert_eq!(tiptap_to_markdown(&doc), "1. First\n2. Second");
    }

    #[test]
    fn test_nested_bullet_list() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "bulletList",
                "content": [
                    {"type": "listItem", "content": [
                        {"type": "paragraph", "content": [{"type": "text", "text": "Parent"}]},
                        {"type": "bulletList", "content": [
                            {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Child"}]}]}
                        ]}
                    ]}
                ]
            }]
        });
        assert_eq!(tiptap_to_markdown(&doc), "- Parent\n  - Child");
    }

    #[test]
    fn test_bold_mark() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "Hello "},
                    {"type": "text", "text": "bold", "marks": [{"type": "bold"}]},
                    {"type": "text", "text": " world"}
                ]}
            ]
        });
        assert_eq!(tiptap_to_markdown(&doc), "Hello **bold** world");
    }

    #[test]
    fn test_link_mark() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "Click "},
                    {"type": "text", "text": "here", "marks": [{"type": "link", "attrs": {"href": "https://example.com"}}]}
                ]}
            ]
        });
        assert_eq!(
            tiptap_to_markdown(&doc),
            "Click [here](https://example.com)"
        );
    }

    #[test]
    fn test_bold_link_combined() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "link", "marks": [
                        {"type": "bold"},
                        {"type": "link", "attrs": {"href": "https://example.com"}}
                    ]}
                ]}
            ]
        });
        assert_eq!(
            tiptap_to_markdown(&doc),
            "[**link**](https://example.com)"
        );
    }

    #[test]
    fn test_horizontal_rule() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Above"}]},
                {"type": "horizontalRule"},
                {"type": "paragraph", "content": [{"type": "text", "text": "Below"}]}
            ]
        });
        assert_eq!(tiptap_to_markdown(&doc), "Above\n\n---\n\nBelow");
    }

    #[test]
    fn test_unknown_node_type_extracts_text() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "customWidget", "content": [
                    {"type": "text", "text": "fallback text"}
                ]}
            ]
        });
        assert_eq!(tiptap_to_markdown(&doc), "fallback text");
    }

    #[test]
    fn test_heading_with_bold() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "heading", "attrs": {"level": 2}, "content": [
                    {"type": "text", "text": "Action Items", "marks": [{"type": "bold"}]}
                ]}
            ]
        });
        assert_eq!(tiptap_to_markdown(&doc), "## **Action Items**");
    }

    #[test]
    fn test_multiple_paragraphs() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "First paragraph"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "Second paragraph"}]}
            ]
        });
        assert_eq!(
            tiptap_to_markdown(&doc),
            "First paragraph\n\nSecond paragraph"
        );
    }

    #[test]
    fn test_realistic_panel() {
        let doc = json!({
            "type": "doc",
            "content": [
                {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Summary"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "The team discussed the Q1 roadmap."}]},
                {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Action Items"}]},
                {"type": "bulletList", "content": [
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [
                        {"type": "text", "text": "Alice", "marks": [{"type": "bold"}]},
                        {"type": "text", "text": " to finalize design"}
                    ]}]},
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [
                        {"type": "text", "text": "Bob", "marks": [{"type": "bold"}]},
                        {"type": "text", "text": " to review PRs"}
                    ]}]}
                ]}
            ]
        });
        let expected = "## Summary\n\nThe team discussed the Q1 roadmap.\n\n## Action Items\n\n- **Alice** to finalize design\n- **Bob** to review PRs";
        assert_eq!(tiptap_to_markdown(&doc), expected);
    }

    // --- extract_chat_url tests ---

    #[test]
    fn extract_chat_url_standard_label() {
        let md = "## Summary\n\nKey decisions made.\n\nChat with meeting transcript: [https://notes.granola.ai/t/abc123](https://notes.granola.ai/t/abc123)";
        let (cleaned, url) = extract_chat_url(md);
        assert_eq!(url.as_deref(), Some("https://notes.granola.ai/t/abc123"));
        assert_eq!(cleaned, "## Summary\n\nKey decisions made.");
    }

    #[test]
    fn extract_chat_url_different_label() {
        let md = "Content here.\n\n[Click to chat](https://notes.granola.ai/t/xyz789)";
        let (cleaned, url) = extract_chat_url(md);
        assert_eq!(url.as_deref(), Some("https://notes.granola.ai/t/xyz789"));
        assert_eq!(cleaned, "Content here.");
    }

    #[test]
    fn extract_chat_url_no_link() {
        let md = "## Summary\n\nJust some content.";
        let (cleaned, url) = extract_chat_url(md);
        assert!(url.is_none());
        assert_eq!(cleaned, md);
    }

    #[test]
    fn extract_chat_url_empty_string() {
        let (cleaned, url) = extract_chat_url("");
        assert!(url.is_none());
        assert_eq!(cleaned, "");
    }

    #[test]
    fn extract_chat_url_preserves_content_above() {
        let md = "## Summary\n\nLine one.\n\n## Action Items\n\n- Do thing\n\n[Chat](https://notes.granola.ai/t/123)";
        let (cleaned, _) = extract_chat_url(md);
        assert_eq!(
            cleaned,
            "## Summary\n\nLine one.\n\n## Action Items\n\n- Do thing"
        );
    }

    #[test]
    fn extract_chat_url_strips_preceding_blank_line() {
        let md = "Content.\n\n[Chat](https://notes.granola.ai/t/abc)";
        let (cleaned, url) = extract_chat_url(md);
        assert_eq!(url.as_deref(), Some("https://notes.granola.ai/t/abc"));
        assert_eq!(cleaned, "Content.");
    }

    #[test]
    fn extract_chat_url_realistic_multi_section() {
        let md = "\
## Summary

The team discussed Q1 roadmap and priorities.

## Action Items

- **Alice** to finalize design
- **Bob** to review PRs

Chat with meeting transcript: [https://notes.granola.ai/t/meeting-abc-123](https://notes.granola.ai/t/meeting-abc-123)";

        let (cleaned, url) = extract_chat_url(md);
        assert_eq!(
            url.as_deref(),
            Some("https://notes.granola.ai/t/meeting-abc-123")
        );
        assert_eq!(
            cleaned,
            "## Summary\n\nThe team discussed Q1 roadmap and priorities.\n\n## Action Items\n\n- **Alice** to finalize design\n- **Bob** to review PRs"
        );
    }
}
