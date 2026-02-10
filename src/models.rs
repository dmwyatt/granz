//! Domain types for grans.
//!
//! These types are the single source of truth for all domain data. They are used
//! for both API deserialization (via `#[serde(default)]`) and database output.
//! API-specific request/response wrappers live in `api::types`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ============================================================================
// Document Types
// ============================================================================

/// A meeting document
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Document {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(rename = "type", default)]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub notes_plain: Option<String>,
    #[serde(default)]
    pub notes_markdown: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub people: Option<DocumentPeople>,
    #[serde(default)]
    pub google_calendar_event: Option<Value>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub notes: Option<Value>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub creation_source: Option<String>,
    #[serde(default)]
    pub privacy_mode_enabled: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub sharing_link_visibility: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// People associated with a meeting
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentPeople {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub creator: Option<DocumentCreator>,
    #[serde(default)]
    pub attendees: Option<Vec<DocumentAttendee>>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub sharing_link_visibility: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Creator information in a document
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentCreator {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub details: Option<Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Attendee information in a document
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentAttendee {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub details: Option<AttendeeDetails>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl DocumentAttendee {
    pub fn full_name(&self) -> Option<&str> {
        self.details
            .as_ref()
            .and_then(|d| d.person.as_ref())
            .and_then(|p| p.name.as_ref())
            .and_then(|n| n.full_name.as_deref())
    }
}

/// Attendee details
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AttendeeDetails {
    #[serde(default)]
    pub person: Option<PersonDetails>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Person details within attendee
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonDetails {
    #[serde(default)]
    pub name: Option<PersonName>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Person name
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonName {
    #[serde(rename = "fullName", default)]
    pub full_name: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Transcript Types
// ============================================================================

/// Filter transcript utterances by speaker
#[derive(Debug, Clone, PartialEq)]
pub enum SpeakerFilter {
    Me,
    Other,
}

impl SpeakerFilter {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "me" => Some(SpeakerFilter::Me),
            "other" => Some(SpeakerFilter::Other),
            _ => None,
        }
    }

    pub fn matches(&self, source: Option<&str>) -> bool {
        match (self, source) {
            (SpeakerFilter::Me, Some("microphone")) => true,
            (SpeakerFilter::Other, Some("system")) => true,
            _ => false,
        }
    }
}

/// A transcript utterance
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TranscriptUtterance {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub start_timestamp: Option<String>,
    #[serde(default)]
    pub end_timestamp: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub is_final: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// People Types
// ============================================================================

/// A person/contact
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Person {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub job_title: Option<String>,
    #[serde(default)]
    pub company_name: Option<String>,
    #[serde(default)]
    pub company_description: Option<String>,
    #[serde(default)]
    pub user_type: Option<String>,
    #[serde(default)]
    pub subscription_name: Option<String>,
    #[serde(default)]
    pub links: Option<Vec<Value>>,
    #[serde(default)]
    pub favorite_panel_templates: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Calendar Types
// ============================================================================

/// A calendar
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Calendar {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub primary: Option<bool>,
    #[serde(default)]
    pub access_role: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub background_color: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A calendar event
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CalendarEvent {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub start: Option<EventDateTime>,
    #[serde(default)]
    pub end: Option<EventDateTime>,
    #[serde(default)]
    pub attendees: Option<Vec<EventAttendee>>,
    #[serde(default)]
    pub creator: Option<EventCreator>,
    #[serde(default)]
    pub organizer: Option<EventOrganizer>,
    #[serde(default, alias = "conferenceData")]
    pub conference_data: Option<Value>,
    #[serde(default, alias = "recurringEventId")]
    pub recurring_event_id: Option<String>,
    #[serde(default, alias = "iCalUID")]
    pub ical_uid: Option<String>,
    #[serde(default, alias = "calendarId")]
    pub calendar_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, alias = "htmlLink")]
    pub html_link: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Date/time with timezone
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventDateTime {
    #[serde(default, rename = "dateTime")]
    pub date_time: Option<String>,
    #[serde(default, rename = "timeZone")]
    pub time_zone: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Event attendee
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventAttendee {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, alias = "responseStatus")]
    pub response_status: Option<String>,
    #[serde(default, alias = "self")]
    pub is_self: Option<bool>,
    #[serde(default)]
    pub organizer: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Event creator
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventCreator {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Event organizer
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventOrganizer {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Panel Types
// ============================================================================

/// An AI-generated panel (meeting notes section) — local/database representation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Panel {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content_markdown: Option<String>,
    #[serde(default)]
    pub content_json: Option<String>,
    #[serde(default)]
    pub template_slug: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub chat_url: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Template Types
// ============================================================================

/// A panel template
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PanelTemplate {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_granola: Option<bool>,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub sections: Option<Vec<TemplateSection>>,
    #[serde(default)]
    pub chat_suggestions: Option<Vec<ChatSuggestion>>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub shared_with: Option<Value>,
    #[serde(default)]
    pub copied_from: Option<String>,
    #[serde(default)]
    pub user_types: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A chat suggestion for templates
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatSuggestion {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Template section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TemplateSection {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub heading: Option<String>,
    #[serde(default)]
    pub section_description: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Recipe Types
// ============================================================================

/// A recipe
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Recipe {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub publisher_slug: Option<String>,
    #[serde(default)]
    pub creator_name: Option<String>,
    #[serde(default)]
    pub config: Option<RecipeConfig>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub shared_with: Option<Value>,
    #[serde(default)]
    pub creation_context: Option<String>,
    #[serde(default)]
    pub source_recipe_id: Option<String>,
    #[serde(default)]
    pub creator_avatar: Option<String>,
    #[serde(default)]
    pub creator_info: Option<Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Recipe configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecipeConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub output_format: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub examples: Option<Vec<Value>>,
    #[serde(default)]
    pub allowed_views: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_recipe_views: Option<Vec<String>>,
    #[serde(default)]
    pub generate_artifact: Option<bool>,
    #[serde(default)]
    pub show_in_shared_tabs: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_filter_parse_me() {
        assert_eq!(SpeakerFilter::parse("me"), Some(SpeakerFilter::Me));
        assert_eq!(SpeakerFilter::parse("ME"), Some(SpeakerFilter::Me));
        assert_eq!(SpeakerFilter::parse("Me"), Some(SpeakerFilter::Me));
    }

    #[test]
    fn speaker_filter_parse_other() {
        assert_eq!(SpeakerFilter::parse("other"), Some(SpeakerFilter::Other));
        assert_eq!(SpeakerFilter::parse("OTHER"), Some(SpeakerFilter::Other));
    }

    #[test]
    fn speaker_filter_parse_invalid() {
        assert_eq!(SpeakerFilter::parse("unknown"), None);
        assert_eq!(SpeakerFilter::parse(""), None);
    }

    #[test]
    fn speaker_filter_matches_me() {
        let filter = SpeakerFilter::Me;
        assert!(filter.matches(Some("microphone")));
        assert!(!filter.matches(Some("system")));
        assert!(!filter.matches(None));
    }

    #[test]
    fn speaker_filter_matches_other() {
        let filter = SpeakerFilter::Other;
        assert!(filter.matches(Some("system")));
        assert!(!filter.matches(Some("microphone")));
        assert!(!filter.matches(None));
    }
}
