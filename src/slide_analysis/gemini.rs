use base64::Engine;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;

use crate::adapter::error::AdapterError;
use crate::domain::Observation;
use crate::lake::BlobStore;

use super::types::StudentProfile;

#[derive(Debug, Clone)]
pub struct GeminiSlideAnalyzer {
    http: Client,
    api_key: String,
    model: String,
}

impl GeminiSlideAnalyzer {
    const BASE_URL: &'static str = "https://generativelanguage.googleapis.com/v1beta/models";

    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self, AdapterError> {
        let http = Client::builder()
            .build()
            .map_err(|err| AdapterError::Network {
                message: err.to_string(),
            })?;

        Ok(Self {
            http,
            api_key: api_key.into(),
            model: model.into(),
        })
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn extract_profile(
        &self,
        observation: &Observation,
        blobs: &BlobStore,
    ) -> Result<Option<StudentProfile>, AdapterError> {
        let blob_ref = observation.attachments.first().ok_or_else(|| AdapterError::Other(
            "slide analysis requires a rendered slide thumbnail attachment".to_string(),
        ))?;
        let image = blobs.get(blob_ref).ok_or_else(|| AdapterError::Other(format!(
            "blob {} not available in blob store",
            blob_ref.as_str()
        )))?;
        let title = observation
            .payload
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown");
        let canonical_uri = observation
            .payload
            .pointer("/artifact/canonicalUri")
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        self.extract_profile_from_png(image, title, canonical_uri)
    }

    pub fn extract_profile_from_png(
        &self,
        image: &[u8],
        title: &str,
        canonical_uri: &str,
    ) -> Result<Option<StudentProfile>, AdapterError> {
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(image);

        let prompt = format!(
            "Analyze this student self-introduction slide and return ONLY a raw JSON object. Context: title={title}, canonical_uri={canonical_uri}. Extract this schema exactly: {{\n  \"email\": \"Email address found on slide (or null)\",\n  \"generated_email\": \"firstname.lastname@hlab.college (lowercase, romaji)\",\n  \"name\": \"Name (Kanji/Yomigana)\",\n  \"bio_text\": \"Full bio text\",\n  \"profile_pic\": {{\n    \"coordinates\": {{ \"x\": 50, \"y\": 50 }},\n    \"description\": \"Visual description of the person\",\n    \"url\": null\n  }},\n  \"gallery_images\": [{{\n    \"coordinates\": {{ \"x\": 80, \"y\": 80 }},\n    \"description\": \"Specific text caption associated with this photo found on the slide. If no text is near the image, return null. Do NOT generate visual descriptions.\",\n    \"url\": null\n  }}],\n  \"properties\": {{\n    \"Nickname\": \"text\",\n    \"Birthplace\": \"text (prefecture/country)\",\n    \"DoB\": \"YYYY-MM-DD (or null)\",\n    \"Major\": \"text\",\n    \"Affiliation\": \"text\",\n    \"MBTI\": \"text\",\n    \"SNS\": \"URL or null\",\n    \"Hobbies\": [\"array\", \"of\", \"strings\"],\n    \"Interests\": [\"array\", \"of\", \"strings\"],\n    \"Likes\": [\"array\", \"of\", \"strings\"],\n    \"Dislikes\": \"text\",\n    \"Hashtags\": [\"array\", \"of\", \"strings\"],\n    \"New Challenges\": \"text\",\n    \"Ask Me About\": \"text\",\n    \"Turning Point\": \"text\",\n    \"BTW\": \"text\",\n    \"Message\": \"text\"\n  }},\n  \"attributes\": [\"Array\", \"of\", \"tags\", \"or\", \"faculties\"]\n}}"
        );

        let request = serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": prompt },
                    {
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": image_base64
                        }
                    }
                ]
            }],
            "generationConfig": {
                "temperature": 0.2,
                "responseMimeType": "application/json"
            }
        });

        let url = format!(
            "{}/{model}:generateContent?key={key}",
            Self::BASE_URL,
            model = self.model,
            key = self.api_key,
        );
        let response = self
            .http
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .map_err(|err| AdapterError::Network {
                message: err.to_string(),
            })?;

        let status = response.status();
        let body = response.text().map_err(|err| AdapterError::Network {
            message: err.to_string(),
        })?;
        if !status.is_success() {
            return Err(AdapterError::Other(format!(
                "gemini api error ({status}): {body}"
            )));
        }

        let parsed: GeminiResponse = serde_json::from_str(&body).map_err(|err| AdapterError::MalformedResponse {
            message: format!("gemini decode error: {err}; body: {body}"),
        })?;
        let text = parsed
            .candidates
            .into_iter()
            .flat_map(|candidate| candidate.content.parts.into_iter())
            .find_map(|part| part.text)
            .ok_or_else(|| AdapterError::MalformedResponse {
                message: format!("gemini returned no text parts; body: {body}"),
            })?;
        let profile = serde_json::from_str::<StudentProfile>(&text).map_err(|err| AdapterError::MalformedResponse {
            message: format!("gemini profile decode error: {err}; text: {text}"),
        })?;
        Ok(Some(profile))
    }
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}