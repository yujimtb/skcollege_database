//! Notion API client and SaaSWriteAdapter implementation.
//!
//! Ported from skcollege_dictionary/NotionService.js — stacking update
//! algorithm, page property sync, and content block rendering.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::adapter::error::AdapterError;
use crate::adapter::writeback::traits::{
    SaaSWriteAdapter, WriteAction, WriteRecord, WriteResult,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Notion adapter configuration.
#[derive(Debug, Clone)]
pub struct NotionConfig {
    /// Notion integration token (Bearer).
    pub token: String,
    /// Target database ID for student directory pages.
    pub database_id: String,
    /// Local blob directory used to source Notion file uploads.
    pub blob_dir: Option<PathBuf>,
    /// Notion API version header.
    pub api_version: String,
}

impl NotionConfig {
    pub fn new(token: impl Into<String>, database_id: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            database_id: database_id.into(),
            blob_dir: None,
            api_version: "2022-06-28".into(),
        }
    }

    pub fn with_blob_dir(mut self, blob_dir: impl Into<PathBuf>) -> Self {
        self.blob_dir = Some(blob_dir.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Notion Client
// ---------------------------------------------------------------------------

/// HTTP-based Notion API client implementing SaaSWriteAdapter.
#[derive(Clone)]
pub struct NotionClient {
    http: Client,
    config: NotionConfig,
    schema: DatabaseSchema,
}

#[derive(Debug, Clone)]
struct DatabaseSchema {
    title_property: String,
    email_property: Option<String>,
    properties: HashMap<String, NotionProperty>,
    actual_names_by_normalized: HashMap<String, String>,
}

impl DatabaseSchema {
    fn resolve_property(&self, candidates: &[&str]) -> Option<(&str, &NotionProperty)> {
        for candidate in candidates {
            let normalized = normalize_property_name(candidate);
            let Some(actual_name) = self.actual_names_by_normalized.get(&normalized) else {
                continue;
            };
            let Some(property) = self.properties.get(actual_name) else {
                continue;
            };
            return Some((actual_name.as_str(), property));
        }
        None
    }
}

impl NotionClient {
    const BASE_URL: &'static str = "https://api.notion.com/v1";
    const FILE_UPLOAD_API_VERSION: &'static str = "2026-03-11";

    pub fn new(config: NotionConfig) -> Result<Self, AdapterError> {
        let http = Client::builder()
            .build()
            .map_err(|err| AdapterError::Network {
                message: err.to_string(),
            })?;
        let schema = Self::load_database_schema(&http, &config)?;
        Ok(Self { http, config, schema })
    }

    fn auth_headers_for_version(&self, api_version: &str) -> Result<HeaderMap, AdapterError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.config.token))
                .map_err(|err| AdapterError::AuthFailure {
                    message: format!("invalid Notion bearer token header: {err}"),
                })?,
        );
        headers.insert(
            "Notion-Version",
            HeaderValue::from_str(api_version)
                .map_err(|err| AdapterError::Other(format!(
                    "invalid Notion-Version header: {err}"
                )))?,
        );
        Ok(headers)
    }

    fn headers_for_version(&self, api_version: &str) -> Result<HeaderMap, AdapterError> {
        let mut headers = self.auth_headers_for_version(api_version)?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    /// Low-level API call.
    fn api_call<T: DeserializeOwned>(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, AdapterError> {
        self.api_call_with_version(method, endpoint, body, &self.config.api_version)
    }

    fn api_call_with_version<T: DeserializeOwned>(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&serde_json::Value>,
        api_version: &str,
    ) -> Result<T, AdapterError> {
        let url = format!("{}{}", Self::BASE_URL, endpoint);
        let request = match method {
            "GET" => self.http.get(&url),
            "POST" => self.http.post(&url),
            "PATCH" => self.http.patch(&url),
            "DELETE" => self.http.delete(&url),
            _ => return Err(AdapterError::Other(format!("unsupported method: {method}"))),
        };

        let request = request.headers(self.headers_for_version(api_version)?);
        let request = if let Some(body) = body {
            request.json(body)
        } else {
            request
        };

        let response = request.send().map_err(|err| AdapterError::Network {
            message: err.to_string(),
        })?;

        Self::decode_response(response)
    }

    fn decode_response<T: DeserializeOwned>(response: Response) -> Result<T, AdapterError> {
        let status = response.status();
        if status.as_u16() == 429 {
            return Err(AdapterError::RateLimited {
                retry_after_secs: 1,
            });
        }
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(AdapterError::AuthFailure {
                message: format!("Notion API {status}"),
            });
        }
        if status.is_client_error() || status.is_server_error() {
            let body_text = response.text().unwrap_or_default();
            return Err(AdapterError::Other(format!(
                "Notion API error ({status}): {body_text}"
            )));
        }

        response.json::<T>().map_err(|err| AdapterError::MalformedResponse {
            message: err.to_string(),
        })
    }

    fn load_database_schema(http: &Client, config: &NotionConfig) -> Result<DatabaseSchema, AdapterError> {
        let client = Self {
            http: http.clone(),
            config: config.clone(),
            schema: DatabaseSchema {
                title_property: "Name".to_string(),
                email_property: Some("Email".to_string()),
                properties: HashMap::new(),
                actual_names_by_normalized: HashMap::new(),
            },
        };
        let database: NotionDatabase = client.api_call("GET", &format!("/databases/{}", config.database_id), None)?;

        let mut title_property = None;
        let mut email_property = None;
        let mut properties = HashMap::new();
        let mut actual_names_by_normalized = HashMap::new();

        for (name, property) in database.properties {
            actual_names_by_normalized
                .entry(normalize_property_name(&name))
                .or_insert_with(|| name.clone());
            if property.property_type == "title" && title_property.is_none() {
                title_property = Some(name.clone());
            }
            if property.property_type == "email" && email_property.is_none() {
                email_property = Some(name.clone());
            }
            properties.insert(name, property);
        }

        Ok(DatabaseSchema {
            title_property: title_property.unwrap_or_else(|| "Name".to_string()),
            email_property,
            properties,
            actual_names_by_normalized,
        })
    }

    fn find_page(&self, email: Option<&str>, title: &str) -> Result<Option<NotionPage>, AdapterError> {
        let filter = if let (Some(email), Some(email_property)) = (email, self.schema.email_property.as_ref()) {
            serde_json::json!({
                "filter": {
                    "property": email_property,
                    "email": {
                        "equals": email,
                    }
                }
            })
        } else {
            serde_json::json!({
                "filter": {
                    "property": self.schema.title_property,
                    "title": {
                        "equals": title,
                    }
                }
            })
        };
        let result: NotionQueryResult =
            self.api_call("POST", &format!("/databases/{}/query", self.config.database_id), Some(&filter))?;
        Ok(result.results.into_iter().next())
    }

    /// Create a new page in the database.
    fn create_page(&self, title: &str, email: &str) -> Result<NotionPage, AdapterError> {
        let mut properties = serde_json::Map::new();
        properties.insert(
            self.schema.title_property.clone(),
            serde_json::json!({
                "title": [{ "text": { "content": title } }]
            }),
        );
        if let Some(email_property) = &self.schema.email_property {
            properties.insert(
                email_property.clone(),
                serde_json::json!({
                    "email": email
                }),
            );
        }
        let payload = serde_json::json!({
            "parent": { "database_id": self.config.database_id },
            "properties": properties
        });
        self.api_call("POST", "/pages", Some(&payload))
    }

    /// Update page properties (metadata fields).
    fn update_page_properties(
        &self,
        page_id: &str,
        properties: &serde_json::Value,
    ) -> Result<(), AdapterError> {
        let payload = serde_json::json!({ "properties": properties });
        let _: serde_json::Value = self.api_call("PATCH", &format!("/pages/{page_id}"), Some(&payload))?;
        Ok(())
    }

    /// Get child blocks of a page/block.
    fn get_children(&self, block_id: &str) -> Result<Vec<NotionBlock>, AdapterError> {
        let result: NotionBlockChildren =
            self.api_call("GET", &format!("/blocks/{block_id}/children"), None)?;
        Ok(result.results)
    }

    /// Delete a block.
    fn delete_block(&self, block_id: &str) -> Result<(), AdapterError> {
        let _: serde_json::Value = self.api_call("DELETE", &format!("/blocks/{block_id}"), None)?;
        Ok(())
    }

    /// Append children blocks to a page/block.
    fn append_children(
        &self,
        block_id: &str,
        children: &[serde_json::Value],
    ) -> Result<Vec<NotionBlock>, AdapterError> {
        let payload = serde_json::json!({ "children": children });
        let api_version = if children.iter().any(|child| {
            child
                .get("image")
                .and_then(|image| image.get("type"))
                .and_then(|value| value.as_str())
                == Some("file_upload")
        }) {
            Self::FILE_UPLOAD_API_VERSION
        } else {
            &self.config.api_version
        };
        let result: NotionBlockChildren = self.api_call_with_version(
            "PATCH",
            &format!("/blocks/{block_id}/children"),
            Some(&payload),
            api_version,
        )?;
        Ok(result.results)
    }

    fn upload_file(&self, filename: &str, content_type: &str, bytes: &[u8]) -> Result<String, AdapterError> {
        let create_payload = serde_json::json!({
            "filename": filename,
            "content_type": content_type,
            "content_length": bytes.len(),
        });
        let created: NotionFileUpload = self.api_call_with_version(
            "POST",
            "/file_uploads",
            Some(&create_payload),
            Self::FILE_UPLOAD_API_VERSION,
        )?;
        let upload_id = created.id;

        let file_part = Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(content_type)
            .map_err(|err| AdapterError::Other(format!("invalid upload mime type: {err}")))?;
        let form = Form::new().part("file", file_part);
        let response = self
            .http
            .post(format!("{}/file_uploads/{upload_id}/send", Self::BASE_URL))
            .headers(self.auth_headers_for_version(Self::FILE_UPLOAD_API_VERSION)?)
            .multipart(form)
            .send()
            .map_err(|err| AdapterError::Network {
                message: err.to_string(),
            })?;
        let uploaded: NotionFileUpload = Self::decode_response(response)?;
        if uploaded.status != "uploaded" {
            return Err(AdapterError::Other(format!(
                "Notion file upload {upload_id} ended with unexpected status {}",
                uploaded.status
            )));
        }
        Ok(upload_id)
    }

    fn load_blob_bytes(&self, blob_ref: &str) -> Result<Vec<u8>, AdapterError> {
        let hash = blob_ref_sha256(blob_ref)
            .ok_or_else(|| AdapterError::Other(format!("invalid thumbnail blob ref: {blob_ref}")))?;
        let blob_dir = self.config.blob_dir.as_deref().ok_or_else(|| {
            AdapterError::Other(
                "Notion file upload requires blob_dir in configuration".to_string(),
            )
        })?;
        let blob_path = blob_dir.join(hash);
        std::fs::read(&blob_path).map_err(|err| {
            AdapterError::Other(format!(
                "failed to read thumbnail blob {}: {err}",
                blob_path.display()
            ))
        })
    }

    fn build_thumbnail_block(
        &self,
        payload: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AdapterError> {
        if let Some(blob_ref) = payload.get("thumbnail_blob_ref").and_then(|value| value.as_str()) {
            let hash = blob_ref_sha256(blob_ref)
                .ok_or_else(|| AdapterError::Other(format!("invalid thumbnail blob ref: {blob_ref}")))?;
            let bytes = self.load_blob_bytes(blob_ref)?;
            let upload_id = self.upload_file(
                &format!("lethe-thumbnail-{}.png", &hash[..8]),
                "image/png",
                &bytes,
            )?;
            return Ok(Some(thumbnail_file_upload_block(&upload_id)));
        }

        Ok(payload
            .get("thumbnail_url")
            .and_then(|v| v.as_str())
            .filter(|url| url.starts_with("http"))
            .map(thumbnail_external_block))
    }

    /// Build the stacking update: replace the bot section with new content,
    /// archiving previous content into a toggle block.
    ///
    /// Ported from NotionService.js `updateBotSection`.
    fn stacking_update(
        &self,
        page_id: &str,
        payload: &serde_json::Value,
    ) -> Result<(), AdapterError> {
        let blocks = self.get_children(page_id)?;
        let thumbnail_block = self.build_thumbnail_block(payload)?;

        // Find the bot-managed section marker and collect blocks to replace.
        let mut delete_queue = Vec::new();
        let mut previous_content_blocks = Vec::new();

        if let Some(marker_index) = blocks.iter().position(is_bot_section_marker) {
            if marker_index > 0 && is_managed_thumbnail_block(&blocks[marker_index - 1]) {
                delete_queue.push(blocks[marker_index - 1].id.clone());
            }

            delete_queue.push(blocks[marker_index].id.clone());

            for block in blocks.iter().skip(marker_index + 1) {
                if let Some(archived_block) = sanitize_archived_block(block) {
                    previous_content_blocks.push(archived_block);
                }
                delete_queue.push(block.id.clone());
            }
        }

        // Delete existing bot section blocks
        for block_id in &delete_queue {
            self.delete_block(block_id)?;
        }

        // Build new content blocks
        let mut children = Vec::new();

        if let Some(thumbnail_block) = thumbnail_block {
            children.push(thumbnail_block);
        }

        // Marker for the bot-managed section. Keep it non-textual so page content
        // does not show a synthetic heading.
        children.push(serde_json::json!({
            "object": "block",
            "type": "divider",
            "divider": {}
        }));

        // New content
        children.extend(render_payload_blocks(payload));

        // Archive previous content
        if !previous_content_blocks.is_empty() {
            let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
            children.push(serde_json::json!({
                "object": "block",
                "type": "toggle",
                "toggle": {
                    "rich_text": [{ "text": { "content": format!("📜 Archive: {date}") } }],
                    "children": previous_content_blocks
                }
            }));
        }

        self.append_children(page_id, &children)?;
        Ok(())
    }

    /// Convert student profile payload to Notion property updates.
    fn build_property_updates(&self, title: &str, payload: &serde_json::Value) -> serde_json::Value {
        let props = payload.get("properties").cloned().unwrap_or_default();
        let mut notion_props = serde_json::Map::new();

        if !title.trim().is_empty() {
            notion_props.insert(
                self.schema.title_property.clone(),
                serde_json::json!({
                    "title": [{ "text": { "content": title.trim() } }]
                }),
            );
        }

        if let Some(email_property) = &self.schema.email_property {
            if let Some(email) = payload
                .get("email")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("generated_email").and_then(|v| v.as_str()))
                .filter(|value| !value.trim().is_empty())
            {
                notion_props.insert(
                    email_property.clone(),
                    serde_json::json!({ "email": email.trim() }),
                );
            }
        }

        let add_text = |map: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: Option<String>| {
            if let Some(value) = value.filter(|text| !text.trim().is_empty()) {
                map.insert(
                    key.to_string(),
                    serde_json::json!({ "rich_text": [{ "text": { "content": value } }] }),
                );
            }
        };

        let add_text_if_exists = |
            map: &mut serde_json::Map<String, serde_json::Value>,
            candidates: &[&str],
            value: Option<String>,
        | {
            let Some(value) = value.filter(|text| !text.trim().is_empty()) else {
                return;
            };
            let Some((property_name, property)) = self.schema.resolve_property(candidates) else {
                return;
            };
            match property.property_type.as_str() {
                "url" if value.starts_with("http://") || value.starts_with("https://") => {
                    map.insert(property_name.to_string(), serde_json::json!({ "url": value }));
                }
                "email" if value.contains('@') => {
                    map.insert(property_name.to_string(), serde_json::json!({ "email": value }));
                }
                "date" => {
                    map.insert(property_name.to_string(), serde_json::json!({
                        "date": {
                            "start": value,
                        }
                    }));
                }
                "status" => {
                    map.insert(property_name.to_string(), serde_json::json!({
                        "status": {
                            "name": value,
                        }
                    }));
                }
                _ => add_text(map, property_name, Some(value)),
            }
        };

        let add_checkbox_if_exists = |
            map: &mut serde_json::Map<String, serde_json::Value>,
            candidates: &[&str],
            value: Option<bool>,
        | {
            let Some(value) = value else {
                return;
            };
            let Some((property_name, property)) = self.schema.resolve_property(candidates) else {
                return;
            };
            if property.property_type == "checkbox" {
                map.insert(
                    property_name.to_string(),
                    serde_json::json!({ "checkbox": value }),
                );
            }
        };

        add_text_if_exists(&mut notion_props, &["Birthplace"], json_text(&props["Birthplace"]));
        add_text_if_exists(&mut notion_props, &["DoB"], json_text(&props["DoB"]));

        let tag_str = combine_list_texts([
            json_list_values(props.get("Hashtags")),
            json_list_values(payload.get("attributes")),
        ]);
        add_text_if_exists(&mut notion_props, &["Hashtag"], tag_str.clone());
        add_text_if_exists(&mut notion_props, &["Hashtags"], tag_str);

        // Merge Major + Interests
        let mut major_interests = Vec::new();
        if let Some(major) = props.get("Major").and_then(|v| v.as_str()) {
            if !major.is_empty() {
                major_interests.push(major.to_string());
            }
        }
        if let Some(interests) = props.get("Interests").and_then(|v| v.as_array()) {
            for val in interests {
                if let Some(s) = val.as_str() {
                    major_interests.push(s.to_string());
                }
            }
        }
        add_text_if_exists(
            &mut notion_props,
            &["Major_Interests", "Major_interests"],
            combine_list_texts([major_interests]),
        );

        for key in [
            "Nickname",
            "Major",
            "Affiliation",
            "MBTI",
            "SNS",
            "Dislikes",
            "New Challenges",
            "Ask Me About",
            "Turning Point",
            "BTW",
            "Message",
        ] {
            add_text_if_exists(&mut notion_props, &[key], json_text(&props[key]));
        }

        for key in ["Hobbies", "Interests", "Likes"] {
            add_text_if_exists(&mut notion_props, &[key], json_list_text(props.get(key)));
        }

        add_text_if_exists(
            &mut notion_props,
            &["Attributes"],
            json_list_text(payload.get("attributes")),
        );
        add_text_if_exists(
            &mut notion_props,
            &["LETHE Person ID"],
            metadata_value(payload, "person_id"),
        );
        add_text_if_exists(
            &mut notion_props,
            &["Source Slide URL"],
            metadata_str(payload, "source_slide_url")
                .or_else(|| payload.get("source_canonical_uri").and_then(|value| value.as_str()))
                .map(ToOwned::to_owned),
        );
        add_text_if_exists(
            &mut notion_props,
            &["Last Synced At"],
            metadata_value(payload, "last_synced_at"),
        );
        add_text_if_exists(
            &mut notion_props,
            &["Projection Version"],
            metadata_value(payload, "projection_version"),
        );
        add_text_if_exists(
            &mut notion_props,
            &["Status"],
            metadata_value(payload, "status"),
        );
        add_checkbox_if_exists(
            &mut notion_props,
            &["Visibility"],
            metadata_bool(payload, "visibility"),
        );

        serde_json::Value::Object(notion_props)
    }
}

// ---------------------------------------------------------------------------
// SaaSWriteAdapter implementation
// ---------------------------------------------------------------------------

impl SaaSWriteAdapter for NotionClient {
    fn write_record(&self, record: &WriteRecord) -> Result<WriteResult, AdapterError> {
        let email = record
            .payload
            .get("email")
            .and_then(|v| v.as_str())
            .or_else(|| record.payload.get("generated_email").and_then(|v| v.as_str()))
            .unwrap_or(&record.entity_id);

        // Find or create the page
        let (page_id, action) = if let Some(ext_id) = &record.external_id {
            (ext_id.clone(), WriteAction::Updated)
        } else {
            match self.find_page(Some(email), &record.title)? {
                Some(page) => (page.id.clone(), WriteAction::Updated),
                None => {
                    let page = self.create_page(&record.title, email)?;
                    (page.id.clone(), WriteAction::Created)
                }
            }
        };

        // Update page properties
        let property_updates = self.build_property_updates(&record.title, &record.payload);
        if property_updates.as_object().is_some_and(|m| !m.is_empty()) {
            self.update_page_properties(&page_id, &property_updates)?;
        }

        // Stacking update of content blocks
        self.stacking_update(&page_id, &record.payload)?;

        let url = format!("https://www.notion.so/{}", page_id.replace('-', ""));
        Ok(WriteResult {
            external_id: page_id,
            action,
            url: Some(url),
        })
    }

    fn find_existing(&self, entity_id: &str) -> Result<Option<String>, AdapterError> {
        Ok(self.find_page(Some(entity_id), entity_id)?.map(|p| p.id))
    }

    fn delete_record(&self, external_id: &str) -> Result<(), AdapterError> {
        // Notion "deletes" by archiving a page
        let payload = serde_json::json!({ "in_trash": true });
        let _: serde_json::Value = self.api_call_with_version(
            "PATCH",
            &format!("/pages/{external_id}"),
            Some(&payload),
            Self::FILE_UPLOAD_API_VERSION,
        )?;
        Ok(())
    }

    fn adapter_name(&self) -> &str {
        "notion"
    }
}

// ---------------------------------------------------------------------------
// Notion API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct NotionPage {
    pub id: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionQueryResult {
    #[serde(default)]
    results: Vec<NotionPage>,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionFileUpload {
    id: String,
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionDatabase {
    #[serde(default)]
    properties: HashMap<String, NotionProperty>,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionProperty {
    #[serde(rename = "type")]
    property_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotionBlock {
    pub id: String,
    #[serde(rename = "type", default)]
    pub block_type: String,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionBlockChildren {
    #[serde(default)]
    results: Vec<NotionBlock>,
}

fn blob_ref_sha256(blob_ref: &str) -> Option<&str> {
    let hash = blob_ref.strip_prefix("blob:sha256:")?;
    if hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(hash)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Helpers: content block rendering
// ---------------------------------------------------------------------------

/// Check if a block marks the start of the bot-managed section.
/// Accept both the legacy heading marker and the current divider marker.
fn is_bot_section_marker(block: &NotionBlock) -> bool {
    if block.block_type == "divider" {
        return true;
    }
    if block.block_type != "heading_2" {
        return false;
    }
    block
        .raw
        .get("heading_2")
        .and_then(|h| h.get("rich_text"))
        .and_then(|rt| rt.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("plain_text").or_else(|| item.get("text").and_then(|t| t.get("content"))))
        .and_then(|v| v.as_str())
        .is_some_and(|text| text == "🤖 From Google Slide")
}

fn is_managed_thumbnail_block(block: &NotionBlock) -> bool {
    block.block_type == "image"
}

fn thumbnail_external_block(url: &str) -> serde_json::Value {
    serde_json::json!({
        "object": "block",
        "type": "image",
        "image": { "type": "external", "external": { "url": url } }
    })
}

fn thumbnail_file_upload_block(file_upload_id: &str) -> serde_json::Value {
    serde_json::json!({
        "object": "block",
        "type": "image",
        "image": { "type": "file_upload", "file_upload": { "id": file_upload_id } }
    })
}

fn sanitize_archived_block(block: &NotionBlock) -> Option<serde_json::Value> {
    let text = extract_plain_text(&block.raw);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(serde_json::json!({
        "object": "block",
        "type": "paragraph",
        "paragraph": {
            "rich_text": [{ "text": { "content": trimmed } }]
        }
    }))
}

fn extract_plain_text(value: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    collect_plain_text(value, &mut parts);
    parts.join(" ")
}

fn collect_plain_text(value: &serde_json::Value, parts: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("plain_text").and_then(|v| v.as_str()) {
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
            }
            if let Some(text) = map
                .get("text")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str())
            {
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
            }
            for child in map.values() {
                collect_plain_text(child, parts);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_plain_text(child, parts);
            }
        }
        _ => {}
    }
}

/// Render student profile payload into Notion block JSON.
/// Ported from NotionService.js `_addPayloadToChildren`.
fn render_payload_blocks(data: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    let props = data.get("properties").cloned().unwrap_or_default();

    let mut source_lines = Vec::new();
    if let Some(email) = data
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("generated_email").and_then(|v| v.as_str()))
    {
        source_lines.push(format!("Email: {email}"));
    }
    if let Some(uri) = data.get("source_canonical_uri").and_then(|v| v.as_str()) {
        source_lines.push(format!("Google Slides: {uri}"));
    }
    if let Some(document_id) = data.get("source_document_id").and_then(|v| v.as_str()) {
        source_lines.push(format!("Document ID: {document_id}"));
    }
    if !source_lines.is_empty() {
        blocks.push(serde_json::json!({
            "object": "block",
            "type": "callout",
            "callout": {
                "rich_text": [{ "text": { "content": source_lines.join("\n") } }],
                "icon": { "emoji": "🔎" },
                "color": "blue_background"
            }
        }));
    }

    // Profile pic
    if let Some(pic) = data.get("profile_pic") {
        if let Some(url) = pic.get("url").and_then(|v| v.as_str()) {
            if url.starts_with("http") {
                blocks.push(serde_json::json!({
                    "object": "block",
                    "type": "image",
                    "image": { "type": "external", "external": { "url": url } }
                }));
            }
        } else if let Some(desc) = pic.get("description").and_then(|v| v.as_str()) {
            blocks.push(serde_json::json!({
                "object": "block",
                "type": "callout",
                "callout": {
                    "rich_text": [{ "text": { "content": format!("🖼️ Profile Pic: {desc}") } }],
                    "icon": { "emoji": "👤" }
                }
            }));
        }
    }

    // Basic info callout
    let mut info_lines = Vec::new();
    if let Some(nick) = props.get("Nickname").and_then(|v| v.as_str()) {
        info_lines.push(format!("📛 Nickname: {nick}"));
    }
    if let Some(aff) = props.get("Affiliation").and_then(|v| v.as_str()) {
        info_lines.push(format!("🏢 Affiliation: {aff}"));
    }
    if let Some(mbti) = props.get("MBTI").and_then(|v| v.as_str()) {
        info_lines.push(format!("🧠 MBTI: {mbti}"));
    }
    if let Some(dislikes) = props.get("Dislikes").and_then(|v| v.as_str()) {
        info_lines.push(format!("🙅 Dislikes: {dislikes}"));
    }
    if !info_lines.is_empty() {
        blocks.push(serde_json::json!({
            "object": "block",
            "type": "callout",
            "callout": {
                "rich_text": [{ "text": { "content": info_lines.join("\n") } }],
                "icon": { "emoji": "ℹ️" },
                "color": "gray_background"
            }
        }));
    }

    // Bio text
    if let Some(bio) = data.get("bio_text").and_then(|v| v.as_str()) {
        if !bio.is_empty() {
            blocks.push(serde_json::json!({
                "object": "block",
                "type": "heading_3",
                "heading_3": { "rich_text": [{ "text": { "content": "About" } }] }
            }));
            blocks.push(serde_json::json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": {
                    "rich_text": [{ "text": { "content": bio } }]
                }
            }));
        }
    }

    let highlights = [
        ("Hobbies", json_list_text(props.get("Hobbies"))),
        ("Interests", json_list_text(props.get("Interests"))),
        ("Hashtags", json_list_text(props.get("Hashtags"))),
        ("Attributes", json_list_text(data.get("attributes"))),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.map(|value| format!("{label}: {value}")))
    .collect::<Vec<_>>();
    if !highlights.is_empty() {
        blocks.push(serde_json::json!({
            "object": "block",
            "type": "heading_3",
            "heading_3": { "rich_text": [{ "text": { "content": "Highlights" } }] }
        }));
        blocks.push(serde_json::json!({
            "object": "block",
            "type": "callout",
            "callout": {
                "rich_text": [{ "text": { "content": highlights.join("\n") } }],
                "icon": { "emoji": "✨" },
                "color": "yellow_background"
            }
        }));
    }

    // Likes
    if let Some(likes) = props.get("Likes") {
        let likes_str = if let Some(arr) = likes.as_array() {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" • ")
        } else {
            likes.as_str().unwrap_or_default().to_string()
        };
        if !likes_str.is_empty() {
            blocks.push(serde_json::json!({
                "object": "block",
                "type": "heading_3",
                "heading_3": { "rich_text": [{ "text": { "content": "❤️ Likes & Favorites" } }] }
            }));
            blocks.push(serde_json::json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": { "rich_text": [{ "text": { "content": likes_str } }] }
            }));
        }
    }

    // Message & SNS
    let mut msg_content = props
        .get("Message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if let Some(sns) = props.get("SNS").and_then(|v| v.as_str()) {
        if !sns.is_empty() {
            msg_content.push_str(&format!("\n\n🔗 SNS: {sns}"));
        }
    }
    if !msg_content.trim().is_empty() {
        blocks.push(serde_json::json!({
            "object": "block",
            "type": "quote",
            "quote": {
                "rich_text": [{ "text": { "content": msg_content } }],
                "color": "default"
            }
        }));
    }

    // Narrative fields
    let narrative_fields = [
        ("New Challenges", "🔥 New Challenges"),
        ("Ask Me About", "💬 Ask Me About"),
        ("Turning Point", "🔄 Turning Point"),
        ("BTW", "👀 BTW"),
    ];
    for (key, label) in &narrative_fields {
        if let Some(text) = props.get(*key).and_then(|v| v.as_str()) {
            if !text.is_empty() {
                blocks.push(serde_json::json!({
                    "object": "block",
                    "type": "heading_3",
                    "heading_3": { "rich_text": [{ "text": { "content": *label } }] }
                }));
                blocks.push(serde_json::json!({
                    "object": "block",
                    "type": "paragraph",
                    "paragraph": { "rich_text": [{ "text": { "content": text } }] }
                }));
            }
        }
    }

    // Gallery images
    if let Some(gallery) = data.get("gallery_images").and_then(|v| v.as_array()) {
        if !gallery.is_empty() {
            blocks.push(serde_json::json!({
                "object": "block",
                "type": "heading_3",
                "heading_3": { "rich_text": [{ "text": { "content": "📷 Gallery" } }] }
            }));
            for img in gallery {
                if let Some(url) = img.get("url").and_then(|v| v.as_str()) {
                    if url.starts_with("http") {
                        blocks.push(serde_json::json!({
                            "object": "block",
                            "type": "image",
                            "image": { "type": "external", "external": { "url": url } }
                        }));
                    }
                }
                if let Some(desc) = img.get("description").and_then(|v| v.as_str()) {
                    if !desc.is_empty() {
                        blocks.push(serde_json::json!({
                            "object": "block",
                            "type": "paragraph",
                            "paragraph": { "rich_text": [{ "text": { "content": desc } }] }
                        }));
                    }
                }
            }
        }
    }

    // Attributes (fallback if no Likes)
    let has_likes = props.get("Likes").is_some();
    if !has_likes {
        if let Some(attrs) = data.get("attributes").and_then(|v| v.as_array()) {
            if !attrs.is_empty() {
                let tag_str: String = attrs
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if !tag_str.is_empty() {
                    blocks.push(serde_json::json!({
                        "object": "block",
                        "type": "paragraph",
                        "paragraph": {
                            "rich_text": [{ "text": { "content": format!("Tags: {tag_str}") } }]
                        }
                    }));
                }
            }
        }
    }

    blocks
}

fn json_text(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

fn json_list_values(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        array
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        json_text(value).into_iter().collect()
    }
}

fn json_list_text(value: Option<&serde_json::Value>) -> Option<String> {
    combine_list_texts([json_list_values(value)])
}

fn combine_list_texts<const N: usize>(groups: [Vec<String>; N]) -> Option<String> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for value in groups.into_iter().flatten() {
        let key = normalize_property_name(&value);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        merged.push(value);
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged.join(", "))
    }
}

fn normalize_property_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn metadata_pointer<'a>(payload: &'a serde_json::Value, field: &str) -> Option<&'a serde_json::Value> {
    payload.pointer(&format!("/_lethe/{field}"))
}

fn metadata_str<'a>(payload: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    metadata_pointer(payload, field).and_then(|value| value.as_str())
}

fn metadata_value(payload: &serde_json::Value, field: &str) -> Option<String> {
    metadata_str(payload, field).map(ToOwned::to_owned)
}

fn metadata_bool(payload: &serde_json::Value, field: &str) -> Option<bool> {
    metadata_pointer(payload, field).and_then(|value| value.as_bool())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_client(person_id_property_name: &str) -> NotionClient {
        let properties = [
            ("Birthplace", "rich_text"),
            ("Major_interests", "rich_text"),
            ("Hashtag", "rich_text"),
            ("Source Slide URL", "url"),
            ("Last Synced At", "date"),
            ("Projection Version", "rich_text"),
            ("Status", "status"),
            ("Visibility", "checkbox"),
        ]
        .into_iter()
        .chain(std::iter::once((person_id_property_name, "rich_text")))
        .map(|(name, property_type)| {
            (
                name.to_string(),
                NotionProperty {
                    property_type: property_type.to_string(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
        NotionClient {
            http: Client::builder().build().unwrap(),
            config: NotionConfig::new("test-token", "test-db"),
            schema: DatabaseSchema {
                title_property: "Name".into(),
                email_property: Some("Email".into()),
                actual_names_by_normalized: [
                    "Birthplace",
                    "Major_interests",
                    "Hashtag",
                    "Source Slide URL",
                    "Last Synced At",
                    "Projection Version",
                    "Status",
                    "Visibility",
                ]
                .into_iter()
                .chain(std::iter::once(person_id_property_name))
                .map(|name| (normalize_property_name(name), name.to_string()))
                .collect(),
                properties,
            },
        }
    }

    #[test]
    fn render_payload_with_profile_pic() {
        let payload = serde_json::json!({
            "profile_pic": {
                "url": "https://example.com/pic.jpg",
                "description": "Photo"
            },
            "email": "taro@example.jp",
            "source_canonical_uri": "https://docs.google.com/presentation/d/test",
            "bio_text": "Hello world",
            "properties": {
                "Nickname": "Taro",
                "MBTI": "ENFP"
            }
        });
        let blocks = render_payload_blocks(&payload);
        assert!(blocks.len() >= 5);
        assert_eq!(blocks[0]["type"], "callout");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[2]["type"], "callout");
    }

    #[test]
    fn render_payload_with_gallery() {
        let payload = serde_json::json!({
            "gallery_images": [
                { "url": "https://example.com/g1.jpg", "description": "Photo 1" },
                { "url": "https://example.com/g2.jpg", "description": "Photo 2" },
            ],
            "properties": {}
        });
        let blocks = render_payload_blocks(&payload);
        // heading_3 + 2 * (image + paragraph) = 5
        assert_eq!(blocks.len(), 5);
    }

    #[test]
    fn render_payload_with_narrative() {
        let payload = serde_json::json!({
            "properties": {
                "New Challenges": "Learning Rust",
                "Ask Me About": "Cooking",
            }
        });
        let blocks = render_payload_blocks(&payload);
        assert_eq!(blocks.len(), 4); // 2 * (heading + paragraph)
    }

    #[test]
    fn build_property_updates_merges_major_interests() {
        let payload = serde_json::json!({
            "properties": {
                "Major": "CS",
                "Interests": ["AI", "Robotics"],
                "Birthplace": "Tokyo",
            }
        });
        let props = fixture_client("LETHE Person ID").build_property_updates("田中太郎", &payload);
        assert!(props.get("Major_interests").is_some());
        assert!(props.get("Birthplace").is_some());
        assert!(props.get("Name").is_some());
    }

    #[test]
    fn build_property_updates_populates_metadata_and_attribute_fallbacks() {
        let payload = serde_json::json!({
            "attributes": ["AI", "ML"],
            "_lethe": {
                "person_id": "person:alice",
                "projection_version": "proj:person-page@0.1.0",
                "last_synced_at": "2026-03-28T11:00:00Z",
                "source_slide_url": "https://example.com/slide",
                "status": "Done",
                "visibility": true
            },
            "properties": {
                "Hashtags": ["#rust"],
                "Major": "CS",
                "Interests": ["Robotics"]
            }
        });

        let props = fixture_client("LETHE Person ID").build_property_updates("田中太郎", &payload);

        assert_eq!(
            props["Hashtag"]["rich_text"][0]["text"]["content"].as_str(),
            Some("#rust, AI, ML")
        );
        assert_eq!(
            props["LETHE Person ID"]["rich_text"][0]["text"]["content"].as_str(),
            Some("person:alice")
        );
        assert_eq!(
            props["Source Slide URL"]["url"].as_str(),
            Some("https://example.com/slide")
        );
        assert_eq!(
            props["Last Synced At"]["date"]["start"].as_str(),
            Some("2026-03-28T11:00:00Z")
        );
        assert_eq!(
            props["Projection Version"]["rich_text"][0]["text"]["content"].as_str(),
            Some("proj:person-page@0.1.0")
        );
        assert_eq!(props["Status"]["status"]["name"].as_str(), Some("Done"));
        assert_eq!(props["Visibility"]["checkbox"].as_bool(), Some(true));
    }

    #[test]
    fn render_payload_adds_highlights_section() {
        let payload = serde_json::json!({
            "attributes": ["AI", "ML"],
            "properties": {
                "Hobbies": ["写真", "散歩"],
                "Hashtags": ["rust", "slides"]
            }
        });
        let blocks = render_payload_blocks(&payload);
        assert!(blocks.iter().any(|block| block["type"] == "heading_3" && block.to_string().contains("Highlights")));
    }

    #[test]
    fn is_bot_section_marker_detects_legacy_heading() {
        let block = NotionBlock {
            id: "test".into(),
            block_type: "heading_2".into(),
            raw: serde_json::json!({
                "heading_2": {
                    "rich_text": [{ "text": { "content": "🤖 From Google Slide" }, "plain_text": "🤖 From Google Slide" }]
                }
            }),
        };
        assert!(is_bot_section_marker(&block));
    }

    #[test]
    fn is_bot_section_marker_detects_divider() {
        let block = NotionBlock {
            id: "test".into(),
            block_type: "divider".into(),
            raw: serde_json::json!({
                "divider": {}
            }),
        };
        assert!(is_bot_section_marker(&block));
    }

    #[test]
    fn is_bot_section_marker_rejects_other() {
        let block = NotionBlock {
            id: "test".into(),
            block_type: "heading_2".into(),
            raw: serde_json::json!({
                "heading_2": {
                    "rich_text": [{ "text": { "content": "Other Heading" }, "plain_text": "Other Heading" }]
                }
            }),
        };
        assert!(!is_bot_section_marker(&block));
    }

    #[test]
    fn headers_reject_invalid_bearer_token() {
        let mut client = fixture_client("LETHE Person ID");
        client.config.token = "bad\r\ntoken".into();
        assert!(matches!(
            client.headers_for_version(&client.config.api_version),
            Err(AdapterError::AuthFailure { .. })
        ));
    }

    #[test]
    fn headers_reject_invalid_api_version() {
        let mut client = fixture_client("LETHE Person ID");
        client.config.api_version = "bad\r\nversion".into();
        assert!(matches!(
            client.headers_for_version(&client.config.api_version),
            Err(AdapterError::Other(_))
        ));
    }

    #[test]
    fn thumbnail_file_upload_block_uses_file_upload_type() {
        let block = thumbnail_file_upload_block("upload-123");
        assert_eq!(block["type"], "image");
        assert_eq!(block["image"]["type"], "file_upload");
        assert_eq!(block["image"]["file_upload"]["id"], "upload-123");
    }

    #[test]
    fn thumbnail_external_block_uses_external_type() {
        let block = thumbnail_external_block("https://example.com/thumb.png");
        assert_eq!(block["type"], "image");
        assert_eq!(block["image"]["type"], "external");
        assert_eq!(block["image"]["external"]["url"], "https://example.com/thumb.png");
    }
}
