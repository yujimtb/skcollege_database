//! M10 — Slack → Observation mapper + SlackAdapter implementation
//!
//! Pure mapping is in `map_message` / `map_channel_snapshot`.
//! IO (fetch) is delegated to the `SlackClient` trait.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::adapter::config::AdapterConfig;
use crate::adapter::heartbeat::heartbeat_draft;
use crate::adapter::idempotency::*;
use crate::adapter::traits::*;
use crate::domain::{
    AuthorityModel, BlobRef, CaptureModel, EntityRef, ObserverRef, SchemaRef, SemVer,
    SourceSystemRef,
};

use super::client::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const SLACK_MESSAGE_SCHEMA: &str = "schema:slack-message";
pub const SLACK_MESSAGE_SCHEMA_VERSION: &str = "1.0.0";
pub const SLACK_CHANNEL_SCHEMA: &str = "schema:slack-channel-snapshot";
pub const SLACK_CHANNEL_SCHEMA_VERSION: &str = "1.0.0";

const OBSERVER_ID: &str = "obs:slack-crawler";
const SOURCE_SYSTEM: &str = "sys:slack";

// ---------------------------------------------------------------------------
// SlackAdapter
// ---------------------------------------------------------------------------

pub struct SlackAdapter<C: SlackClient> {
    pub client: C,
    pub config: AdapterConfig,
    /// Per-channel cursor: channel_id → last known ts.
    pub cursors: HashMap<String, String>,
    pub last_successful_capture: Option<DateTime<Utc>>,
}

impl<C: SlackClient> SlackAdapter<C> {
    pub fn new(client: C, config: AdapterConfig) -> Self {
        Self {
            client,
            config,
            cursors: HashMap::new(),
            last_successful_capture: None,
        }
    }

    /// Map a single Slack message to an ObservationDraft.
    pub fn map_message(&self, msg: &SlackMessage) -> ObservationDraft {
        let idem_key = match msg.message_type {
            SlackMessageType::Edit => {
                let edit_ts = msg
                    .edited
                    .as_ref()
                    .map(|e| e.ts.as_str())
                    .unwrap_or("unknown");
                slack_edit_key(&msg.channel_id, &msg.ts, edit_ts)
            }
            SlackMessageType::Delete => slack_delete_key(&msg.channel_id, &msg.ts),
            _ => slack_message_key(&msg.channel_id, &msg.ts),
        };

        let subject = EntityRef::new(format!(
            "message:slack:{}-{}",
            msg.channel_id, msg.ts
        ));

        let mut meta = serde_json::json!({
            "sourceAdapterVersion": self.config.adapter_version.as_str(),
        });

        if msg.message_type == SlackMessageType::Delete {
            meta["retracts"] = serde_json::json!(format!(
                "message:slack:{}-{}",
                msg.channel_id, msg.ts
            ));
        }

        let mut payload = serde_json::json!({
            "channel_id": msg.channel_id,
            "channel_name": msg.channel_name,
            "ts": msg.ts,
            "user_id": msg.user_id,
            "user_name": msg.user_name,
            "text": msg.text,
            "message_type": msg.message_type,
        });

        if let Some(ref email) = msg.email {
            payload["email"] = serde_json::json!(email);
        }

        if let Some(ref thread_ts) = msg.thread_ts {
            payload["thread_ts"] = serde_json::json!(thread_ts);
        }
        if let Some(ref edited) = msg.edited {
            payload["edited"] = serde_json::json!({
                "user": edited.user,
                "ts": edited.ts,
            });
        }
        if !msg.reactions.is_empty() {
            payload["reactions"] = serde_json::to_value(&msg.reactions).unwrap_or_default();
        }
        if !msg.files.is_empty() {
            payload["files"] = serde_json::to_value(&msg.files).unwrap_or_default();
        }
        if msg.reply_count > 0 {
            payload["reply_count"] = serde_json::json!(msg.reply_count);
            payload["reply_users_count"] = serde_json::json!(msg.reply_users_count);
        }

        let attachments: Vec<BlobRef> = msg
            .files
            .iter()
            .filter_map(|f| f.blob_ref.as_ref().map(|r| BlobRef::new(r.clone())))
            .collect();

        // Parse the Slack ts to a DateTime.
        let published = parse_slack_ts(&msg.ts).unwrap_or_else(Utc::now);

        ObservationDraft {
            schema: SchemaRef::new(SLACK_MESSAGE_SCHEMA),
            schema_version: SemVer::new(SLACK_MESSAGE_SCHEMA_VERSION),
            observer: ObserverRef::new(OBSERVER_ID),
            source_system: Some(SourceSystemRef::new(SOURCE_SYSTEM)),
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject,
            target: None,
            payload,
            attachments,
            published,
            idempotency_key: idem_key,
            meta,
        }
    }

    /// Map a Slack channel snapshot to an ObservationDraft.
    pub fn map_channel_snapshot(&self, snap: &SlackChannelSnapshot) -> ObservationDraft {
        let idem_key = crate::domain::IdempotencyKey::new(format!(
            "slack:channel:{}:snapshot:{}",
            snap.channel_id,
            snap.snapshot_at.format("%Y-%m-%dT%H:%M")
        ));

        let payload = serde_json::json!({
            "channel_id": snap.channel_id,
            "channel_name": snap.channel_name,
            "purpose": snap.purpose,
            "topic": snap.topic,
            "member_count": snap.member_count,
            "members": snap.members,
            "is_archived": snap.is_archived,
            "snapshot_at": snap.snapshot_at,
        });

        ObservationDraft {
            schema: SchemaRef::new(SLACK_CHANNEL_SCHEMA),
            schema_version: SemVer::new(SLACK_CHANNEL_SCHEMA_VERSION),
            observer: ObserverRef::new(OBSERVER_ID),
            source_system: Some(SourceSystemRef::new(SOURCE_SYSTEM)),
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject: EntityRef::new(format!("channel:slack:{}", snap.channel_id)),
            target: None,
            payload,
            attachments: vec![],
            published: snap.snapshot_at,
            idempotency_key: idem_key,
            meta: serde_json::json!({
                "sourceAdapterVersion": self.config.adapter_version.as_str(),
            }),
        }
    }

    /// Update the per-channel cursor after a successful fetch.
    pub fn update_cursor(&mut self, channel_id: &str, latest_ts: &str) {
        self.cursors
            .insert(channel_id.to_string(), latest_ts.to_string());
    }

    /// Get the current cursor for a channel.
    pub fn get_cursor(&self, channel_id: &str) -> Option<&str> {
        self.cursors.get(channel_id).map(String::as_str)
    }
}

impl<C: SlackClient> SourceAdapter for SlackAdapter<C> {
    fn fetch_incremental(&self, cursor: Option<&Cursor>) -> FetchResult {
        let oldest = cursor.map(|c| c.value.as_str());
        match self
            .client
            .conversations_history("default", oldest, None, 200)
        {
            Ok(page) => {
                let items: Vec<RawData> = page
                    .messages
                    .iter()
                    .map(|m| RawData {
                        data: serde_json::to_value(m).unwrap_or_default(),
                        blobs: vec![],
                    })
                    .collect();
                let next_cursor = page.next_cursor.map(|v| Cursor {
                    value: v,
                    updated_at: Utc::now(),
                });
                FetchResult::Ok {
                    items,
                    next_cursor,
                    has_more: page.has_more,
                }
            }
            Err(e) => FetchResult::Error(e),
        }
    }

    fn fetch_snapshot(&self, target_id: &str) -> FetchResult {
        match self.client.conversations_info(target_id) {
            Ok(snap) => FetchResult::Ok {
                items: vec![RawData {
                    data: serde_json::to_value(&snap).unwrap_or_default(),
                    blobs: vec![],
                }],
                next_cursor: None,
                has_more: false,
            },
            Err(e) => FetchResult::Error(e),
        }
    }

    fn to_observations(&self, raw: &RawData) -> Vec<ObservationDraft> {
        // Try to deserialize as SlackMessage first, then as channel snapshot.
        if let Ok(msg) = serde_json::from_value::<SlackMessage>(raw.data.clone()) {
            vec![self.map_message(&msg)]
        } else if let Ok(snap) =
            serde_json::from_value::<SlackChannelSnapshot>(raw.data.clone())
        {
            vec![self.map_channel_snapshot(&snap)]
        } else {
            vec![]
        }
    }

    fn heartbeat(&self) -> ObservationDraft {
        heartbeat_draft(
            &ObserverRef::new(OBSERVER_ID),
            &SourceSystemRef::new(SOURCE_SYSTEM),
            Utc::now(),
            0,
            self.last_successful_capture,
        )
    }

    fn observer_ref(&self) -> &ObserverRef {
        &self.config.observer_id
    }

    fn source_system_ref(&self) -> &SourceSystemRef {
        &self.config.source_system_id
    }
}

/// Parse Slack's epoch.micro timestamp to DateTime<Utc>.
fn parse_slack_ts(ts: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = ts.split('.').collect();
    let secs: i64 = parts.first()?.parse().ok()?;
    let micros: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    DateTime::from_timestamp(secs, micros * 1000)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::config::*;
    use std::time::Duration;

    fn test_config() -> AdapterConfig {
        AdapterConfig {
            observer_id: ObserverRef::new(OBSERVER_ID),
            source_system_id: SourceSystemRef::new(SOURCE_SYSTEM),
            adapter_version: SemVer::new("1.0.0"),
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            schemas: vec![
                SchemaRef::new(SLACK_MESSAGE_SCHEMA),
                SchemaRef::new(SLACK_CHANNEL_SCHEMA),
            ],
            schema_bindings: vec![SchemaBinding {
                schema: SchemaRef::new(SLACK_MESSAGE_SCHEMA),
                versions: ">=1.0.0 <2.0.0".into(),
            }],
            poll_interval: Duration::from_secs(300),
            heartbeat_interval: Duration::from_secs(60),
            rate_limit: RateLimitConfig {
                requests_per_second: 50,
                burst: 10,
            },
            retry: RetryConfig {
                max_retries: 3,
                backoff: BackoffStrategy::Exponential,
                max_wait: Duration::from_secs(30),
            },
            credential_ref: "secret:slack-token".into(),
        }
    }

    fn sample_message() -> SlackMessage {
        SlackMessage {
            channel_id: "C01ABC".into(),
            channel_name: "general".into(),
            ts: "1234567890.123456".into(),
            thread_ts: None,
            user_id: "U01XYZ".into(),
            user_name: "tanaka".into(),
            email: Some("tanaka@example.jp".into()),
            text: "Hello everyone!".into(),
            message_type: SlackMessageType::Message,
            edited: None,
            reactions: vec![],
            files: vec![],
            reply_count: 0,
            reply_users_count: 0,
        }
    }

    #[test]
    fn map_regular_message() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let msg = sample_message();
        let draft = adapter.map_message(&msg);

        assert_eq!(draft.schema.as_str(), SLACK_MESSAGE_SCHEMA);
        assert_eq!(
            draft.idempotency_key.as_str(),
            "slack:C01ABC:1234567890.123456"
        );
        assert_eq!(
            draft.subject.as_str(),
            "message:slack:C01ABC-1234567890.123456"
        );
        assert_eq!(draft.payload["text"], "Hello everyone!");
        assert_eq!(draft.payload["message_type"], "message");
        assert_eq!(draft.authority_model, AuthorityModel::LakeAuthoritative);
        assert_eq!(draft.capture_model, CaptureModel::Event);
    }

    #[test]
    fn map_edit_message() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let mut msg = sample_message();
        msg.message_type = SlackMessageType::Edit;
        msg.text = "Hello everyone! (edited)".into();
        msg.edited = Some(SlackEdited {
            user: "U01XYZ".into(),
            ts: "1234567891.000000".into(),
        });

        let draft = adapter.map_message(&msg);
        assert_eq!(
            draft.idempotency_key.as_str(),
            "slack:C01ABC:1234567890.123456:edit:1234567891.000000"
        );
        assert_eq!(draft.payload["message_type"], "edit");
        assert!(draft.payload["edited"].is_object());
    }

    #[test]
    fn map_delete_message() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let mut msg = sample_message();
        msg.message_type = SlackMessageType::Delete;

        let draft = adapter.map_message(&msg);
        assert_eq!(
            draft.idempotency_key.as_str(),
            "slack:C01ABC:1234567890.123456:delete"
        );
        assert_eq!(draft.payload["message_type"], "delete");
        assert!(draft.meta["retracts"].is_string());
    }

    #[test]
    fn map_thread_reply() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let mut msg = sample_message();
        msg.thread_ts = Some("1234567880.000000".into());

        let draft = adapter.map_message(&msg);
        assert_eq!(draft.payload["thread_ts"], "1234567880.000000");
    }

    #[test]
    fn map_file_share() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let mut msg = sample_message();
        msg.message_type = SlackMessageType::FileShare;
        msg.files = vec![SlackFile {
            id: "F01DEF".into(),
            name: "photo.jpg".into(),
            mimetype: "image/jpeg".into(),
            size: 12345,
            download_url: None,
            blob_ref: Some("blob:sha256:abcdef".into()),
        }];

        let draft = adapter.map_message(&msg);
        assert_eq!(draft.attachments.len(), 1);
        assert_eq!(draft.attachments[0].as_str(), "blob:sha256:abcdef");
        assert!(draft.payload["files"].is_array());
    }

    #[test]
    fn map_channel_snapshot() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let snap = SlackChannelSnapshot {
            channel_id: "C01ABC".into(),
            channel_name: "general".into(),
            purpose: Some("General discussion".into()),
            topic: Some("Welcome!".into()),
            member_count: 42,
            members: vec!["U01".into(), "U02".into()],
            is_archived: false,
            snapshot_at: Utc::now(),
        };

        let draft = adapter.map_channel_snapshot(&snap);
        assert_eq!(draft.schema.as_str(), SLACK_CHANNEL_SCHEMA);
        assert_eq!(draft.payload["channel_id"], "C01ABC");
        assert_eq!(draft.payload["member_count"], 42);
    }

    #[test]
    fn same_message_same_idempotency_key() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let msg = sample_message();
        let d1 = adapter.map_message(&msg);
        let d2 = adapter.map_message(&msg);
        assert_eq!(d1.idempotency_key, d2.idempotency_key);
    }

    #[test]
    fn fetch_incremental_via_fixture() {
        let page = SlackHistoryPage {
            messages: vec![sample_message()],
            has_more: false,
            next_cursor: None,
        };
        let client = FixtureSlackClient::new().with_history(vec![page]);
        let adapter = SlackAdapter::new(client, test_config());

        let result = adapter.fetch_incremental(None);
        match result {
            FetchResult::Ok {
                items, has_more, ..
            } => {
                assert_eq!(items.len(), 1);
                assert!(!has_more);
            }
            FetchResult::Error(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn to_observations_round_trip() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let msg = sample_message();
        let raw = RawData {
            data: serde_json::to_value(&msg).unwrap(),
            blobs: vec![],
        };
        let drafts = adapter.to_observations(&raw);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].payload["text"], "Hello everyone!");
    }

    #[test]
    fn heartbeat_generated() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let hb = adapter.heartbeat();
        assert_eq!(hb.schema.as_str(), "schema:observer-heartbeat");
        assert_eq!(hb.payload["status"], "alive");
    }

    #[test]
    fn cursor_management() {
        let mut adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        assert!(adapter.get_cursor("C01ABC").is_none());

        adapter.update_cursor("C01ABC", "1234567890.123456");
        assert_eq!(adapter.get_cursor("C01ABC"), Some("1234567890.123456"));

        adapter.update_cursor("C01ABC", "1234567891.000000");
        assert_eq!(adapter.get_cursor("C01ABC"), Some("1234567891.000000"));
    }

    #[test]
    fn adapter_metadata_in_observations() {
        let adapter = SlackAdapter::new(FixtureSlackClient::new(), test_config());
        let draft = adapter.map_message(&sample_message());
        assert_eq!(draft.meta["sourceAdapterVersion"], "1.0.0");
        assert_eq!(draft.schema_version.as_str(), SLACK_MESSAGE_SCHEMA_VERSION);
    }

    #[test]
    fn parse_slack_ts_works() {
        let dt = parse_slack_ts("1234567890.123456").unwrap();
        assert_eq!(dt.timestamp(), 1234567890);
    }
}
