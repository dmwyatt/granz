use anyhow::Result;
use rusqlite::Connection;

use crate::models::{Recipe, RecipeConfig};

pub fn list_recipes(conn: &Connection, visibility: Option<&str>) -> Result<Vec<Recipe>> {
    let mut sql = String::from(
        "SELECT id, slug, visibility, publisher_slug, creator_name, config_json, created_at, updated_at, deleted_at, user_id, workspace_id, extra_json FROM recipes WHERE deleted_at IS NULL",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(vis) = visibility {
        sql.push_str(" AND visibility = ?");
        params.push(Box::new(vis.to_string()));
    }

    sql.push_str(" ORDER BY slug");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(RecipeRow {
            id: row.get(0)?,
            slug: row.get(1)?,
            visibility: row.get(2)?,
            publisher_slug: row.get(3)?,
            creator_name: row.get(4)?,
            config_json: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            deleted_at: row.get(8)?,
            user_id: row.get(9)?,
            workspace_id: row.get(10)?,
            extra_json: row.get(11)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_recipe).collect())
}

pub fn show_recipe(conn: &Connection, query: &str) -> Result<Option<Recipe>> {
    let mut stmt = conn.prepare(
        "SELECT id, slug, visibility, publisher_slug, creator_name, config_json, created_at, updated_at, deleted_at, user_id, workspace_id, extra_json FROM recipes WHERE id = ?1 OR slug = ?1 OR slug LIKE ?2 LIMIT 1",
    )?;

    let pattern = format!("%{}%", query);
    let result = stmt
        .query_row(rusqlite::params![query, pattern], |row| {
            Ok(RecipeRow {
                id: row.get(0)?,
                slug: row.get(1)?,
                visibility: row.get(2)?,
                publisher_slug: row.get(3)?,
                creator_name: row.get(4)?,
                config_json: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                deleted_at: row.get(8)?,
                user_id: row.get(9)?,
                workspace_id: row.get(10)?,
                extra_json: row.get(11)?,
            })
        })
        .ok();

    Ok(result.map(row_to_recipe))
}

struct RecipeRow {
    id: Option<String>,
    slug: Option<String>,
    visibility: Option<String>,
    publisher_slug: Option<String>,
    creator_name: Option<String>,
    config_json: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    deleted_at: Option<String>,
    user_id: Option<String>,
    workspace_id: Option<String>,
    extra_json: Option<String>,
}

fn row_to_recipe(row: RecipeRow) -> Recipe {
    let config: Option<RecipeConfig> = row
        .config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let extra = row
        .extra_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    Recipe {
        id: row.id,
        slug: row.slug,
        visibility: row.visibility,
        publisher_slug: row.publisher_slug,
        creator_name: row.creator_name,
        config,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted_at: row.deleted_at,
        user_id: row.user_id,
        workspace_id: row.workspace_id,
        shared_with: None,
        extra,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, recipes_state};

    #[test]
    fn test_list_recipes_all() {
        let conn = build_test_db(&recipes_state());
        let recipes = list_recipes(&conn, None).unwrap();
        assert_eq!(recipes.len(), 2);
    }

    #[test]
    fn test_list_recipes_by_visibility() {
        let conn = build_test_db(&recipes_state());
        let recipes = list_recipes(&conn, Some("public")).unwrap();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].slug.as_deref(), Some("summary-recipe"));
    }

    #[test]
    fn test_show_recipe_by_id() {
        let conn = build_test_db(&recipes_state());
        let recipe = show_recipe(&conn, "r-1").unwrap().unwrap();
        assert_eq!(recipe.slug.as_deref(), Some("summary-recipe"));
        assert!(recipe.config.is_some());
        assert_eq!(
            recipe.config.as_ref().unwrap().model.as_deref(),
            Some("gpt-4")
        );
    }

    #[test]
    fn test_show_recipe_by_slug() {
        let conn = build_test_db(&recipes_state());
        let recipe = show_recipe(&conn, "my-recipe").unwrap().unwrap();
        assert_eq!(recipe.id.as_deref(), Some("r-2"));
    }

    #[test]
    fn test_show_recipe_not_found() {
        let conn = build_test_db(&recipes_state());
        let recipe = show_recipe(&conn, "nonexistent").unwrap();
        assert!(recipe.is_none());
    }
}
