//! M13: Person Page Projector — builds person page data from identity resolution + lake.
//!
//! Input: M12 IdentityResolutionOutput + Lake observations.
//! Output: PersonPageOutput (profiles, slides, messages, activities).

use std::collections::HashMap;

use chrono::Utc;

use crate::domain::{EntityRef, Observation, SchemaRef};
use crate::identity::types::{IdentifierType, IdentityResolutionOutput, ResolvedPerson};

use super::types::*;

/// Person page projector — pure functional core.
pub struct PersonPageProjector;

impl PersonPageProjector {
    /// Build person page output from identity resolution and observations.
    ///
    /// Only uses `resolved_persons` with confirmed status (not pending candidates).
    pub fn project(
        identity: &IdentityResolutionOutput,
        observations: &[Observation],
    ) -> PersonPageOutput {
        let mut profiles = Vec::new();
        let mut all_slides = Vec::new();
        let mut all_messages = Vec::new();
        let mut all_activities = Vec::new();

        // Build person-id → identifiers map for matching.
        let person_identifiers = Self::build_person_identifier_map(identity);

        for person in &identity.resolved_persons {
            let (slides, messages) =
                Self::collect_related(person, observations, &person_identifiers);

            let activity = Self::build_activity(&person.person_id, &slides, &messages);

            let profile = PersonProfile {
                person_id: person.person_id.clone(),
                display_name: person.canonical_name.clone(),
                self_intro_text: None, // Populated from supplemental in real pipeline.
                self_intro_slide_id: None,
                self_intro_thumbnail: None,
                identities: person
                    .identifiers
                    .iter()
                    .filter_map(|ident| {
                        match ident.identifier_type {
                            IdentifierType::Email | IdentifierType::UserId => {
                                Some(IdentityInfo {
                                    system: ident.source.clone(),
                                    external_id: ident.value.clone(),
                                })
                            }
                            _ => None,
                        }
                    })
                    .collect(),
                source_count: person.sources.len(),
                last_activity: activity.last_activity,
                profile_updated_at: Utc::now(),
            };

            profiles.push(profile);
            all_slides.extend(slides);
            all_messages.extend(messages);
            all_activities.push(activity);
        }

        PersonPageOutput {
            profiles,
            slides: all_slides,
            messages: all_messages,
            activities: all_activities,
        }
    }

    /// Build a map from source identifier values to person_id.
    fn build_person_identifier_map(
        identity: &IdentityResolutionOutput,
    ) -> HashMap<String, EntityRef> {
        let mut map = HashMap::new();
        for person in &identity.resolved_persons {
            for ident in &person.identifiers {
                map.insert(ident.value.clone(), person.person_id.clone());
            }
        }
        map
    }

    /// Collect slides and messages related to a person.
    fn collect_related(
        person: &ResolvedPerson,
        observations: &[Observation],
        identifier_map: &HashMap<String, EntityRef>,
    ) -> (Vec<PersonSlide>, Vec<PersonMessage>) {
        let mut slides = Vec::new();
        let mut messages = Vec::new();
        let mut slide_counter = 0u64;
        let mut msg_counter = 0u64;

        for obs in observations {
            let belongs = Self::observation_belongs_to(obs, person, identifier_map);
            if !belongs {
                continue;
            }

            if obs.schema == SchemaRef::new("schema:workspace-object-snapshot") {
                slide_counter += 1;
                let title = obs
                    .payload
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled")
                    .to_string();
                let revision = obs
                    .payload
                    .pointer("/revision/sourceRevisionId")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                slides.push(PersonSlide {
                    id: format!("ps:{}:{slide_counter}", person.person_id),
                    person_id: person.person_id.clone(),
                    document_id: obs.subject.as_str().to_string(),
                    title,
                    role: "editor".into(),
                    last_seen_revision: revision,
                    slide_count: None,
                    thumbnail_ref: None,
                    last_modified: Some(obs.published),
                });
            } else if obs.schema == SchemaRef::new("schema:slack-message") {
                msg_counter += 1;
                let text = obs
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let channel = obs
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let thread_ts = obs
                    .payload
                    .get("thread_ts")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                messages.push(PersonMessage {
                    id: format!("pm:{}:{msg_counter}", person.person_id),
                    person_id: person.person_id.clone(),
                    channel,
                    text,
                    ts: obs.published,
                    thread_ts,
                    has_attachments: !obs.attachments.is_empty(),
                });
            }
        }

        (slides, messages)
    }

    /// Check if an observation belongs to a person.
    fn observation_belongs_to(
        obs: &Observation,
        person: &ResolvedPerson,
        identifier_map: &HashMap<String, EntityRef>,
    ) -> bool {
        // Check via user_id in payload.
        if let Some(user_id) = obs.payload.get("user_id").and_then(|v| v.as_str()) {
            if let Some(pid) = identifier_map.get(user_id) {
                if *pid == person.person_id {
                    return true;
                }
            }
        }

        // Check via email in payload.
        if let Some(email) = obs.payload.get("email").and_then(|v| v.as_str()) {
            if let Some(pid) = identifier_map.get(email) {
                if *pid == person.person_id {
                    return true;
                }
            }
        }

        // Check via editors in GSlides.
        if let Some(editors) = obs
            .payload
            .pointer("/relations/editors")
            .and_then(|v| v.as_array())
        {
            for editor in editors {
                if let Some(email) = editor.as_str() {
                    if let Some(pid) = identifier_map.get(email) {
                        if *pid == person.person_id {
                            return true;
                        }
                    }
                }
            }
        }

        // Check via owner in GSlides.
        if let Some(owner) = obs.payload.pointer("/relations/owner").and_then(|v| v.as_str()) {
            if let Some(pid) = identifier_map.get(owner) {
                if *pid == person.person_id {
                    return true;
                }
            }
        }

        false
    }

    /// Build activity summary for a person.
    fn build_activity(
        person_id: &EntityRef,
        slides: &[PersonSlide],
        messages: &[PersonMessage],
    ) -> PersonActivity {
        let mut channels: Vec<String> = messages
            .iter()
            .map(|m| m.channel.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        channels.sort();

        let first_slide = slides.iter().filter_map(|s| s.last_modified).min();
        let first_msg = messages.iter().map(|m| m.ts).min();
        let first_activity = [first_slide, first_msg].into_iter().flatten().min();

        let last_slide = slides.iter().filter_map(|s| s.last_modified).max();
        let last_msg = messages.iter().map(|m| m.ts).max();
        let last_activity = [last_slide, last_msg].into_iter().flatten().max();

        PersonActivity {
            person_id: person_id.clone(),
            total_slides_related: slides.len(),
            total_messages: messages.len(),
            first_activity,
            last_activity,
            active_channels: channels,
        }
    }

    /// Build a PersonListItem from a profile and activity.
    pub fn to_list_item(profile: &PersonProfile, activity: &PersonActivity) -> PersonListItem {
        PersonListItem {
            person_id: profile.person_id.clone(),
            display_name: profile.display_name.clone(),
            source_count: profile.source_count,
            total_slides: activity.total_slides_related,
            total_messages: activity.total_messages,
            last_activity: activity.last_activity,
            thumbnail_url: profile.self_intro_thumbnail.clone(),
        }
    }

    /// Build a PersonDetailResponse from projector output.
    pub fn to_detail(
        profile: &PersonProfile,
        slides: &[PersonSlide],
        messages: &[PersonMessage],
        activity: &PersonActivity,
    ) -> PersonDetailResponse {
        let self_introduction = profile.self_intro_text.as_ref().map(|text| SelfIntroduction {
            text: text.clone(),
            slide_id: profile.self_intro_slide_id.clone(),
            thumbnail_url: profile.self_intro_thumbnail.clone(),
        });

        PersonDetailResponse {
            person_id: profile.person_id.clone(),
            display_name: profile.display_name.clone(),
            self_introduction,
            identities: profile.identities.clone(),
            related_slides: slides.to_vec(),
            recent_messages: messages.to_vec(),
            activity_summary: activity.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::governance::types::ConfidenceLevel;
    use crate::identity::types::*;

    fn slack_obs(user_id: &str, email: &str, text: &str, channel: &str, key: &str) -> Observation {
        Observation {
            id: Observation::new_id(),
            schema: SchemaRef::new("schema:slack-message"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:slack-crawler"),
            source_system: Some(SourceSystemRef::new("sys:slack")),
            actor: None,
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject: EntityRef::new(format!("message:slack:{key}")),
            target: None,
            payload: serde_json::json!({
                "user_id": user_id,
                "email": email,
                "text": text,
                "channel": channel,
            }),
            attachments: vec![],
            published: Utc::now(),
            recorded_at: Utc::now(),
            consent: None,
            idempotency_key: Some(IdempotencyKey::new(key)),
            meta: serde_json::json!({}),
        }
    }

    fn gslides_obs(editors: &[&str], owner: &str, key: &str) -> Observation {
        Observation {
            id: Observation::new_id(),
            schema: SchemaRef::new("schema:workspace-object-snapshot"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:gslides-crawler"),
            source_system: Some(SourceSystemRef::new("sys:google-slides")),
            actor: None,
            authority_model: AuthorityModel::SourceAuthoritative,
            capture_model: CaptureModel::Snapshot,
            subject: EntityRef::new(format!("document:gslide:{key}")),
            target: None,
            payload: serde_json::json!({
                "title": "自己紹介スライド",
                "relations": {
                    "editors": editors,
                    "owner": owner,
                },
            }),
            attachments: vec![],
            published: Utc::now(),
            recorded_at: Utc::now(),
            consent: None,
            idempotency_key: Some(IdempotencyKey::new(key)),
            meta: serde_json::json!({}),
        }
    }

    fn sample_identity() -> IdentityResolutionOutput {
        IdentityResolutionOutput {
            resolved_persons: vec![ResolvedPerson {
                person_id: EntityRef::new("person:tanaka-2026"),
                canonical_name: "田中太郎".into(),
                aliases: vec!["田中太郎".into()],
                identifiers: vec![
                    SourceIdentifier {
                        source: "slack".into(),
                        identifier_type: IdentifierType::UserId,
                        value: "U123".into(),
                    },
                    SourceIdentifier {
                        source: "slack".into(),
                        identifier_type: IdentifierType::Email,
                        value: "tanaka@example.jp".into(),
                    },
                    SourceIdentifier {
                        source: "google".into(),
                        identifier_type: IdentifierType::Email,
                        value: "tanaka@example.jp".into(),
                    },
                ],
                confidence: ConfidenceLevel::High,
                sources: vec!["slack".into(), "google".into()],
                resolved_at: Utc::now(),
                resolved_by: "projector:identity-resolution:v1.0.0".into(),
            }],
            candidates: vec![],
            person_identifiers: vec![],
        }
    }

    #[test]
    fn person_page_with_slides_and_messages() {
        let identity = sample_identity();
        let observations = vec![
            slack_obs("U123", "tanaka@example.jp", "こんにちは", "general", "s1"),
            slack_obs("U123", "tanaka@example.jp", "明日の会議", "project-a", "s2"),
            gslides_obs(&["tanaka@example.jp"], "tanaka@example.jp", "g1"),
        ];

        let output = PersonPageProjector::project(&identity, &observations);
        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].display_name, "田中太郎");
        assert_eq!(output.messages.len(), 2);
        assert_eq!(output.slides.len(), 1);
        assert_eq!(output.activities.len(), 1);
        assert_eq!(output.activities[0].total_messages, 2);
        assert_eq!(output.activities[0].total_slides_related, 1);
        assert_eq!(output.activities[0].active_channels.len(), 2);
    }

    #[test]
    fn unrelated_observation_excluded() {
        let identity = sample_identity();
        let observations = vec![
            slack_obs("U999", "other@example.com", "unrelated", "general", "s1"),
        ];

        let output = PersonPageProjector::project(&identity, &observations);
        assert_eq!(output.messages.len(), 0);
    }

    #[test]
    fn person_detail_response_builds() {
        let identity = sample_identity();
        let observations = vec![
            slack_obs("U123", "tanaka@example.jp", "hello", "general", "s1"),
        ];

        let output = PersonPageProjector::project(&identity, &observations);
        let profile = &output.profiles[0];
        let activity = &output.activities[0];
        let msgs: Vec<_> = output.messages.iter().filter(|m| m.person_id == profile.person_id).collect();

        let detail = PersonPageProjector::to_detail(
            profile,
            &[],
            &msgs.into_iter().cloned().collect::<Vec<_>>(),
            activity,
        );
        assert_eq!(detail.person_id.as_str(), "person:tanaka-2026");
        assert_eq!(detail.display_name, "田中太郎");
        assert_eq!(detail.recent_messages.len(), 1);
    }

    #[test]
    fn person_list_item_builds() {
        let identity = sample_identity();
        let observations = vec![
            slack_obs("U123", "tanaka@example.jp", "msg", "general", "s1"),
            gslides_obs(&["tanaka@example.jp"], "tanaka@example.jp", "g1"),
        ];

        let output = PersonPageProjector::project(&identity, &observations);
        let item = PersonPageProjector::to_list_item(&output.profiles[0], &output.activities[0]);
        assert_eq!(item.display_name, "田中太郎");
        assert_eq!(item.total_messages, 1);
        assert_eq!(item.total_slides, 1);
        assert_eq!(item.source_count, 2);
    }

    #[test]
    fn replay_deterministic() {
        let identity = sample_identity();
        let observations = vec![
            slack_obs("U123", "tanaka@example.jp", "hello", "general", "s1"),
            gslides_obs(&["tanaka@example.jp"], "tanaka@example.jp", "g1"),
        ];

        let r1 = PersonPageProjector::project(&identity, &observations);
        let r2 = PersonPageProjector::project(&identity, &observations);
        assert_eq!(r1.profiles.len(), r2.profiles.len());
        assert_eq!(r1.messages.len(), r2.messages.len());
        assert_eq!(r1.slides.len(), r2.slides.len());
    }
}
