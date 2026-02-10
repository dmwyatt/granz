use crate::models::{Document, DocumentPeople};

pub(crate) struct DocumentRow {
    pub id: Option<String>,
    pub title: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub deleted_at: Option<String>,
    pub doc_type: Option<String>,
    pub notes_plain: Option<String>,
    pub notes_markdown: Option<String>,
    pub summary: Option<String>,
    pub people_json: Option<String>,
    pub google_calendar_event_json: Option<String>,
}

pub(crate) fn row_to_document(row: DocumentRow) -> Document {
    let people: Option<DocumentPeople> = row
        .people_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let google_calendar_event = row
        .google_calendar_event_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    Document {
        id: row.id,
        title: row.title,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted_at: row.deleted_at,
        doc_type: row.doc_type,
        notes_plain: row.notes_plain,
        notes_markdown: row.notes_markdown,
        summary: row.summary,
        people,
        google_calendar_event,
        user_id: None,
        notes: None,
        workspace_id: None,
        visibility: None,
        creation_source: None,
        privacy_mode_enabled: None,
        status: None,
        sharing_link_visibility: None,
        extra: Default::default(),
    }
}
