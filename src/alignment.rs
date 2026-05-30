use crate::state::{AgentState, SemanticContext};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundVector {
    pub objective_id: String,
    pub embedding: Vec<f64>,
    pub label: String,
    pub weight: f64,
}
/// An immutable primary-objective anchor vector used for trajectory alignment scoring.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlignmentReport {
    pub passed: bool,
    pub score: f64,
    pub threshold: f64,
    pub active_vectors: Vec<GroundVector>,
    pub ed25519_signature: String,
    pub signed_payload_hash: String,
}
/// Output of a full alignment gate pass: score, pass/fail, injected vectors, and cryptographic signature.

#[derive(Debug, Error)]
pub enum AlignmentError {
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Alignment score {score:.4} below threshold {threshold:.4}")]
    BelowThreshold { score: f64, threshold: f64 },
    #[error("Signing error: {0}")]
    Signing(String),
}
/// Typed errors for alignment gate rejections and cryptographic failures.

pub struct AlignmentGate {
    pub objective: String,
    pub threshold: f64,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub ground_vectors: Vec<GroundVector>,
}
/// Manages immutable objective anchoring, semantic trajectory scoring, and ed25519 state signing.

impl AlignmentGate {
    pub fn new(objective: &str, threshold: f64) -> Self {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        Self {
            objective: objective.to_string(),
            threshold,
            signing_key,
            verifying_key,
            ground_vectors: vec![],
        }
    }
}
/// Generates an ephemeral ed25519 keypair and configures objective anchor and scoring threshold.

pub fn strip_structural_metadata(state: &AgentState) -> HashMap<String, serde_json::Value> {
    let mut stripped: HashMap<String, serde_json::Value> = HashMap::new();
    stripped.insert(
        "sanitized_summary".to_string(),
        serde_json::Value::String(state.semantic.sanitized_summary.clone()),
    );
    stripped.insert(
        "key_findings".to_string(),
        serde_json::to_value(&state.semantic.key_findings).unwrap_or(serde_json::Value::Null),
    );
    stripped.insert(
        "scratchpad_tokens".to_string(),
        serde_json::to_value(&state.semantic.scratchpad_tokens).unwrap_or(serde_json::Value::Null),
    );
    stripped
}
/// Extracts only semantic fields from `AgentState`, dropping DAG topology and pod identity metadata.

fn text_to_embedding(text: &str) -> Vec<f64> {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest
        .chunks(4)
        .map(|chunk| {
            let val = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32);
            (val as f64) / (u32::MAX as f64)
        })
        .collect()
}
/// Produces a deterministic 8-dimensional [0,1] embedding from SHA-256 digest chunks of input text.

fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    let min_len = a.len().min(b.len());
    let dot: f64 = a[..min_len].iter().zip(b[..min_len].iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a[..min_len].iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b[..min_len].iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}
/// Computes cosine similarity between two embedding slices, returning 0.0 for zero-norm inputs.

pub fn compute_alignment_score(
    semantic: &SemanticContext,
    objective: &str,
    ground_vectors: &[GroundVector],
) -> f64 {
    let combined_text = format!(
        "{} {} {}",
        semantic.sanitized_summary,
        semantic.key_findings.join(" "),
        semantic.scratchpad_tokens.join(" ")
    );
    let state_embedding = text_to_embedding(&combined_text);
    let objective_embedding = text_to_embedding(objective);

    let mut base_score = cosine_similarity(&state_embedding, &objective_embedding);

    if !ground_vectors.is_empty() {
        let gv_scores: f64 = ground_vectors
            .iter()
            .map(|gv| cosine_similarity(&state_embedding, &gv.embedding) * gv.weight)
            .sum::<f64>()
            / ground_vectors.iter().map(|gv| gv.weight).sum::<f64>().max(1e-9);
        base_score = (base_score + gv_scores) / 2.0;
    }

    base_score
}
/// Fuses state embedding cosine similarity against objective + weighted GroundVectors into a scalar alignment score.

pub fn sign_payload(gate: &AlignmentGate, payload: &str) -> Result<(String, String), AlignmentError> {
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let hash = hex::encode(hasher.finalize());

    let signature: Signature = gate.signing_key.sign(hash.as_bytes());
    let sig_hex = hex::encode(signature.to_bytes());
    Ok((sig_hex, hash))
}
/// Signs the SHA-256 hash of `payload` with the gate's ed25519 signing key, returning (signature_hex, hash_hex).

pub fn run_alignment_gate(
    gate: &mut AlignmentGate,
    state: &AgentState,
) -> Result<AlignmentReport, AlignmentError> {
    let stripped = strip_structural_metadata(state);
    let stripped_json = serde_json::to_string(&stripped)?;

    let score = compute_alignment_score(&state.semantic, &gate.objective, &gate.ground_vectors);

    if score < gate.threshold {
        warn!(
            score = score,
            threshold = gate.threshold,
            pod = %state.pod_name,
            "Alignment gate REJECTED: trajectory below objective threshold"
        );
        return Err(AlignmentError::BelowThreshold {
            score,
            threshold: gate.threshold,
        });
    }

    info!(
        score = score,
        pod = %state.pod_name,
        "Alignment gate PASSED"
    );

    let new_gv = GroundVector {
        objective_id: uuid::Uuid::new_v4().to_string(),
        embedding: text_to_embedding(&state.semantic.sanitized_summary),
        label: format!("epoch_{}_pod_{}", state.dag.epoch, state.pod_name),
        weight: score,
    };
    gate.ground_vectors.push(new_gv);

    let active_vectors = gate.ground_vectors.clone();
    let full_payload = format!("{}:{}", stripped_json, score);
    let (ed25519_signature, signed_payload_hash) = sign_payload(gate, &full_payload)?;

    Ok(AlignmentReport {
        passed: true,
        score,
        threshold: gate.threshold,
        active_vectors,
        ed25519_signature,
        signed_payload_hash,
    })
}
/// Strips metadata, scores semantic trajectory, appends GroundVector, and signs the payload — returns `AlignmentReport`.

pub fn get_verifying_key_hex(gate: &AlignmentGate) -> String {
    hex::encode(gate.verifying_key.to_bytes())
}
// Returns the hex-encoded ed25519 verifying key for out-of-band distribution to state consumers.
