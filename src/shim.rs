use crate::state::{AgentState, DagNode, DagNodeStatus, ExecutionDag, SemanticContext};
use anyhow::Result;
use chrono::Utc;
use futures::stream::StreamExt;
use reqwest::Client as HttpClient;
use signal_hook::consts::signal::{SIGTERM};
use signal_hook_tokio::Signals;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ShimError {
    #[error("HTTP transmission error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Environment variable missing: {0}")]
    Env(String),
}
/// Typed errors for the in-pod shim signal interceptor.

pub struct ShimRuntime {
    pub agent_id: String,
    pub pod_name: String,
    pub namespace: String,
    pub controller_endpoint: String,
    pub llm_api_key: String,
    pub dag: Arc<RwLock<ExecutionDag>>,
    pub semantic: Arc<RwLock<SemanticContext>>,
}
/// Runtime context for the in-pod SIGTERM shim holding all mutable agent state references.

impl ShimRuntime {
    pub fn from_env() -> Result<Self, ShimError> {
        let pod_name = env::var("POD_NAME")
            .map_err(|_| ShimError::Env("POD_NAME".to_string()))?;
        let namespace = env::var("POD_NAMESPACE")
            .map_err(|_| ShimError::Env("POD_NAMESPACE".to_string()))?;
        let controller_endpoint = env::var("CONTROLLER_ENDPOINT")
            .map_err(|_| ShimError::Env("CONTROLLER_ENDPOINT".to_string()))?;
        let llm_api_key = env::var("LLM_API_KEY")
            .map_err(|_| ShimError::Env("LLM_API_KEY".to_string()))?;

        let initial_dag = env::var("INITIAL_STATE").ok().and_then(|s| {
            serde_json::from_str::<AgentState>(&s)
                .ok()
                .map(|a| a.dag)
        }).unwrap_or_else(|| ExecutionDag {
            nodes: vec![DagNode {
                id: Uuid::new_v4().to_string(),
                task_type: "research_init".to_string(),
                dependencies: vec![],
                status: DagNodeStatus::Pending,
                metadata: HashMap::new(),
            }],
            current_task_pointer: None,
            epoch: 0,
        });

        let initial_semantic = env::var("INITIAL_STATE").ok().and_then(|s| {
            serde_json::from_str::<AgentState>(&s)
                .ok()
                .map(|a| a.semantic)
        }).unwrap_or_else(|| SemanticContext {
            sanitized_summary: String::new(),
            key_findings: vec![],
            scratchpad_tokens: vec![],
            source_urls: vec![],
        });

        Ok(Self {
            agent_id: Uuid::new_v4().to_string(),
            pod_name,
            namespace,
            controller_endpoint,
            llm_api_key,
            dag: Arc::new(RwLock::new(initial_dag)),
            semantic: Arc::new(RwLock::new(initial_semantic)),
        })
    }
}
/// Constructs `ShimRuntime` from environment variables, optionally rehydrating state from `INITIAL_STATE`.

pub async fn serialize_and_transmit(runtime: &ShimRuntime) -> Result<(), ShimError> {
    let dag = runtime.dag.read().await.clone();
    let semantic = runtime.semantic.read().await.clone();

    let mut state = AgentState {
        agent_id: runtime.agent_id.clone(),
        pod_name: runtime.pod_name.clone(),
        namespace: runtime.namespace.clone(),
        captured_at: Utc::now(),
        dag,
        semantic,
        signature: String::new(),
    };
    state.signature = state.compute_signature(&runtime.llm_api_key);

    let client = HttpClient::new();
    let url = format!("{}/state/ingest", runtime.controller_endpoint);
    client
        .post(&url)
        .json(&state)
        .send()
        .await?
        .error_for_status()?;

    info!(pod = %runtime.pod_name, "State serialized and transmitted to controller");
    Ok(())
}
/// Snapshots current DAG + semantic state, signs it, and POST-transmits to the controller before pod exit.

pub async fn run_signal_interceptor(runtime: Arc<ShimRuntime>) -> Result<(), ShimError> {
    let mut signals = Signals::new([SIGTERM])
        .map_err(|e| ShimError::Env(format!("Signal registration failed: {e}")))?;

    info!(pod = %runtime.pod_name, "SIGTERM interceptor active");
    while let Some(sig) = signals.next().await {
        match sig {
            SIGTERM => {
                info!("SIGTERM received — serializing state before exit");
                if let Err(e) = serialize_and_transmit(&runtime).await {
                    error!(error = %e, "State transmission failed");
                }
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
// Registers SIGTERM handler via `signal_hook_tokio`; on receipt, serializes and transmits state then exits.
