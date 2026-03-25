use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::Digest;

use crate::domain::{BlobRef, Observation, SupplementalRecord};

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct SqlitePersistence {
    conn: Connection,
    blob_dir: PathBuf,
}

impl SqlitePersistence {
    pub fn open(database_path: &Path, blob_dir: &Path) -> Result<Self, PersistenceError> {
        if let Some(parent) = database_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(blob_dir)?;

        let conn = Connection::open(database_path)?;
        let store = Self {
            conn,
            blob_dir: blob_dir.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn load_observations(&self) -> Result<Vec<Observation>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT observation_json FROM observations ORDER BY recorded_at, id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut observations = Vec::new();
        for row in rows {
            let json = row?;
            observations.push(serde_json::from_str::<Observation>(&json)?);
        }
        Ok(observations)
    }

    pub fn load_supplementals(&self) -> Result<Vec<SupplementalRecord>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT supplemental_json FROM supplementals ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut supplementals = Vec::new();
        for row in rows {
            let json = row?;
            supplementals.push(serde_json::from_str::<SupplementalRecord>(&json)?);
        }
        Ok(supplementals)
    }

    pub fn persist_observation(&self, observation: &Observation) -> Result<(), PersistenceError> {
        let json = serde_json::to_string(observation)?;
        self.conn.execute(
            "INSERT INTO observations (id, idempotency_key, recorded_at, observation_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                observation.id.as_str(),
                observation.idempotency_key.as_ref().map(|value| value.as_str()),
                observation.recorded_at.to_rfc3339(),
                json,
            ],
        )?;
        Ok(())
    }

    pub fn persist_supplemental(&self, record: &SupplementalRecord) -> Result<(), PersistenceError> {
        let json = serde_json::to_string(record)?;
        self.conn.execute(
            "INSERT INTO supplementals (id, created_at, supplemental_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET created_at = excluded.created_at, supplemental_json = excluded.supplemental_json",
            params![
                record.id.as_str(),
                record.created_at.to_rfc3339(),
                json,
            ],
        )?;
        Ok(())
    }

    pub fn get_state(&self, key: &str) -> Result<Option<String>, PersistenceError> {
        self.conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<(), PersistenceError> {
        self.conn.execute(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn persist_blob(&self, data: &[u8]) -> Result<BlobRef, PersistenceError> {
        let hash = hex::encode(sha2::Sha256::digest(data));
        let blob_ref = BlobRef::new(format!("blob:sha256:{hash}"));
        let path = self.blob_dir.join(&hash);
        if !path.exists() {
            fs::write(&path, data)?;
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO blobs (blob_ref, file_path) VALUES (?1, ?2)",
            params![blob_ref.as_str(), path.to_string_lossy().to_string()],
        )?;
        Ok(blob_ref)
    }

    pub fn load_blobs(&self) -> Result<Vec<Vec<u8>>, PersistenceError> {
        let mut stmt = self.conn.prepare("SELECT file_path FROM blobs ORDER BY file_path")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut blobs = Vec::new();
        for row in rows {
            blobs.push(fs::read(row?)?);
        }
        Ok(blobs)
    }

    fn init_schema(&self) -> Result<(), PersistenceError> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS observations (
                id TEXT PRIMARY KEY,
                idempotency_key TEXT UNIQUE,
                recorded_at TEXT NOT NULL,
                observation_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sync_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS blobs (
                blob_ref TEXT PRIMARY KEY,
                file_path TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS supplementals (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                supplemental_json TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    use crate::domain::{
        supplemental::InputAnchorSet, ActorRef, AuthorityModel, CaptureModel, EntityRef,
        IdempotencyKey, Mutability, Observation, ObserverRef, SchemaRef, SemVer, SupplementalId,
        SupplementalRecord,
    };

    fn sample_observation() -> Observation {
        Observation {
            id: Observation::new_id(),
            schema: SchemaRef::new("schema:test"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:test"),
            source_system: None,
            actor: None,
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject: EntityRef::new("entity:test"),
            target: None,
            payload: serde_json::json!({"hello": "world"}),
            attachments: vec![],
            published: Utc::now(),
            recorded_at: Utc::now(),
            consent: None,
            idempotency_key: Some(IdempotencyKey::new("sample-key")),
            meta: serde_json::json!({}),
        }
    }

    #[test]
    fn persist_and_reload_observation() {
        let tmp = std::env::temp_dir().join(format!("dokp-test-{}", uuid::Uuid::now_v7()));
        let db = tmp.join("test.sqlite3");
        let blob_dir = tmp.join("blobs");
        let store = SqlitePersistence::open(&db, &blob_dir).unwrap();
        let observation = sample_observation();

        store.persist_observation(&observation).unwrap();
        let observations = store.load_observations().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].schema, observation.schema);

        let _ = fs::remove_dir_all(tmp);
    }

    fn sample_supplemental(observation_id: &crate::domain::ObservationId) -> SupplementalRecord {
        SupplementalRecord {
            id: SupplementalId::new("sup:test"),
            kind: "slide-analysis".into(),
            derived_from: InputAnchorSet {
                observations: vec![observation_id.clone()],
                blobs: vec![],
                supplementals: vec![],
            },
            payload: serde_json::json!({"bio_text": "hello"}),
            created_by: ActorRef::new("actor:test"),
            created_at: Utc::now(),
            mutability: Mutability::ManagedCache,
            record_version: Some("1".into()),
            model_version: Some("fixture".into()),
            consent_metadata: None,
            lineage: None,
        }
    }

    #[test]
    fn duplicate_persist_observation_surfaces_constraint_error() {
        let tmp = std::env::temp_dir().join(format!("dokp-test-{}", uuid::Uuid::now_v7()));
        let db = tmp.join("test.sqlite3");
        let blob_dir = tmp.join("blobs");
        let store = SqlitePersistence::open(&db, &blob_dir).unwrap();
        let observation = sample_observation();

        store.persist_observation(&observation).unwrap();
        let err = store.persist_observation(&observation).unwrap_err();
        assert!(matches!(err, PersistenceError::Sqlite(_)));

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn persist_and_reload_supplemental() {
        let tmp = std::env::temp_dir().join(format!("dokp-test-{}", uuid::Uuid::now_v7()));
        let db = tmp.join("test.sqlite3");
        let blob_dir = tmp.join("blobs");
        let store = SqlitePersistence::open(&db, &blob_dir).unwrap();
        let observation = sample_observation();
        let supplemental = sample_supplemental(&observation.id);

        store.persist_observation(&observation).unwrap();
        store.persist_supplemental(&supplemental).unwrap();
        let supplementals = store.load_supplementals().unwrap();

        assert_eq!(supplementals.len(), 1);
        assert_eq!(supplementals[0].id, supplemental.id);
        assert_eq!(supplementals[0].kind, "slide-analysis");

        let _ = fs::remove_dir_all(tmp);
    }
}
