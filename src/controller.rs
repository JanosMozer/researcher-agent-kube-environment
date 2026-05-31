use crate::config::Config;
use crate::state::AgentState;
use anyhow::Result;
use k8s_openapi::api::core::v1::{
    Affinity, NodeAffinity, Pod, PodAffinityTerm, PodAntiAffinity, PodSpec,
    ResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{
    api::{Api, DeleteParams, Patch, PatchParams, PostParams},
    Client,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Notify, RwLock};
use tokio::time::{interval, Duration};
use tracing::{error, info};

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),
    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),
    #[error("State verification failed for pod: {0}")]
    StateVerificationFailed(String),
    #[error("Warm pool exhausted")]
    WarmPoolExhausted,
}
/// Typed error surface for all controller-layer failures.

pub struct ControllerState {
    pub config: Config,
    pub pending_state: Option<AgentState>,
    pub active_pod_name: Option<String>,
    pub warm_pod_names: Vec<String>,
}
/// Shared mutable runtime state for the controller reconciliation loop.

pub struct Controller {
    pub client: Client,
    pub state: Arc<RwLock<ControllerState>>,
}
/// Root controller handle owning the Kubernetes client and shared runtime state.

impl Controller {
    pub async fn new(config: Config) -> Result<Self, ControllerError> {
        let client = Client::try_default().await?;
        let state = Arc::new(RwLock::new(ControllerState {
            config,
            pending_state: None,
            active_pod_name: None,
            warm_pod_names: Vec::new(),
        }));
        Ok(Self { client, state })
    }
}
/// Initializes the Kubernetes in-cluster or kubeconfig client and zeroes runtime state.

pub async fn build_agent_pod_manifest(
    name: &str,
    config: &Config,
    warm: bool,
    injected_state: Option<&AgentState>,
) -> Result<Pod, ControllerError> {
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), "amtd-agent".to_string());
    labels.insert(
        "role".to_string(),
        if warm { "warm" } else { "active" }.to_string(),
    );

    let mut env_vars = vec![
        k8s_openapi::api::core::v1::EnvVar {
            name: "LLM_API_KEY".to_string(),
            value_from: Some(k8s_openapi::api::core::v1::EnvVarSource {
                secret_key_ref: Some(k8s_openapi::api::core::v1::SecretKeySelector {
                    name: "amtd-secrets".to_string(),
                    key: "llm-api-key".to_string(),
                    optional: Some(false),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        k8s_openapi::api::core::v1::EnvVar {
            name: "CONTROLLER_ENDPOINT".to_string(),
            value: Some(format!(
                "http://amtd-controller:{}",
                config.controller_port
            )),
            ..Default::default()
        },
    ];

    if let Some(state) = injected_state {
        let serialized = serde_json::to_string(state)?;
        env_vars.push(k8s_openapi::api::core::v1::EnvVar {
            name: "INITIAL_STATE".to_string(),
            value: Some(serialized),
            ..Default::default()
        });
    }

    let anti_affinity = PodAntiAffinity {
        required_during_scheduling_ignored_during_execution: Some(vec![PodAffinityTerm {
            label_selector: Some(LabelSelector {
                match_labels: Some({
                    let mut m = BTreeMap::new();
                    m.insert("app".to_string(), "amtd-agent".to_string());
                    m
                }),
                ..Default::default()
            }),
            topology_key: "kubernetes.io/hostname".to_string(),
            ..Default::default()
        }]),
        ..Default::default()
    };

    let pod = Pod {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(config.target_namespace.clone()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(PodSpec {
            restart_policy: Some("Never".to_string()),
            affinity: Some(Affinity {
                pod_anti_affinity: Some(anti_affinity),
                node_affinity: Some(NodeAffinity::default()),
                ..Default::default()
            }),
            containers: vec![k8s_openapi::api::core::v1::Container {
                name: "agent".to_string(),
                image: Some(config.agent_image.clone()),
                image_pull_policy: Some("IfNotPresent".to_string()),
                env: Some(env_vars),
                resources: Some(ResourceRequirements {
                    requests: Some({
                        let mut r = BTreeMap::new();
                        r.insert(
                            "memory".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                "512Mi".to_string(),
                            ),
                        );
                        r.insert(
                            "cpu".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                "250m".to_string(),
                            ),
                        );
                        r
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    Ok(pod)
}
/// Constructs a `Pod` manifest for active or warm-standby agent roles with anti-affinity and optional state injection.

pub async fn provision_warm_pod(
    controller: &Controller,
    name: &str,
) -> Result<(), ControllerError> {
    let state_guard = controller.state.read().await;
    let config = state_guard.config.clone();
    drop(state_guard);

    let pod_api: Api<Pod> =
        Api::namespaced(controller.client.clone(), &config.target_namespace);
    let manifest = build_agent_pod_manifest(name, &config, true, None).await?;
    pod_api.create(&PostParams::default(), &manifest).await?;
    info!(pod = name, "Warm standby pod provisioned");

    let mut state_guard = controller.state.write().await;
    state_guard.warm_pod_names.push(name.to_string());
    Ok(())
}
/// Creates a warm-standby agent pod with anti-affinity constraints to eliminate scheduling lag at rotation.

pub async fn rotate_active_pod(controller: &Controller) -> Result<(), ControllerError> {
    let state_guard = controller.state.read().await;
    let config = state_guard.config.clone();
    let active_pod = state_guard.active_pod_name.clone();
    let warm_pods = state_guard.warm_pod_names.clone();
    drop(state_guard);

    let pod_api: Api<Pod> =
        Api::namespaced(controller.client.clone(), &config.target_namespace);

    let next_warm = warm_pods.first().ok_or(ControllerError::WarmPoolExhausted)?;

    if let Some(ref active_name) = active_pod {
        info!(pod = active_name, "Issuing graceful termination to active pod");
        pod_api
            .delete(active_name, &DeleteParams::default())
            .await?;
    }

    let pending_state = {
        let guard = controller.state.read().await;
        guard.pending_state.clone()
    };

    let promoted_active_name = if let Some(ref captured_state) = pending_state {
        let expected_sig =
            captured_state.compute_signature(&config.llm_api_key);
        if captured_state.signature != expected_sig {
            return Err(ControllerError::StateVerificationFailed(
                captured_state.pod_name.clone(),
            ));
        }

        let new_active_name = format!(
            "amtd-agent-{}",
            uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x")
        );

        let manifest =
            build_agent_pod_manifest(&new_active_name, &config, false, Some(captured_state))
                .await?;
        pod_api.create(&PostParams::default(), &manifest).await?;
        info!(
            pod = new_active_name,
            "Active pod promoted from warm pool with injected state"
        );
        new_active_name
    } else {
        let promote_patch = serde_json::json!({
            "metadata": { "labels": { "role": "active" } }
        });
        pod_api
            .patch(
                next_warm,
                &PatchParams::default(),
                &Patch::Merge(&promote_patch),
            )
            .await?;
        info!(pod = next_warm, "Warm pod promoted to active (no state)");
        next_warm.clone()
    };

    let replenish_name = format!("amtd-warm-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x"));

    let mut state_guard = controller.state.write().await;
    state_guard.active_pod_name = Some(promoted_active_name);
    state_guard.warm_pod_names.retain(|p| p != next_warm);
    state_guard.pending_state = None;
    drop(state_guard);

    provision_warm_pod(controller, &replenish_name).await?;
    Ok(())
}
/// Executes a full state rotation: terminates active pod, verifies + injects captured state into promoted warm pod, replenishes warm pool.

pub async fn run_epoch_loop(controller: Arc<Controller>, trigger: Arc<Notify>) -> Result<()> {
    let epoch_secs = {
        let guard = controller.state.read().await;
        guard.config.epoch_duration_sec
    };

    let mut ticker = interval(Duration::from_secs(epoch_secs));
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                info!("Epoch boundary reached — initiating state rotation");
            }
            _ = trigger.notified() => {
                info!("Immediate rotation triggered via ground vector injection");
                ticker.reset();
            }
        }
        if let Err(e) = rotate_active_pod(&controller).await {
            error!(error = %e, "Rotation failed");
        }
    }
}
// Drives the deterministic epoch-boundary rotation loop with tokio::select! over timer and immediate-trigger Notify.
