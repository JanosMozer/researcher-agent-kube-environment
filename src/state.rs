use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DagNode {
    pub id: String,
    pub task_type: String,
    pub dependencies: Vec<String>,
    pub status: DagNodeStatus,
    pub metadata: HashMap<String, serde_json::Value>,
}
/// A single vertex in the execution DAG representing an atomic research subtask.

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DagNodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
/// Lifecycle state of a `DagNode` within the execution graph.

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionDag {
    pub nodes: Vec<DagNode>,
    pub current_task_pointer: Option<String>,
    pub epoch: u64,
}
/// Full structural state: the DAG of pending/completed tasks and the active task cursor.

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SemanticContext {
    pub sanitized_summary: String,
    pub key_findings: Vec<String>,
    pub scratchpad_tokens: Vec<String>,
    pub source_urls: Vec<String>,
}
/// Sanitized semantic state: distilled LLM context safe for cross-pod migration.

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentState {
    pub agent_id: String,
    pub pod_name: String,
    pub namespace: String,
    #[schemars(with = "String")]
    pub captured_at: DateTime<Utc>,
    pub dag: ExecutionDag,
    pub semantic: SemanticContext,
    pub signature: String,
}
/// Complete serialized agent state payload transmitted from pod to controller on rotation.

impl AgentState {
    pub fn compute_signature(&self, secret: &str) -> String {
        use sha2::{Digest, Sha256};
        let payload = format!(
            "{}:{}:{}:{}",
            self.agent_id, self.pod_name, self.dag.epoch, secret
        );
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        hex::encode(hasher.finalize())
    }
}
// Produces a deterministic HMAC-like SHA-256 signature binding pod identity to epoch and shared secret.
