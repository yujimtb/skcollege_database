//! Notion API client and SaaSWriteAdapter implementation.
//!
//! Ported from skcollege_dictionary/NotionService.js — stacking update
//! algorithm, page property sync, and content block rendering.

use std::collections::{HashMap, HashSet};

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
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
    /// Notion API version header.
    pub api_version: String,
}

impl NotionConfig {
    pub fn new(token: impl Into<String>, database_id: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            database_id: database_id.into(),
            api_version: "2022-06-28".into(),
        }
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
    property_names: HashSet<String>,
}

impl NotionClient {
    const BASE_URL: &'static str = "https://api.notion.com/v1";

    pub fn new(config: NotionConfig) -> Result<Self, AdapterError> {
        let http = Client::builder()
            .build()
            .map_err(|err| AdapterError::Network {
                message: err.to_string(),
            })?;
        let schema = Self::load_database_schema(&http, &config)?;
        Ok(Self { http, config, schema })
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.config.token))
                .unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "Notion-Version",
            HeaderValue::from_str(&self.config.api_version)
                .unwrap_or_else(|_| HeaderValue::from_static("2022-06-28")),
        );
        headers
    }

    /// Low-level API call.
    fn api_call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, AdapterError> {
        let url = format!("{}{}", Self::BASE_URL, endpoint);
        let request = match method {
            "GET" => self.http.get(&url),
            "POST" => self.http.post(&url),
            "PATCH" => self.http.patch(&url),
            "DELETE" => self.http.delete(&url),
            _ => return Err(AdapterError::Other(format!("unsupported method: {method}"))),
        };

        let request = request.headers(self.headers());
        let request = if let Some(body) = body {
            request.json(body)
        } else {
            request
        };

        let response = request.send().map_err(|err| AdapterError::Network {
            message: err.to_string(),
        })?;

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
                property_names: HashSet::new(),
            },
        };
        let database: NotionDatabase = client.api_call("GET", &format!("/databases/{}", config.database_id), None)?;

        let mut title_property = None;
        let mut email_property = None;
        let mut property_names = HashSet::new();

        for (name, property) in database.properties {
            property_names.insert(name.clone());
            if property.property_type == "title" && title_property.is_none() {
                title_property = Some(name.clone());
            }
            if property.property_type == "email" && email_property.is_none() {
                email_property = Some(name.clone());
            }
        }

        Ok(DatabaseSchema {
            title_property: title_property.unwrap_or_else(|| "Name".to_string()),
            email_property,
            property_names,
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
        let result: NotionBlockChildren =
            self.api_call("PATCH", &format!("/blocks/{block_id}/children"), Some(&payload))?;
        Ok(result.results)
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
            let _ = self.delete_block(block_id);
        }

        // Build new content blocks
        let mut children = Vec::new();

        if let Some(slide_image_url) = payload.get("thumbnail_url").and_then(|v| v.as_str()) {
            if slide_image_url.starts_with("http") {
                children.push(serde_json::json!({
                    "object": "block",
                    "type": "image",
                    "image": { "type": "external", "external": { "url": slide_image_url } }
                }));
            }
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
    fn build_property_updates(&self, payload: &serde_json::Value) -> serde_json::Value {
        let props = payload.get("properties").cloned().unwrap_or_default();
        let mut notion_props = serde_json::Map::new();

        let add_text = |map: &mut serde_json::Map<String, serde_json::Value>, key: &str, val: &serde_json::Value| {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    map.insert(
                        key.to_string(),
                        serde_json::json!({ "rich_text": [{ "text": { "content": s } }] }),
                    );
                }
            }
        };

        if self.schema.property_names.contains("Birthplace") {
            add_text(&mut notion_props, "Birthplace", &props["Birthplace"]);
        }
        if self.schema.property_names.contains("DoB") {
            add_text(&mut notion_props, "DoB", &props["DoB"]);
        }

        // Hashtags: join array into comma-separated text
        if let Some(tags) = props.get("Hashtags") {
            let tag_str = if let Some(arr) = tags.as_array() {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                tags.as_str().unwrap_or_default().to_string()
            };
            if !tag_str.is_empty() && self.schema.property_names.contains("Hashtag") {
                notion_props.insert(
                    "Hashtag".to_string(),
                    serde_json::json!({ "rich_text": [{ "text": { "content": tag_str } }] }),
                );
            }
        }

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
        if !major_interests.is_empty() && self.schema.property_names.contains("Major_Interests") {
            notion_props.insert(
                "Major_Interests".to_string(),
                serde_json::json!({ "rich_text": [{ "text": { "content": major_interests.join(", ") } }] }),
            );
        }

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
        let property_updates = self.build_property_updates(&record.payload);
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
        let payload = serde_json::json!({ "archived": true });
        let _: serde_json::Value = self.api_call("PATCH", &format!("/pages/{external_id}"), Some(&payload))?;
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
                "type": "paragraph",
                "paragraph": {
                    "rich_text": [{ "text": { "content": bio } }]
                }
            }));
        }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_client() -> NotionClient {
        NotionClient {
            http: Client::builder().build().unwrap(),
            config: NotionConfig::new("test-token", "test-db"),
            schema: DatabaseSchema {
                title_property: "Name".into(),
                email_property: Some("Email".into()),
                property_names: ["Birthplace", "Major_Interests", "Hashtag"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
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
            "bio_text": "Hello world",
            "properties": {
                "Nickname": "Taro",
                "MBTI": "ENFP"
            }
        });
        let blocks = render_payload_blocks(&payload);
        assert!(blocks.len() >= 3); // image + callout + bio
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[1]["type"], "callout");
        assert_eq!(blocks[2]["type"], "paragraph");
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
        let props = fixture_client().build_property_updates(&payload);
        assert!(props.get("Major_Interests").is_some());
        assert!(props.get("Birthplace").is_some());
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
}
