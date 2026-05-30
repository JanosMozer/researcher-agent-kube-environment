use amtd_controller::shim::{run_signal_interceptor, ShimRuntime};
use anyhow::Result;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let runtime = Arc::new(ShimRuntime::from_env()?);
    info!(pod = %runtime.pod_name, agent_id = %runtime.agent_id, "Agent shim initialized");

    run_signal_interceptor(runtime).await?;
    Ok(())
}
