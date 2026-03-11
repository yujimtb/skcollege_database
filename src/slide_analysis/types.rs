//! Slide analysis types — student profile extracted from a slide,
//! and the output records for supplemental store + SaaS write-back.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::domain::{BlobRef, EntityRef, ObservationId, SupplementalId};

/// Structured student profile extracted from a Google Slides self-introduction.
/// Schema mirrors the Gemini AI output from skcollege_dictionary/AIService.js.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudentProfile {
    /// Email found on the slide (or AI-generated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// AI-generated email (firstname.lastname@hlab.college).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_email: Option<String>,
    /// Display name (Kanji/Yomigana).
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub name: String,
    /// Bio text from the slide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio_text: Option<String>,
    /// Profile picture info.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_pic: Option<ProfilePic>,
    /// Gallery images from the slide.
    #[serde(default, deserialize_with = "deserialize_vec_or_default")]
    pub gallery_images: Vec<GalleryImage>,
    /// Structured properties extracted by AI.
    #[serde(default)]
    pub properties: StudentProperties,
    /// Tags / attributes.
    #[serde(default, deserialize_with = "deserialize_vec_or_default")]
    pub attributes: Vec<String>,
    /// Source slide object id inside the presentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_slide_object_id: Option<String>,
    /// Canonical DOKP document id for the analyzed slide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_document_id: Option<String>,
    /// Canonical Google Slides URL for the source deck.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_canonical_uri: Option<String>,
    /// Blob ref for the rendered thumbnail used for analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_blob_ref: Option<String>,
    /// External slide image URL from Google thumbnail rendering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// If present, this slide is a continuation of another primary slide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion_to_slide_object_id: Option<String>,
}

/// Profile picture information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilePic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<ImageCoordinates>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Image coordinates as percentage (0-100) from top-left.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageCoordinates {
    pub x: f64,
    pub y: f64,
}

/// Gallery image entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryImage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<ImageCoordinates>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Structured student properties from AI extraction.
/// Fields mirror the Gemini prompt schema in AIService.js.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StudentProperties {
    #[serde(skip_serializing_if = "Option::is_none", rename = "Nickname")]
    pub nickname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Birthplace")]
    pub birthplace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "DoB")]
    pub dob: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Major")]
    pub major: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Affiliation")]
    pub affiliation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "MBTI")]
    pub mbti: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "SNS")]
    pub sns: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_default", rename = "Hobbies")]
    pub hobbies: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_default", rename = "Interests")]
    pub interests: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_default", rename = "Likes")]
    pub likes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Dislikes")]
    pub dislikes: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_default", rename = "Hashtags")]
    pub hashtags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "New Challenges")]
    pub new_challenges: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Ask Me About")]
    pub ask_me_about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Turning Point")]
    pub turning_point: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "BTW")]
    pub btw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "Message")]
    pub message: Option<String>,
}

/// A slide analysis output: links an observation to the extracted profile
/// and the supplemental record that stores it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideAnalysisResult {
    /// The observation ID of the source slide snapshot.
    pub source_observation_id: ObservationId,
    /// Presentation ID.
    pub presentation_id: String,
    /// The extracted student profile.
    pub profile: StudentProfile,
    /// The person entity this profile was linked to.
    pub person_entity: EntityRef,
    /// Supplemental record ID if stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supplemental_id: Option<SupplementalId>,
    /// When the analysis was performed.
    pub analyzed_at: DateTime<Utc>,
    /// Model or strategy version used to generate the profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    /// The slide object id that produced this profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide_object_id: Option<String>,
    /// Rendered slide thumbnail blob used as the AI input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_blob_ref: Option<BlobRef>,
}

fn deserialize_string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_vec_or_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }

    Ok(match Option::<OneOrMany<T>>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(OneOrMany::One(value)) => vec![value],
        Some(OneOrMany::Many(values)) => values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn student_profile_round_trips() {
        let profile = StudentProfile {
            email: Some("test@hlab.college".into()),
            generated_email: None,
            name: "田中太郎".into(),
            bio_text: Some("Hello".into()),
            profile_pic: None,
            gallery_images: vec![],
            properties: StudentProperties {
                nickname: Some("Taro".into()),
                mbti: Some("ENFP".into()),
                ..Default::default()
            },
            attributes: vec!["CS".into()],
            source_slide_object_id: Some("slide-1".into()),
            source_document_id: Some("document:gslides:deck#slide:slide-1".into()),
            source_canonical_uri: Some("https://docs.google.com/presentation/d/deck".into()),
            thumbnail_blob_ref: Some("blob:sha256:test".into()),
            thumbnail_url: Some("https://lh3.googleusercontent.com/test".into()),
            companion_to_slide_object_id: None,
        };
        let json = serde_json::to_value(&profile).unwrap();
        let back: StudentProfile = serde_json::from_value(json).unwrap();
        assert_eq!(back.name, "田中太郎");
        assert_eq!(back.properties.nickname, Some("Taro".into()));
    }
}
