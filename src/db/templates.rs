use anyhow::Result;
use rusqlite::Connection;

use crate::models::{ChatSuggestion, PanelTemplate, TemplateSection};

pub fn list_templates(conn: &Connection, category: Option<&str>) -> Result<Vec<PanelTemplate>> {
    let mut sql = String::from(
        "SELECT id, title, category, symbol, color, description, is_granola, owner_id, sections_json, created_at, updated_at, deleted_at, chat_suggestions_json, extra_json FROM templates WHERE deleted_at IS NULL",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(cat) = category {
        sql.push_str(" AND category LIKE ?");
        params.push(Box::new(format!("%{}%", cat)));
    }

    sql.push_str(" ORDER BY title");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(TemplateRow {
            id: row.get(0)?,
            title: row.get(1)?,
            category: row.get(2)?,
            symbol: row.get(3)?,
            color: row.get(4)?,
            description: row.get(5)?,
            is_granola: row.get(6)?,
            owner_id: row.get(7)?,
            sections_json: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
            deleted_at: row.get(11)?,
            chat_suggestions_json: row.get(12)?,
            extra_json: row.get(13)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_template).collect())
}

pub fn show_template(conn: &Connection, query: &str) -> Result<Option<PanelTemplate>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, category, symbol, color, description, is_granola, owner_id, sections_json, created_at, updated_at, deleted_at, chat_suggestions_json, extra_json FROM templates WHERE id = ?1 OR title LIKE ?2 LIMIT 1",
    )?;

    let pattern = format!("%{}%", query);
    let result = stmt
        .query_row(rusqlite::params![query, pattern], |row| {
            Ok(TemplateRow {
                id: row.get(0)?,
                title: row.get(1)?,
                category: row.get(2)?,
                symbol: row.get(3)?,
                color: row.get(4)?,
                description: row.get(5)?,
                is_granola: row.get(6)?,
                owner_id: row.get(7)?,
                sections_json: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                deleted_at: row.get(11)?,
                chat_suggestions_json: row.get(12)?,
                extra_json: row.get(13)?,
            })
        })
        .ok();

    Ok(result.map(row_to_template))
}

struct TemplateRow {
    id: Option<String>,
    title: Option<String>,
    category: Option<String>,
    symbol: Option<String>,
    color: Option<String>,
    description: Option<String>,
    is_granola: Option<bool>,
    owner_id: Option<String>,
    sections_json: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    deleted_at: Option<String>,
    chat_suggestions_json: Option<String>,
    extra_json: Option<String>,
}

fn row_to_template(row: TemplateRow) -> PanelTemplate {
    let sections: Option<Vec<TemplateSection>> = row
        .sections_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let chat_suggestions: Option<Vec<ChatSuggestion>> = row
        .chat_suggestions_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let extra = row
        .extra_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    PanelTemplate {
        id: row.id,
        title: row.title,
        category: row.category,
        symbol: row.symbol,
        color: row.color,
        description: row.description,
        is_granola: row.is_granola,
        owner_id: row.owner_id,
        sections,
        chat_suggestions,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted_at: row.deleted_at,
        shared_with: None,
        extra,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, templates_state};

    #[test]
    fn test_list_templates_all() {
        let conn = build_test_db(&templates_state());
        let templates = list_templates(&conn, None).unwrap();
        // deleted template excluded
        assert_eq!(templates.len(), 2);
    }

    #[test]
    fn test_list_templates_by_category() {
        let conn = build_test_db(&templates_state());
        let templates = list_templates(&conn, Some("agile")).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].title.as_deref(), Some("Standup"));
    }

    #[test]
    fn test_show_template_by_id() {
        let conn = build_test_db(&templates_state());
        let tmpl = show_template(&conn, "tmpl-1").unwrap().unwrap();
        assert_eq!(tmpl.title.as_deref(), Some("Meeting Notes"));
        assert_eq!(tmpl.sections.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_show_template_by_title() {
        let conn = build_test_db(&templates_state());
        let tmpl = show_template(&conn, "Standup").unwrap().unwrap();
        assert_eq!(tmpl.id.as_deref(), Some("tmpl-2"));
    }

    #[test]
    fn test_show_template_not_found() {
        let conn = build_test_db(&templates_state());
        let tmpl = show_template(&conn, "nonexistent").unwrap();
        assert!(tmpl.is_none());
    }
}
