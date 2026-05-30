use crate::fact_graph::{commit_fact, FactGraph, FactNode};
use crate::ledger::{
    add_entry, neutralize_entry, save_ledger, Ledger, RotationPolicy,
};
use crate::state::AgentState;
use crate::verify::{verify, VerificationEngine, VerifierBackend};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Notify, RwLock};
use tracing::{info, warn};

pub struct AppState {
    pub pending_state: Arc<RwLock<Option<AgentState>>>,
    pub llm_api_key: String,
    pub verification_engine: Arc<VerificationEngine>,
    pub fact_graph: Arc<RwLock<FactGraph>>,
    pub ledger: Arc<RwLock<Ledger>>,
    pub rotation_trigger: Arc<Notify>,
}
/// Unified Axum shared state: pending pod state, API key, verification engine, fact graph, ledger, and rotation signal.

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Payload rejected: {0}")]
    Rejected(String),
    #[error("Internal error: {0}")]
    Internal(String),
}
/// Typed HTTP error variants for all controller API endpoints.

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            ApiError::Rejected(_) => StatusCode::BAD_REQUEST,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}
/// Maps `ApiError` variants to HTTP 400 or 500 responses.

pub async fn ingest_state(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentState>,
) -> Result<impl IntoResponse, ApiError> {
    let expected_sig = payload.compute_signature(&state.llm_api_key);
    if payload.signature != expected_sig {
        warn!(pod = %payload.pod_name, "State signature mismatch — rejecting payload");
        return Err(ApiError::Rejected(format!(
            "Invalid signature for pod {}",
            payload.pod_name
        )));
    }
    info!(pod = %payload.pod_name, epoch = payload.dag.epoch, "State ingested and verified");
    *state.pending_state.write().await = Some(payload);
    Ok(StatusCode::ACCEPTED)
}
/// POST /state/ingest — receives, verifies SHA-256 signature, and stores serialized agent state from a terminating pod.

#[derive(Debug, Deserialize)]
pub struct VerifyApiRequest {
    pub backend: String,
    pub payload: String,
    pub sandbox_language: Option<String>,
    pub sandbox_expected_output: Option<String>,
    pub sandbox_stdin: Option<String>,
    #[serde(default)]
    pub fact_tags: Vec<String>,
    pub epoch: Option<u64>,
}
/// Request body for the unified `/api/v1/verify` RPC endpoint used by in-cluster agent pods.

#[derive(Debug, Serialize)]
pub struct VerifyApiResponse {
    pub success: bool,
    pub backend: String,
    pub stdout: String,
    pub stderr: String,
    pub tactic_goals: Vec<String>,
    pub error_message: Option<String>,
    pub fact_hash: Option<String>,
}
/// Response from `/api/v1/verify`: full subprocess output for agent self-correction loops, plus committed fact hash.

pub async fn api_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyApiRequest>,
) -> Result<Json<VerifyApiResponse>, ApiError> {
    let backend = match req.backend.as_str() {
        "lean4" => VerifierBackend::Lean4,
        "z3" => VerifierBackend::Z3,
        "sandbox" => VerifierBackend::Sandbox {
            language: req
                .sandbox_language
                .clone()
                .unwrap_or_else(|| "python3".to_string()),
            expected_output: req.sandbox_expected_output.clone().unwrap_or_default(),
            stdin_input: req.sandbox_stdin.clone(),
        },
        other => {
            return Err(ApiError::Rejected(format!(
                "Unknown backend: {other}. Valid: lean4, z3, sandbox"
            )))
        }
    };

    let result = verify(&state.verification_engine, backend, &req.payload)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    {
        let mut fg = state.fact_graph.write().await;
        if let Err(e) = crate::fact_graph::log_attempt(
            &mut fg,
            &result.backend,
            &req.payload,
            result.success,
            &result.stdout,
            &result.stderr,
            result.tactic_goals.clone(),
            result.error_message.clone(),
            req.epoch.unwrap_or(0),
            req.fact_tags.clone(),
        )
        .await
        {
            warn!(error = %e, "Failed to log verification attempt");
        }
    }

    let fact_hash = if result.success {
        let mut hasher = Sha256::new();
        hasher.update(req.payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let mut fg = state.fact_graph.write().await;
        match commit_fact(
            &mut fg,
            &result.backend,
            &req.payload,
            req.epoch.unwrap_or(0),
            req.fact_tags.clone(),
        )
        .await
        {
            Ok(Some(h)) => Some(h),
            Ok(None) => Some(hash),
            Err(e) => {
                warn!(error = %e, "Fact graph commit failed");
                None
            }
        }
    } else {
        None
    };

    Ok(Json(VerifyApiResponse {
        success: result.success,
        backend: result.backend,
        stdout: result.stdout,
        stderr: result.stderr,
        tactic_goals: result.tactic_goals,
        error_message: result.error_message,
        fact_hash,
    }))
}
/// POST /api/v1/verify — multi-backend verification RPC: dispatches to Lean4/Z3/Sandbox, logs attempts, commits successes to fact graph.

#[derive(Debug, Deserialize)]
pub struct HistoryQueryParams {
    pub backend: Option<String>,
    pub status: Option<String>,
    pub epoch: Option<u64>,
}
/// Query parameters for listing verification history.

pub async fn list_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HistoryQueryParams>,
) -> Json<Vec<crate::fact_graph::VerificationAttempt>> {
    let fg = state.fact_graph.read().await;
    let mut attempts = fg.attempts.clone();

    if let Some(ref backend) = params.backend {
        attempts.retain(|a| &a.backend == backend);
    }
    if let Some(ref status) = params.status {
        match status.as_str() {
            "passed" | "success" | "true" => attempts.retain(|a| a.success),
            "failed" | "failure" | "false" => attempts.retain(|a| !a.success),
            _ => {}
        }
    }
    if let Some(epoch) = params.epoch {
        attempts.retain(|a| a.epoch == epoch);
    }

    Json(attempts)
}
/// GET /api/v1/history — returns the log of all formal verification attempts, optionally filtered.

pub async fn search_history_by_hash(
    State(state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> Json<Vec<crate::fact_graph::VerificationAttempt>> {
    let fg = state.fact_graph.read().await;
    let attempts = fg
        .attempts
        .iter()
        .filter(|a| a.payload_hash == hash)
        .cloned()
        .collect();
    Json(attempts)
}
/// GET /api/v1/history/search/:hash — returns all verification attempts matching a payload proof hash.


#[derive(Debug, Deserialize)]
pub struct AddGroundVectorRequest {
    pub label: String,
    pub content: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default = "default_policy")]
    pub rotation_policy: String,
}
/// Request body for injecting a new steering ground vector into the ledger.

fn default_weight() -> f64 {
    1.0
}
fn default_policy() -> String {
    "next_epoch".to_string()
}

pub async fn add_ground_vector(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddGroundVectorRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let policy = match req.rotation_policy.as_str() {
        "immediate" => RotationPolicy::Immediate,
        _ => RotationPolicy::NextEpoch,
    };
    let is_immediate = policy == RotationPolicy::Immediate;

    let id = {
        let mut ledger = state.ledger.write().await;
        let id = add_entry(&mut ledger, req.label, req.content, req.weight, policy);
        save_ledger(&ledger)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        id
    };

    if is_immediate {
        info!(id = %id, "Immediate rotation triggered by ground vector injection");
        state.rotation_trigger.notify_one();
    }

    Ok(Json(serde_json::json!({
        "id": id,
        "status": "added",
        "rotation": if is_immediate { "immediate" } else { "next_epoch" }
    })))
}
/// POST /api/v1/ledger/vectors — injects a steering ground vector; `immediate` policy triggers instant pod rotation.

pub async fn neutralize_ground_vector(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut ledger = state.ledger.write().await;
    neutralize_entry(&mut ledger, &id).map_err(|e| ApiError::Rejected(e.to_string()))?;
    save_ledger(&ledger)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "id": id, "status": "neutralized" })))
}
/// POST /api/v1/ledger/vectors/:id/neutralize — soft-deletes a ground vector, excluding it from future alignment scoring.

pub async fn list_ground_vectors(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<crate::ledger::LedgerEntry>> {
    let ledger = state.ledger.read().await;
    Json(ledger.entries.values().cloned().collect())
}
/// GET /api/v1/ledger/vectors — lists all ledger entries including neutralized ones.

pub async fn list_facts(State(state): State<Arc<AppState>>) -> Json<Vec<FactNode>> {
    let fg = state.fact_graph.read().await;
    Json(fg.nodes.values().cloned().collect())
}
/// GET /api/v1/facts — returns all permanently committed verified fact nodes.

pub async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}
// GET /healthz — liveness probe endpoint returning 200 OK.

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/state/ingest", post(ingest_state))
        .route("/healthz", get(healthz))
        .route("/api/v1/verify", post(api_verify))
        .route("/api/v1/ledger/vectors", post(add_ground_vector))
        .route(
            "/api/v1/ledger/vectors/:id/neutralize",
            post(neutralize_ground_vector),
        )
        .route("/api/v1/ledger/vectors", get(list_ground_vectors))
        .route("/api/v1/facts", get(list_facts))
        .route("/api/v1/history", get(list_history))
        .route("/api/v1/history/search/:hash", get(search_history_by_hash))
        .with_state(state)
}
// Constructs the Axum router with all AMTD controller endpoints mounted.

