use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SelfHostConfig {
    pub bind_addr: String,
    pub database_path: PathBuf,
    pub blob_dir: PathBuf,
    pub poll_interval: Duration,
    pub slack: SlackConfig,
    pub google: GoogleConfig,
    pub slide_analysis_limit: usize,
    pub slide_ai: Option<SlideAiConfig>,
    pub notion: Option<NotionWritebackConfig>,
}

#[derive(Debug, Clone)]
pub struct SlackConfig {
    pub bot_token: String,
    pub channel_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub access_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub refresh_token: Option<String>,
    pub presentation_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SlideAiConfig {
    pub api_key: String,
    pub model: String,
}

/// Configuration for Notion write-back adapter.
#[derive(Debug, Clone)]
pub struct NotionWritebackConfig {
    /// Notion integration token.
    pub token: String,
    /// Target Notion database ID.
    pub database_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing environment variable {0}")]
    MissingEnv(&'static str),
    #[error("invalid environment variable {name}: {message}")]
    InvalidEnv { name: &'static str, message: String },
    #[error("google credentials require either DOKP_GOOGLE_ACCESS_TOKEN or the trio DOKP_GOOGLE_CLIENT_ID, DOKP_GOOGLE_CLIENT_SECRET, DOKP_GOOGLE_REFRESH_TOKEN")]
    MissingGoogleCredentials,
}

impl SelfHostConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();

        let bind_addr = env::var("DOKP_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let database_path = PathBuf::from(env::var("DOKP_DATABASE_PATH").unwrap_or_else(|_| "./data/dokp.sqlite3".to_string()));
        let blob_dir = PathBuf::from(env::var("DOKP_BLOB_DIR").unwrap_or_else(|_| "./data/blobs".to_string()));
        let poll_interval = Duration::from_secs(parse_u64_env("DOKP_POLL_SECONDS", 300)?);

        let slack = SlackConfig {
            bot_token: required_env("DOKP_SLACK_BOT_TOKEN")?,
            channel_ids: parse_csv_env("DOKP_SLACK_CHANNEL_IDS", true)?,
        };

        let google = GoogleConfig {
            access_token: env::var("DOKP_GOOGLE_ACCESS_TOKEN").ok().filter(|v| !v.trim().is_empty()),
            client_id: env::var("DOKP_GOOGLE_CLIENT_ID").ok().filter(|v| !v.trim().is_empty()),
            client_secret: env::var("DOKP_GOOGLE_CLIENT_SECRET").ok().filter(|v| !v.trim().is_empty()),
            refresh_token: env::var("DOKP_GOOGLE_REFRESH_TOKEN").ok().filter(|v| !v.trim().is_empty()),
            presentation_ids: parse_csv_env("DOKP_GOOGLE_PRESENTATION_IDS", true)?,
        };
        let slide_analysis_limit = parse_usize_env("DOKP_GOOGLE_SLIDE_ANALYSIS_LIMIT", 10)?;

        if google.access_token.is_none()
            && (google.client_id.is_none()
                || google.client_secret.is_none()
                || google.refresh_token.is_none())
        {
            return Err(ConfigError::MissingGoogleCredentials);
        }

        let notion = match (
            env::var("DOKP_NOTION_TOKEN").ok().filter(|v| !v.trim().is_empty()),
            env::var("DOKP_NOTION_DATABASE_ID").ok().filter(|v| !v.trim().is_empty()),
        ) {
            (Some(token), Some(database_id)) => Some(NotionWritebackConfig { token, database_id }),
            _ => None,
        };

        let slide_ai = env::var("DOKP_GEMINI_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|api_key| SlideAiConfig {
                api_key,
                model: env::var("DOKP_GEMINI_MODEL")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or_else(|| "gemini-2.5-flash".to_string()),
            });

        Ok(Self {
            bind_addr,
            database_path,
            blob_dir,
            poll_interval,
            slack,
            google,
            slide_analysis_limit,
            slide_ai,
            notion,
        })
    }
}

fn required_env(name: &'static str) -> Result<String, ConfigError> {
    env::var(name).map_err(|_| ConfigError::MissingEnv(name))
}

fn parse_u64_env(name: &'static str, default: u64) -> Result<u64, ConfigError> {
    match env::var(name) {
        Ok(raw) => raw.parse::<u64>().map_err(|err| ConfigError::InvalidEnv {
            name,
            message: err.to_string(),
        }),
        Err(_) => Ok(default),
    }
}

fn parse_csv_env(name: &'static str, required: bool) -> Result<Vec<String>, ConfigError> {
    match env::var(name) {
        Ok(raw) => {
            let values: Vec<String> = raw
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            if required && values.is_empty() {
                return Err(ConfigError::InvalidEnv {
                    name,
                    message: "must contain at least one comma-separated value".to_string(),
                });
            }
            Ok(values)
        }
        Err(_) if required => Err(ConfigError::MissingEnv(name)),
        Err(_) => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_splits_values() {
        unsafe {
            env::set_var("DOKP_SLACK_CHANNEL_IDS", "C1, C2 ,,C3");
        }
        let values = parse_csv_env("DOKP_SLACK_CHANNEL_IDS", true).unwrap();
        assert_eq!(values, vec!["C1", "C2", "C3"]);
    }
}

fn parse_usize_env(name: &'static str, default: usize) -> Result<usize, ConfigError> {
    match env::var(name) {
        Ok(raw) => raw.parse::<usize>().map_err(|err| ConfigError::InvalidEnv {
            name,
            message: err.to_string(),
        }),
        Err(_) => Ok(default),
    }
}