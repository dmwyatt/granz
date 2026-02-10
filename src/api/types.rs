use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::models::{CalendarEvent, Document, Recipe, TranscriptUtterance};

// ============================================================================
// Transcript Request/Response
// ============================================================================

/// Request body for fetching a transcript
#[derive(Debug, Serialize)]
pub struct GetTranscriptRequest {
    pub document_id: String,
}

/// Container for transcript API response
/// Note: The API returns a raw array, which we wrap in this struct in client.rs
#[derive(Debug)]
pub struct TranscriptResponse {
    pub transcript: Vec<TranscriptUtterance>,
}

// ============================================================================
// Document Request/Response
// ============================================================================

/// Request body for fetching documents
#[derive(Debug, Serialize, Default)]
pub struct GetDocumentsRequest {
    /// Optional: fetch a specific document by ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Response from get-documents-v2 API
#[derive(Debug, Deserialize)]
pub struct GetDocumentsResponse {
    pub docs: Vec<Document>,
}

// ============================================================================
// People Request
// ============================================================================

/// Request body for fetching people (empty object)
#[derive(Debug, Serialize, Default)]
pub struct GetPeopleRequest {}

// ============================================================================
// Calendar Request/Response
// ============================================================================

/// Request body for fetching selected calendars (empty object)
#[derive(Debug, Serialize, Default)]
pub struct GetSelectedCalendarsRequest {}

/// Response from get-selected-calendars API
#[derive(Debug, Deserialize)]
pub struct GetSelectedCalendarsResponse {
    #[serde(default)]
    pub calendars_selected: Option<HashMap<String, bool>>,
    #[serde(default)]
    pub enabled_calendars: Option<Vec<String>>,
}

/// Request body for refreshing calendar events (empty object)
#[derive(Debug, Serialize, Default)]
pub struct RefreshCalendarEventsRequest {}

/// Response from refresh-calendar-events API
#[derive(Debug, Deserialize)]
pub struct RefreshCalendarEventsResponse {
    #[serde(default)]
    pub results: Option<CalendarEventsResults>,
}

/// The results object containing calendar events
#[derive(Debug, Deserialize)]
pub struct CalendarEventsResults {
    #[serde(default)]
    pub events: Vec<CalendarEvent>,
}

// ============================================================================
// Template Request
// ============================================================================

/// Request body for fetching panel templates (empty object)
#[derive(Debug, Serialize, Default)]
pub struct GetPanelTemplatesRequest {}

// ============================================================================
// Recipe Request/Response
// ============================================================================

/// Request body for fetching recipes (empty object)
#[derive(Debug, Serialize, Default)]
pub struct GetRecipesRequest {}

/// Response from get-recipes API
#[derive(Debug, Deserialize)]
pub struct GetRecipesResponse {
    #[serde(default, rename = "defaultRecipes")]
    pub default_recipes: Vec<Recipe>,
    #[serde(default, rename = "publicRecipes")]
    pub public_recipes: Vec<Recipe>,
    #[serde(default, rename = "userRecipes")]
    pub user_recipes: Vec<Recipe>,
    #[serde(default, rename = "sharedRecipes")]
    pub shared_recipes: Vec<Recipe>,
    #[serde(default, rename = "unlistedRecipes")]
    pub unlisted_recipes: Vec<Recipe>,
}

// ============================================================================
// Panel Types (API-specific — structurally different from model Panel)
// ============================================================================

/// Request body for fetching document panels
#[derive(Debug, Serialize)]
pub struct GetDocumentPanelsRequest {
    pub document_id: String,
}

/// A panel (AI-generated note section) as returned by the API.
///
/// This differs from `models::Panel` in that the API returns `content` as
/// structured TipTap JSON, while the model stores `content_markdown` (the
/// converted output) and `content_json` (the raw string).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiPanel {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub original_content: Option<Value>,
    #[serde(default)]
    pub template_slug: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PanelTemplate, Person, RecipeConfig};

    // ========================================================================
    // Transcript Tests
    // ========================================================================

    #[test]
    fn test_deserialize_utterance_array() {
        let json = r#"[
            {
                "id": "u-1",
                "document_id": "doc-1",
                "start_timestamp": "2026-01-20T10:00:00Z",
                "end_timestamp": "2026-01-20T10:00:30Z",
                "text": "Hello everyone",
                "source": "system",
                "is_final": true
            }
        ]"#;

        let utterances: Vec<TranscriptUtterance> = serde_json::from_str(json).unwrap();
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].id, Some("u-1".to_string()));
        assert_eq!(utterances[0].text, Some("Hello everyone".to_string()));
        assert_eq!(utterances[0].source, Some("system".to_string()));
        assert_eq!(utterances[0].is_final, Some(true));
        assert!(
            !utterances[0].extra.contains_key("source"),
            "source should be a direct field, not in extra"
        );
        assert!(
            !utterances[0].extra.contains_key("is_final"),
            "is_final should be a direct field, not in extra"
        );
    }

    #[test]
    fn test_deserialize_empty_array() {
        let json = r#"[]"#;
        let utterances: Vec<TranscriptUtterance> = serde_json::from_str(json).unwrap();
        assert!(utterances.is_empty());
    }

    #[test]
    fn test_deserialize_missing_optional_fields() {
        let json = r#"[{"id": "u-1"}]"#;

        let utterances: Vec<TranscriptUtterance> = serde_json::from_str(json).unwrap();
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].id, Some("u-1".to_string()));
        assert!(utterances[0].text.is_none());
    }

    // ========================================================================
    // Document Tests
    // ========================================================================

    #[test]
    fn test_deserialize_documents_response() {
        let json = r#"{
            "docs": [
                {
                    "id": "doc-1",
                    "title": "Test Meeting",
                    "type": "meeting",
                    "created_at": "2026-01-20T10:00:00Z"
                }
            ]
        }"#;

        let response: GetDocumentsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.docs.len(), 1);
        assert_eq!(response.docs[0].id, Some("doc-1".to_string()));
        assert_eq!(response.docs[0].title, Some("Test Meeting".to_string()));
    }

    #[test]
    fn test_deserialize_document_with_people() {
        let json = r#"{
            "id": "doc-1",
            "title": "Test Meeting",
            "people": {
                "title": "Meeting Title",
                "creator": {
                    "name": "Alice",
                    "email": "alice@example.com"
                },
                "attendees": [
                    {"email": "bob@example.com", "name": "Bob"}
                ]
            }
        }"#;

        let doc: Document = serde_json::from_str(json).unwrap();
        assert_eq!(doc.id, Some("doc-1".to_string()));
        let people = doc.people.unwrap();
        assert_eq!(people.creator.as_ref().unwrap().name, Some("Alice".to_string()));
        assert_eq!(people.attendees.as_ref().unwrap().len(), 1);
    }

    // ========================================================================
    // People Tests
    // ========================================================================

    #[test]
    fn test_deserialize_person() {
        let json = r#"{
            "id": "person-1",
            "name": "Alice Smith",
            "email": "alice@example.com",
            "company_name": "Acme Corp",
            "job_title": "Engineer"
        }"#;

        let person: Person = serde_json::from_str(json).unwrap();
        assert_eq!(person.id, Some("person-1".to_string()));
        assert_eq!(person.name, Some("Alice Smith".to_string()));
        assert_eq!(person.company_name, Some("Acme Corp".to_string()));
    }

    #[test]
    fn test_deserialize_people_array() {
        let json = r#"[
            {"id": "p-1", "name": "Alice"},
            {"id": "p-2", "name": "Bob"}
        ]"#;

        let people: Vec<Person> = serde_json::from_str(json).unwrap();
        assert_eq!(people.len(), 2);
    }

    // ========================================================================
    // Calendar Tests
    // ========================================================================

    #[test]
    fn test_deserialize_calendar_event() {
        let json = r#"{
            "id": "event-1",
            "summary": "Team Standup",
            "start": {
                "dateTime": "2026-01-29T10:00:00-06:00",
                "timeZone": "America/Chicago"
            },
            "end": {
                "dateTime": "2026-01-29T10:30:00-06:00",
                "timeZone": "America/Chicago"
            },
            "calendarId": "user@example.com"
        }"#;

        let event: CalendarEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.id, Some("event-1".to_string()));
        assert_eq!(event.summary, Some("Team Standup".to_string()));
        assert_eq!(
            event.start.as_ref().unwrap().time_zone,
            Some("America/Chicago".to_string())
        );
    }

    #[test]
    fn test_deserialize_refresh_calendar_events_response() {
        let json = r#"{
            "message": "Refreshed 1 user.",
            "results": {
                "events": [
                    {"id": "e-1", "summary": "Meeting"}
                ]
            }
        }"#;

        let response: RefreshCalendarEventsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.results.as_ref().unwrap().events.len(), 1);
    }

    #[test]
    fn test_deserialize_selected_calendars_response() {
        let json = r#"{
            "calendars_selected": {"user@example.com": true},
            "enabled_calendars": ["google"]
        }"#;

        let response: GetSelectedCalendarsResponse = serde_json::from_str(json).unwrap();
        assert!(response.calendars_selected.as_ref().unwrap().contains_key("user@example.com"));
        assert_eq!(response.enabled_calendars.as_ref().unwrap()[0], "google");
    }

    // ========================================================================
    // Template Tests
    // ========================================================================

    #[test]
    fn test_deserialize_panel_template() {
        let json = r#"{
            "id": "template-1",
            "title": "Meeting Notes",
            "category": "General",
            "is_granola": true,
            "sections": [
                {
                    "id": "s-1",
                    "heading": "Summary",
                    "section_description": "Summarize the key points"
                }
            ]
        }"#;

        let template: PanelTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(template.id, Some("template-1".to_string()));
        assert_eq!(template.is_granola, Some(true));
        assert_eq!(template.sections.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_deserialize_template_array() {
        let json = r#"[
            {"id": "t-1", "title": "Notes"},
            {"id": "t-2", "title": "Interview"}
        ]"#;

        let templates: Vec<PanelTemplate> = serde_json::from_str(json).unwrap();
        assert_eq!(templates.len(), 2);
    }

    // ========================================================================
    // Recipe Tests
    // ========================================================================

    #[test]
    fn test_deserialize_recipe() {
        let json = r#"{
            "id": "recipe-1",
            "slug": "test-recipe",
            "visibility": "public",
            "config": {
                "model": "gpt-4o",
                "description": "A test recipe",
                "instructions": "Do something"
            }
        }"#;

        let recipe: Recipe = serde_json::from_str(json).unwrap();
        assert_eq!(recipe.id, Some("recipe-1".to_string()));
        assert_eq!(recipe.slug, Some("test-recipe".to_string()));
        assert_eq!(recipe.config.as_ref().unwrap().model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_deserialize_recipes_response() {
        let json = r#"{
            "defaultRecipes": [],
            "publicRecipes": [{"id": "r-1", "slug": "public-recipe"}],
            "userRecipes": [{"id": "r-2", "slug": "my-recipe"}],
            "sharedRecipes": [],
            "unlistedRecipes": [],
            "recipesUsage": {}
        }"#;

        let response: GetRecipesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.default_recipes.len(), 0);
        assert_eq!(response.public_recipes.len(), 1);
        assert_eq!(response.public_recipes[0].slug, Some("public-recipe".to_string()));
        assert_eq!(response.user_recipes.len(), 1);
        assert_eq!(response.user_recipes[0].slug, Some("my-recipe".to_string()));
        assert_eq!(response.shared_recipes.len(), 0);
        assert_eq!(response.unlisted_recipes.len(), 0);
    }

    #[test]
    fn test_deserialize_recipe_config_with_explicit_fields() {
        let json = r#"{
            "name": "My Recipe",
            "icon": "sparkle",
            "model": "gpt-4o",
            "description": "A test recipe",
            "instructions": "Do something",
            "trigger": "auto",
            "output_format": "markdown",
            "enabled": true,
            "unknown_field": "captured"
        }"#;

        let config: RecipeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, Some("My Recipe".to_string()));
        assert_eq!(config.icon, Some("sparkle".to_string()));
        assert_eq!(config.model, Some("gpt-4o".to_string()));
        assert_eq!(config.description, Some("A test recipe".to_string()));
        assert_eq!(config.instructions, Some("Do something".to_string()));
        assert_eq!(config.trigger, Some("auto".to_string()));
        assert_eq!(config.output_format, Some("markdown".to_string()));
        assert_eq!(config.enabled, Some(true));
        assert!(
            !config.extra.contains_key("name"),
            "name should be a direct field, not in extra"
        );
        assert!(
            !config.extra.contains_key("icon"),
            "icon should be a direct field, not in extra"
        );
        assert!(
            !config.extra.contains_key("trigger"),
            "trigger should be a direct field, not in extra"
        );
        assert!(
            !config.extra.contains_key("output_format"),
            "output_format should be a direct field, not in extra"
        );
        assert!(
            !config.extra.contains_key("enabled"),
            "enabled should be a direct field, not in extra"
        );
        assert!(config.extra.contains_key("unknown_field"));
    }

    // ========================================================================
    // Panel Tests
    // ========================================================================

    #[test]
    fn test_deserialize_panel() {
        let json = r#"{
            "id": "panel-1",
            "document_id": "doc-1",
            "title": "Summary",
            "content": {"type": "doc", "content": []},
            "template_slug": "meeting-notes"
        }"#;

        let panel: ApiPanel = serde_json::from_str(json).unwrap();
        assert_eq!(panel.id, Some("panel-1".to_string()));
        assert_eq!(panel.document_id, Some("doc-1".to_string()));
        assert_eq!(panel.title, Some("Summary".to_string()));
        assert!(panel.content.is_some());
    }

    #[test]
    fn test_deserialize_panel_minimal() {
        let json = r#"{"id": "panel-1"}"#;
        let panel: ApiPanel = serde_json::from_str(json).unwrap();
        assert_eq!(panel.id, Some("panel-1".to_string()));
        assert!(panel.title.is_none());
        assert!(panel.content.is_none());
    }

    #[test]
    fn test_deserialize_panels_array() {
        let json = r#"[
            {"id": "p-1", "title": "Summary"},
            {"id": "p-2", "title": "Action Items"}
        ]"#;
        let panels: Vec<ApiPanel> = serde_json::from_str(json).unwrap();
        assert_eq!(panels.len(), 2);
    }

    #[test]
    fn test_document_with_unknown_fields() {
        let json = r#"{
            "id": "doc-1",
            "title": "Test",
            "unknown_field": "should be captured",
            "another_unknown": 42
        }"#;

        let doc: Document = serde_json::from_str(json).unwrap();
        assert_eq!(doc.id, Some("doc-1".to_string()));
        assert!(doc.extra.contains_key("unknown_field"));
        assert!(doc.extra.contains_key("another_unknown"));
    }
}
