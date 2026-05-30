use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactNode {
    pub id: String,
    pub proof_hash: String,
    pub payload_summary: String,
    pub backend: String,
    pub verified_at: DateTime<Utc>,
    pub epoch: u64,
    pub tags: Vec<String>,
}
/// An immutable verified fact committed to the permanent fact graph, surviving agent epoch rotations.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationAttempt {
    pub id: String,
    pub payload: String,
    pub payload_hash: String,
    pub backend: String,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub tactic_goals: Vec<String>,
    pub error_message: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub epoch: u64,
    pub tags: Vec<String>,
}
/// Tracks a single programmatic formal verification attempt (success or failure) in the persistent audit trail.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FactGraph {
    pub nodes: HashMap<String, FactNode>,
    #[serde(default)]
    pub attempts: Vec<VerificationAttempt>,
    #[serde(skip)]
    pub path: String,
}
/// Permanent hash-indexed fact store: accumulates verified proofs and logs all verification attempts across all agent lifecycles.


#[derive(Debug, Error)]
pub enum FactGraphError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
/// Typed errors for fact graph I/O and serialization failures.

impl FactGraph {
    pub fn new(path: &str) -> Self {
        Self {
            nodes: HashMap::new(),
            attempts: Vec::new(),
            path: path.to_string(),
        }
    }
}
/// Creates an empty fact graph bound to `path`.

pub fn ephemeral() -> FactGraph {
    FactGraph::new("")
}
/// Creates a non-persistent ephemeral fact graph for testing.

pub async fn load_fact_graph(path: &str) -> Result<FactGraph, FactGraphError> {
    if path.is_empty() || !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(FactGraph::new(path));
    }
    let raw = tokio::fs::read_to_string(path).await?;
    let mut graph: FactGraph = serde_json::from_str(&raw)?;
    graph.path = path.to_string();
    info!(path = path, nodes = graph.nodes.len(), "Fact graph loaded from disk");
    Ok(graph)
}
/// Loads a `FactGraph` from JSON at `path`, returning empty if the file does not yet exist.

pub async fn save_fact_graph(graph: &FactGraph) -> Result<(), FactGraphError> {
    if graph.path.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(graph)?;
    tokio::fs::write(&graph.path, json.as_bytes()).await?;
    Ok(())
}
/// Persists `FactGraph` to its configured JSON path; no-op for ephemeral instances.

pub async fn commit_fact(
    graph: &mut FactGraph,
    backend: &str,
    payload: &str,
    epoch: u64,
    tags: Vec<String>,
) -> Result<Option<String>, FactGraphError> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let proof_hash = hex::encode(hasher.finalize());

    if graph.nodes.contains_key(&proof_hash) {
        return Ok(None);
    }

    let node = FactNode {
        id: Uuid::new_v4().to_string(),
        proof_hash: proof_hash.clone(),
        payload_summary: payload.chars().take(256).collect(),
        backend: backend.to_string(),
        verified_at: Utc::now(),
        epoch,
        tags,
    };

    graph.nodes.insert(proof_hash.clone(), node);
    save_fact_graph(graph).await?;
    info!(hash = %proof_hash, backend = backend, "Fact committed to permanent graph");
    Ok(Some(proof_hash))
}
/// Hashes `payload`, deduplicates by hash, commits `FactNode` to graph, persists to disk. Returns the hash or `None` if duplicate.

pub async fn log_attempt(
    graph: &mut FactGraph,
    backend: &str,
    payload: &str,
    success: bool,
    stdout: &str,
    stderr: &str,
    tactic_goals: Vec<String>,
    error_message: Option<String>,
    epoch: u64,
    tags: Vec<String>,
) -> Result<String, FactGraphError> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let proof_hash = hex::encode(hasher.finalize());

    let attempt = VerificationAttempt {
        id: Uuid::new_v4().to_string(),
        payload: payload.to_string(),
        payload_hash: proof_hash.clone(),
        backend: backend.to_string(),
        success,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        tactic_goals,
        error_message,
        submitted_at: Utc::now(),
        epoch,
        tags,
    };

    graph.attempts.push(attempt);
    save_fact_graph(graph).await?;
    Ok(proof_hash)
}
/// Logs a verification attempt (success or failure) to the permanent registry and commits changes to disk.


pub fn query_by_tag<'a>(graph: &'a FactGraph, tag: &str) -> Vec<&'a FactNode> {
    graph
        .nodes
        .values()
        .filter(|n| n.tags.iter().any(|t| t == tag))
        .collect()
}
/// Returns all `FactNode`s whose tag list contains `tag`.

pub fn contains_hash(graph: &FactGraph, hash: &str) -> bool {
    graph.nodes.contains_key(hash)
}
// Returns true if `hash` is already committed to the fact graph.
