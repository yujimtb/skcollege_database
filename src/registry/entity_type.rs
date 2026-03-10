//! M02 Registry — EntityType definitions

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::EntityTypeRef;

/// A registered observation-target type (e.g. `et:person`, `et:room`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityType {
    /// `et:{name}` format.
    pub id: EntityTypeRef,
    pub name: String,
    pub description: String,
    /// Optional parent for is-a hierarchy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<EntityTypeRef>,
    /// Recommended attribute names on observations of this type.
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<DateTime<Utc>>,
}

/// Foundation entity types that are always present.
pub fn base_entity_types() -> Vec<EntityType> {
    let now = Utc::now();
    let base = |id: &str, name: &str, desc: &str| EntityType {
        id: EntityTypeRef::new(id),
        name: name.into(),
        description: desc.into(),
        parent: None,
        attributes: vec![],
        registered_by: Some("system".into()),
        registered_at: Some(now),
    };
    vec![
        base("et:person", "Person", "寮に関わる人物"),
        base("et:space", "Space", "物理空間"),
        base("et:artifact", "Artifact", "物理的・デジタルな対象物"),
        base("et:document", "Document", "デジタル文書"),
        base("et:message", "Message", "メッセージ"),
        base("et:observer", "Observer", "Observer 自身"),
    ]
}
