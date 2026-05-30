use crate::alignment::GroundVector;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RotationPolicy {
    Immediate,
    NextEpoch,
}
/// Determines whether a newly injected ground vector triggers an immediate pod rotation or waits for the next epoch.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: String,
    pub label: String,
    pub content: String,
    pub weight: f64,
    pub rotation_policy: RotationPolicy,
    pub neutralized: bool,
    pub created_at: DateTime<Utc>,
    pub neutralized_at: Option<DateTime<Utc>>,
}
/// A single user-injected steering directive stored in the Ground Vector Ledger.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Ledger {
    pub entries: HashMap<String, LedgerEntry>,
    #[serde(skip)]
    pub path: String,
}
/// Persistent Ground Vector Ledger: manages user-injected steering prompts with soft-deletion and rotation policies.

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Entry not found: {0}")]
    NotFound(String),
}
/// Typed error surface for ledger persistence and lookup failures.

impl LedgerEntry {
    pub fn to_ground_vector(&self) -> GroundVector {
        let mut hasher = Sha256::new();
        hasher.update(self.content.as_bytes());
        let digest = hasher.finalize();
        let embedding: Vec<f64> = digest
            .chunks(4)
            .map(|c| {
                let val = c.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32);
                (val as f64) / (u32::MAX as f64)
            })
            .collect();
        GroundVector {
            objective_id: self.id.clone(),
            embedding,
            label: self.label.clone(),
            weight: self.weight,
        }
    }
}
/// Converts a `LedgerEntry` to an alignment `GroundVector` via SHA-256 content embedding.

impl Ledger {
    pub fn new(path: &str) -> Self {
        Self {
            entries: HashMap::new(),
            path: path.to_string(),
        }
    }
}
/// Creates an empty ledger bound to `path` for persistent storage.

pub fn ephemeral() -> Ledger {
    Ledger::new("")
}
/// Creates a non-persistent ephemeral ledger for testing contexts.

pub async fn load_ledger(path: &str) -> Result<Ledger, LedgerError> {
    if path.is_empty() || !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(Ledger::new(path));
    }
    let raw = tokio::fs::read_to_string(path).await?;
    let mut ledger: Ledger = serde_json::from_str(&raw)?;
    ledger.path = path.to_string();
    info!(path = path, entries = ledger.entries.len(), "Ledger loaded from disk");
    Ok(ledger)
}
/// Loads a `Ledger` from JSON at `path`, returning an empty ledger if the file does not yet exist.

pub async fn save_ledger(ledger: &Ledger) -> Result<(), LedgerError> {
    if ledger.path.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(ledger)?;
    tokio::fs::write(&ledger.path, json.as_bytes()).await?;
    Ok(())
}
/// Persists `Ledger` to its configured JSON path; no-op for ephemeral ledgers.

pub fn add_entry(
    ledger: &mut Ledger,
    label: String,
    content: String,
    weight: f64,
    policy: RotationPolicy,
) -> String {
    let id = Uuid::new_v4().to_string();
    let entry = LedgerEntry {
        id: id.clone(),
        label,
        content,
        weight,
        rotation_policy: policy,
        neutralized: false,
        created_at: Utc::now(),
        neutralized_at: None,
    };
    ledger.entries.insert(id.clone(), entry);
    info!(id = %id, "Ground vector added to ledger");
    id
}
/// Inserts a new `LedgerEntry` with a generated UUID and returns the assigned ID.

pub fn neutralize_entry(ledger: &mut Ledger, id: &str) -> Result<(), LedgerError> {
    let entry = ledger
        .entries
        .get_mut(id)
        .ok_or_else(|| LedgerError::NotFound(id.to_string()))?;
    entry.neutralized = true;
    entry.neutralized_at = Some(Utc::now());
    warn!(id = %id, "Ground vector neutralized (soft-deleted)");
    Ok(())
}
/// Soft-deletes a ledger entry by marking it neutralized; it remains in the store but is excluded from alignment.

pub fn active_ground_vectors(ledger: &Ledger) -> Vec<GroundVector> {
    ledger
        .entries
        .values()
        .filter(|e| !e.neutralized)
        .map(|e| e.to_ground_vector())
        .collect()
}
// Returns `GroundVector` representations of all non-neutralized ledger entries for alignment scoring.
