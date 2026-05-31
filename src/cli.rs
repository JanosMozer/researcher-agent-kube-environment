use clap::{Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
#[command(name = "raket-controller", about = "RAKET Kubernetes Controller — Researcher-Agent-Kubernetes-EnvironmenT")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    #[arg(long, env = "EPOCH_DURATION_SEC", default_value = "300")]
    pub epoch: u64,
    /// LLM API key and HMAC signing secret
    #[arg(long = "llm-key", env = "LLM_API_KEY", default_value = "")]
    pub llm_key: String,
    /// User-injected steering ground vectors (comma-separated labels:content:weight:policy tuples)
    #[arg(long = "ground-vector")]
    pub ground_vectors: Vec<String>,
    #[arg(long, env = "TARGET_NAMESPACE", default_value = "raket")]
    pub namespace: String,
    #[arg(long = "agent-image", env = "AGENT_IMAGE", default_value = "raket-agent:latest")]
    pub agent_image: String,
    #[arg(long = "warm-pool-size", env = "WARM_POOL_SIZE", default_value = "2")]
    pub warm_pool_size: u32,
    #[arg(long, env = "CONTROLLER_PORT", default_value = "8080")]
    pub port: u16,
    #[arg(long = "fact-graph-path", env = "FACT_GRAPH_PATH", default_value = "fact_graph.json")]
    pub fact_graph_path: String,
    #[arg(long = "ledger-path", env = "LEDGER_PATH", default_value = "ledger.json")]
    pub ledger_path: String,
    #[arg(long = "verify-timeout", env = "VERIFY_TIMEOUT_SEC", default_value = "30")]
    pub verify_timeout_sec: u64,
    #[arg(long = "verify-work-dir", env = "VERIFY_WORK_DIR", default_value = "/tmp/raket-verify")]
    pub verify_work_dir: String,
    #[arg(long = "alignment-objective", env = "ALIGNMENT_OBJECTIVE", default_value = "Autonomous research")]
    pub alignment_objective: String,
    #[arg(long = "alignment-threshold", env = "ALIGNMENT_THRESHOLD", default_value = "0.0")]
    pub alignment_threshold: f64,
}
/// Clap CLI struct mapping all controller configuration from flags or environment variables.

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Start the RAKET controller daemon (default behavior if no subcommand is given)
    Run,
    /// Query permanently committed facts
    QueryFacts {
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        backend: Option<String>,
        #[arg(long)]
        epoch: Option<u64>,
    },
    /// List all theories and findings sent to the formal verification engine (history)
    ListHistory {
        #[arg(long)]
        backend: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        epoch: Option<u64>,
    },
    /// Search for theories, findings, or verified facts matching a payload proof hash
    Search {
        hash: String,
    },
    /// List all steering ground vectors in the ledger
    ListVectors,
    /// Inject a new steering ground vector into the running controller ledger
    AddVector {
        #[arg(long)]
        label: String,
        #[arg(long)]
        content: String,
        #[arg(long, default_value = "1.0")]
        weight: f64,
        #[arg(long, default_value = "next_epoch")]
        policy: String,
    },
    /// Soft-delete (neutralize) a steering ground vector in the ledger
    Neutralize {
        id: String,
    },
    /// Detailed help guide explaining what and how to use the RAKET CLI commands
    HelpGuide,
    /// Show current agent pod status (active or warm) and lists all running pods
    PodStatus,
    /// Fetch and print the logs of a specific running researcher pod
    PodLogs {
        #[arg(long)]
        pod_name: Option<String>,
        #[arg(long, default_value = "100")]
        tail: i64,
    },
}
// Defines all subcommands supported by the CLI tool.

