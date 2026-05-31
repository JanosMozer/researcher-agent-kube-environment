use raket_controller::api::{build_router, AppState};
use raket_controller::cli::{Cli, Commands};
use raket_controller::config::Config;
use raket_controller::controller::{provision_warm_pod, run_epoch_loop, Controller};
use raket_controller::fact_graph::{load_fact_graph, FactNode, VerificationAttempt};
use raket_controller::ledger::{add_entry, neutralize_entry, load_ledger, save_ledger, RotationPolicy, LedgerEntry};
use raket_controller::verify::VerificationEngine;
use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

async fn execute_subcommand(cli: &Cli, cmd: &Commands) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let base_url = format!("http://127.0.0.1:{}", cli.port);

    match cmd {
        Commands::Run => Ok(()),
        Commands::QueryFacts { tag, backend, epoch } => {
            let facts = match client.get(format!("{}/api/v1/facts", base_url)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    resp.json::<Vec<FactNode>>().await.ok()
                }
                _ => None,
            };

            let facts = match facts {
                Some(f) => f,
                None => {
                    let fg = load_fact_graph(&cli.fact_graph_path).await?;
                    fg.nodes.values().cloned().collect::<Vec<_>>()
                }
            };

            let mut filtered = facts;
            if let Some(t) = tag {
                filtered.retain(|f| f.tags.iter().any(|tg| tg == t));
            }
            if let Some(b) = backend {
                filtered.retain(|f| &f.backend == b);
            }
            if let Some(ep) = epoch {
                filtered.retain(|f| f.epoch == *ep);
            }

            if filtered.is_empty() {
                println!("No verified facts found matching criteria.");
            } else {
                println!("{:<36} | {:<10} | {:<5} | {:<20} | {}", "PROOFS (HASH)", "BACKEND", "EPOCH", "VERIFIED AT", "SUMMARY");
                println!("{}", "-".repeat(110));
                for fact in filtered {
                    println!(
                        "{:<36} | {:<10} | {:<5} | {:<20} | {}",
                        fact.proof_hash,
                        fact.backend,
                        fact.epoch,
                        fact.verified_at.format("%Y-%m-%d %H:%M:%S"),
                        fact.payload_summary
                    );
                }
            }
            Ok(())
        }
        Commands::ListHistory { backend, status, epoch } => {
            let mut url = format!("{}/api/v1/history", base_url);
            let mut queries = Vec::new();
            if let Some(b) = backend {
                queries.push(format!("backend={}", b));
            }
            if let Some(s) = status {
                queries.push(format!("status={}", s));
            }
            if let Some(ep) = epoch {
                queries.push(format!("epoch={}", ep));
            }
            if !queries.is_empty() {
                url = format!("{}?{}", url, queries.join("&"));
            }

            let attempts = match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    resp.json::<Vec<VerificationAttempt>>().await.ok()
                }
                _ => None,
            };

            let attempts = match attempts {
                Some(a) => a,
                None => {
                    let fg = load_fact_graph(&cli.fact_graph_path).await?;
                    let mut local = fg.attempts.clone();
                    if let Some(b) = backend {
                        local.retain(|a| &a.backend == b);
                    }
                    if let Some(s) = status {
                        match s.as_str() {
                            "passed" | "success" | "true" => local.retain(|a| a.success),
                            "failed" | "failure" | "false" => local.retain(|a| !a.success),
                            _ => {}
                        }
                    }
                    if let Some(ep) = epoch {
                        local.retain(|a| a.epoch == *ep);
                    }
                    local
                }
            };

            if attempts.is_empty() {
                println!("No verification attempts found matching criteria.");
            } else {
                println!("{:<36} | {:<10} | {:<5} | {:<7} | {:<20} | {}", "ID", "BACKEND", "EPOCH", "STATUS", "SUBMITTED AT", "HASH");
                println!("{}", "-".repeat(110));
                for attempt in attempts {
                    println!(
                        "{:<36} | {:<10} | {:<5} | {:<7} | {:<20} | {}",
                        attempt.id,
                        attempt.backend,
                        attempt.epoch,
                        if attempt.success { "PASSED" } else { "FAILED" },
                        attempt.submitted_at.format("%Y-%m-%d %H:%M:%S"),
                        attempt.payload_hash
                    );
                }
            }
            Ok(())
        }
        Commands::Search { hash } => {
            let attempts = match client.get(format!("{}/api/v1/history/search/{}", base_url, hash)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    resp.json::<Vec<VerificationAttempt>>().await.ok()
                }
                _ => None,
            };

            let attempts = match attempts {
                Some(a) => a,
                None => {
                    let fg = load_fact_graph(&cli.fact_graph_path).await?;
                    fg.attempts.iter().filter(|a| a.payload_hash == *hash).cloned().collect::<Vec<_>>()
                }
            };

            if attempts.is_empty() {
                println!("No theories, findings, or verification attempts found for hash: {}", hash);
            } else {
                for (i, attempt) in attempts.iter().enumerate() {
                    println!("Match #{} (ID: {})", i + 1, attempt.id);
                    println!("================================================================================");
                    println!("Submitted At: {}", attempt.submitted_at.format("%Y-%m-%d %H:%M:%S"));
                    println!("Epoch:        {}", attempt.epoch);
                    println!("Backend:      {}", attempt.backend);
                    println!("Status:       {}", if attempt.success { "PASSED" } else { "FAILED" });
                    println!("Tags:         {:?}", attempt.tags);
                    if let Some(ref err) = attempt.error_message {
                        println!("Error Msg:    {}", err);
                    }
                    println!("\n--- STDIN / PAYLOAD ---");
                    println!("{}", attempt.payload.trim());
                    if !attempt.stdout.is_empty() {
                        println!("\n--- STDOUT ---");
                        println!("{}", attempt.stdout.trim());
                    }
                    if !attempt.stderr.is_empty() {
                        println!("\n--- STDERR ---");
                        println!("{}", attempt.stderr.trim());
                    }
                    if !attempt.tactic_goals.is_empty() {
                        println!("\n--- TACTIC GOALS ---");
                        for goal in &attempt.tactic_goals {
                            println!("  {}", goal);
                        }
                    }
                    println!("================================================================================\n");
                }
            }
            Ok(())
        }
        Commands::ListVectors => {
            let vectors = match client.get(format!("{}/api/v1/ledger/vectors", base_url)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    resp.json::<Vec<LedgerEntry>>().await.ok()
                }
                _ => None,
            };

            let vectors = match vectors {
                Some(v) => v,
                None => {
                    let ledger = load_ledger(&cli.ledger_path).await?;
                    ledger.entries.values().cloned().collect::<Vec<_>>()
                }
            };

            if vectors.is_empty() {
                println!("Ledger contains no steering ground vectors.");
            } else {
                println!("{:<36} | {:<20} | {:<6} | {:<10} | {:<10} | {}", "ID", "LABEL", "WEIGHT", "POLICY", "STATUS", "CONTENT");
                println!("{}", "-".repeat(110));
                for v in vectors {
                    let status = if v.neutralized_at.is_some() { "NEUTRALIZED" } else { "ACTIVE" };
                    let policy_str = match v.rotation_policy {
                        RotationPolicy::Immediate => "immediate",
                        RotationPolicy::NextEpoch => "next_epoch",
                    };
                    println!(
                        "{:<36} | {:<20} | {:<6.2} | {:<10} | {:<10} | {}",
                        v.id,
                        v.label,
                        v.weight,
                        policy_str,
                        status,
                        v.content
                    );
                }
            }
            Ok(())
        }
        Commands::AddVector { label, content, weight, policy } => {
            let rotation_policy = match policy.as_str() {
                "immediate" => "immediate",
                _ => "next_epoch",
            };

            let body = serde_json::json!({
                "label": label,
                "content": content,
                "weight": weight,
                "rotation_policy": rotation_policy
            });

            match client.post(format!("{}/api/v1/ledger/vectors", base_url)).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let res: serde_json::Value = resp.json().await?;
                    println!("Successfully added vector via running controller API:");
                    println!("  ID:       {}", res["id"]);
                    println!("  Rotation: {}", res["rotation"]);
                }
                _ => {
                    println!("Controller daemon not responding. Applying modifications directly to ledger file on disk...");
                    let mut ledger = load_ledger(&cli.ledger_path).await?;
                    let rot = match rotation_policy {
                        "immediate" => RotationPolicy::Immediate,
                        _ => RotationPolicy::NextEpoch,
                    };
                    let id = add_entry(&mut ledger, label.clone(), content.clone(), *weight, rot);
                    save_ledger(&ledger).await?;
                    println!("Successfully committed new vector directly to disk:");
                    println!("  ID:       {}", id);
                    println!("  Ledger:   {}", cli.ledger_path);
                    println!("  Note:     Daemon is offline; any immediate rotation will not trigger automatically.");
                }
            }
            Ok(())
        }
        Commands::Neutralize { id } => {
            match client.post(format!("{}/api/v1/ledger/vectors/{}/neutralize", base_url, id)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    println!("Successfully neutralized vector {} via running controller API.", id);
                }
                _ => {
                    println!("Controller daemon not responding. Applying modifications directly to ledger file on disk...");
                    let mut ledger = load_ledger(&cli.ledger_path).await?;
                    neutralize_entry(&mut ledger, id)?;
                    save_ledger(&ledger).await?;
                    println!("Successfully neutralized vector {} directly on disk.", id);
                }
            }
            Ok(())
        }
        Commands::HelpGuide => {
            println!("================================================================================");
            println!("RAKET CLI Tool - Detailed Command Guide");
            println!("================================================================================");
            println!("The RAKET CLI enables real-time auditing and steering of the background ");
            println!("autonomous mathematical researcher pods running in Kubernetes.\n");
            println!("1. Query Committed Facts");
            println!("   Lists all permanently committed mathematical findings that successfully passed");
            println!("   formal verification.");
            println!("   Usage:   cargo run --bin controller -- query-facts [--tag <TAG>] [--backend <BACKEND>] [--epoch <EPOCH>]");
            println!("   Example: cargo run --bin controller -- query-facts --tag raket\n");
            println!("2. List History");
            println!("   Lists all historical mathematical conjectures and theories submitted to the");
            println!("   formal verification sandbox (both passed and failed).");
            println!("   Usage:   cargo run --bin controller -- list-history [--backend <BACKEND>] [--status <passed|failed>] [--epoch <EPOCH>]");
            println!("   Example: cargo run --bin controller -- list-history --status failed\n");
            println!("3. Search by Proof Hash");
            println!("   Retrieve the exact Python code and standard output/error trace of any ");
            println!("   conjecture or attempt matching a specific payload hash.");
            println!("   Usage:   cargo run --bin controller -- search <HASH>");
            println!("   Example: cargo run --bin controller -- search df87a39704b58d35dba1767cb13c0208b4bdc76190e508b6273d4be7ec03635f\n");
            println!("4. Steering Ground Vectors");
            println!("   - List Vectors:");
            println!("     Displays all current steering directives in the ledger that control the agent.");
            println!("     Usage:   cargo run --bin controller -- list-vectors");
            println!("   - Add Vector:");
            println!("     Injects a new steering directive to influence future epochs. Immediate policy");
            println!("     triggers an instant warm-pool pod promotion.");
            println!("     Usage:   cargo run --bin controller -- add-vector --label <LABEL> --content <CONTENT> [--weight <WEIGHT>] [--policy <immediate|next_epoch>]");
            println!("     Example: cargo run --bin controller -- add-vector --label \"Symmetry\" --content \"Favor divisor symmetries\" --policy immediate");
            println!("   - Neutralize Vector:");
            println!("     Soft-deletes a directive to disable its influence.");
            println!("     Usage:   cargo run --bin controller -- neutralize <ID>");
            println!("================================================================================");
            Ok(())
        }
        Commands::PodStatus => {
            let output = std::process::Command::new("kubectl")
                .args(["get", "pods", "-n", &cli.namespace, "-o", "wide"])
                .output()?;
            println!("{}", String::from_utf8_lossy(&output.stdout));
            Ok(())
        }
        Commands::PodLogs { pod_name, tail } => {
            let name = match pod_name {
                Some(n) => n.clone(),
                None => {
                    let output = std::process::Command::new("kubectl")
                        .args(["get", "pods", "-n", &cli.namespace, "-o", "jsonpath={.items[0].metadata.name}"])
                        .output()?;
                    let name_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if name_str.is_empty() {
                        println!("No running researcher pods found in namespace {}.", cli.namespace);
                        return Ok(());
                    }
                    name_str
                }
            };
            println!("Fetching logs for pod {} in namespace {} (tail {}):", name, cli.namespace, tail);
            let output = std::process::Command::new("kubectl")
                .args(["logs", "-n", &cli.namespace, &name, "--tail", &tail.to_string()])
                .output()?;
            println!("{}", String::from_utf8_lossy(&output.stdout));
            Ok(())
        }
    }
}
/// Executes parsed CLI subcommands with graceful daemon API failover to direct JSON store manipulation.

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    if let Some(cmd) = &cli.command {
        if !matches!(cmd, Commands::Run) {
            execute_subcommand(&cli, cmd).await?;
            return Ok(());
        }
    }

    let config = Config::from_cli(&cli);

    info!(
        namespace = %config.target_namespace,
        image = %config.agent_image,
        epoch_sec = config.epoch_duration_sec,
        warm_pool = config.warm_pool_size,
        port = config.controller_port,
        "RAKET Controller starting (out-of-cluster via kubeconfig)"
    );

    tokio::fs::create_dir_all(&cli.verify_work_dir).await?;

    let ledger = {
        let mut l = load_ledger(&cli.ledger_path).await?;
        for raw_gv in &cli.ground_vectors {
            let parts: Vec<&str> = raw_gv.splitn(4, ':').collect();
            let label = parts.first().copied().unwrap_or("cli-vector").to_string();
            let content = parts.get(1).copied().unwrap_or("").to_string();
            let weight: f64 = parts.get(2).copied().and_then(|w| w.parse().ok()).unwrap_or(1.0);
            let policy = match parts.get(3).copied() {
                Some("immediate") => RotationPolicy::Immediate,
                _ => RotationPolicy::NextEpoch,
            };
            add_entry(&mut l, label, content, weight, policy);
        }
        if !cli.ground_vectors.is_empty() {
            save_ledger(&l).await?;
        }
        l
    };

    let fact_graph = load_fact_graph(&cli.fact_graph_path).await?;

    let verification_engine = Arc::new(VerificationEngine::new(
        PathBuf::from(&cli.verify_work_dir),
        cli.verify_timeout_sec,
    ));

    let rotation_trigger = Arc::new(Notify::new());

    let app_state = Arc::new(AppState {
        pending_state: Arc::new(RwLock::new(None)),
        llm_api_key: config.llm_api_key.clone(),
        verification_engine,
        fact_graph: Arc::new(RwLock::new(fact_graph)),
        ledger: Arc::new(RwLock::new(ledger)),
        rotation_trigger: Arc::clone(&rotation_trigger),
    });

    let ctrl = Arc::new(Controller::new(config.clone()).await?);

    let ctrl_warmup = Arc::clone(&ctrl);
    tokio::spawn(async move {
        let warm_pool_size = ctrl_warmup.state.read().await.config.warm_pool_size;
        for i in 0..warm_pool_size {
            let name = format!(
                "raket-warm-{}-{}",
                i,
                Uuid::new_v4().to_string().split('-').next().unwrap_or("x")
            );
            if let Err(e) = provision_warm_pod(&ctrl_warmup, &name).await {
                tracing::error!(error = %e, "Failed to provision warm pod {}", name);
            }
        }
    });

    let ctrl_epoch = Arc::clone(&ctrl);
    let epoch_trigger = Arc::clone(&rotation_trigger);
    tokio::spawn(async move {
        if let Err(e) = run_epoch_loop(ctrl_epoch, epoch_trigger).await {
            tracing::error!(error = %e, "Epoch loop terminated");
        }
    });

    let router = build_router(app_state);
    let addr: SocketAddr = format!("0.0.0.0:{}", config.controller_port).parse()?;
    info!(addr = %addr, "HTTP API listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
// Starts the RAKET controller daemon.
