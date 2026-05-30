use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub target_namespace: String,
    pub agent_image: String,
    pub epoch_duration_sec: u64,
    pub llm_api_key: String,
    pub warm_pool_size: u32,
    #[serde(default = "default_controller_port")]
    pub controller_port: u16,
    #[serde(default = "default_state_endpoint")]
    pub state_endpoint: String,
}
/// Environment-driven or CLI-driven cluster configuration struct.

fn default_controller_port() -> u16 {
    8080
}
/// Returns the default controller port (8080).

fn default_state_endpoint() -> String {
    "/state/ingest".to_string()
}
/// Returns the default path for the state ingestion HTTP endpoint.

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing or invalid environment variable: {0}")]
    EnvError(#[from] envy::Error),
}
/// Typed error variants for configuration loading failures.

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        envy::from_env::<Config>().map_err(ConfigError::EnvError)
    }
}
// Constructs `Config` by deserializing all required environment variables.

impl Config {
    pub fn from_cli(cli: &crate::cli::Cli) -> Self {
        Self {
            target_namespace: cli.namespace.clone(),
            agent_image: cli.agent_image.clone(),
            epoch_duration_sec: cli.epoch,
            llm_api_key: cli.llm_key.clone(),
            warm_pool_size: cli.warm_pool_size,
            controller_port: cli.port,
            state_endpoint: "/state/ingest".to_string(),
        }
    }
}
// Constructs `Config` from a parsed `Cli` struct, prioritizing explicit CLI flags over environment defaults.
