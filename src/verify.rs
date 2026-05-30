use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub enum VerifierBackend {
    Lean4,
    Z3,
    Sandbox {
        language: String,
        expected_output: String,
        stdin_input: Option<String>,
    },
}
/// Selects the formal verification backend: Lean4 REPL, Z3 SMT solver, or differential code sandbox.

#[derive(Debug, Clone)]
pub struct VerificationEngine {
    pub lean_binary: PathBuf,
    pub z3_binary: PathBuf,
    pub work_dir: PathBuf,
    pub timeout_sec: u64,
}
/// Manages dispatch to external `lean`, `z3`, and sandbox interpreter subprocesses with isolated scratch workspaces.

#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub success: bool,
    pub backend: String,
    pub stdout: String,
    pub stderr: String,
    pub tactic_goals: Vec<String>,
    pub error_message: Option<String>,
}
/// Structured output from a verification subprocess: success flag, raw I/O, parsed Lean tactic goals, and error detail.

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("Subprocess I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Verification timeout after {0}s")]
    Timeout(u64),
    #[error("Binary not found: {0}")]
    BinaryNotFound(String),
    #[error("UTF-8 decode error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("Unsupported sandbox language: {0}")]
    UnsupportedLanguage(String),
}
/// Typed error surface for subprocess spawning, I/O, timeout, and unsupported sandbox language failures.

impl VerificationEngine {
    pub fn new(work_dir: PathBuf, timeout_sec: u64) -> Self {
        Self {
            lean_binary: PathBuf::from(
                std::env::var("LEAN4_BINARY").unwrap_or_else(|_| "lean".to_string()),
            ),
            z3_binary: PathBuf::from(
                std::env::var("Z3_BINARY").unwrap_or_else(|_| "z3".to_string()),
            ),
            work_dir,
            timeout_sec,
        }
    }
}
/// Constructs a `VerificationEngine` reading binary paths from `LEAN4_BINARY` / `Z3_BINARY` env vars.

fn parse_lean_tactic_goals(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.contains('⊢') || l.contains("goals") || l.starts_with("case "))
        .map(|l| l.trim().to_string())
        .collect()
}
/// Extracts Lean 4 tactic proof obligation lines from raw stdout output.

fn parse_z3_result(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{}{}", stdout, stderr);
    combined.contains("unsat") && !combined.contains("sat\n")
}
/// Returns `true` when Z3 stdout contains `unsat` without a contradicting `sat` line.

pub async fn run_lean4(
    engine: &VerificationEngine,
    payload: &str,
) -> Result<VerificationResult, VerifyError> {
    let tmp = tempfile::Builder::new()
        .prefix("amtd_lean_")
        .suffix(".lean")
        .tempfile_in(&engine.work_dir)
        .map_err(VerifyError::Io)?;

    let path = tmp.path().to_path_buf();
    let mut file = tokio::fs::File::create(&path).await?;
    file.write_all(payload.as_bytes()).await?;
    file.flush().await?;
    drop(file);

    info!(file = %path.display(), "Spawning lean4 verification subprocess");

    let result = timeout(
        Duration::from_secs(engine.timeout_sec),
        Command::new(&engine.lean_binary).arg("--run").arg(&path).output(),
    )
    .await
    .map_err(|_| VerifyError::Timeout(engine.timeout_sec))?
    .map_err(VerifyError::Io)?;

    let stdout = String::from_utf8(result.stdout)?;
    let stderr = String::from_utf8(result.stderr)?;
    let tactic_goals = parse_lean_tactic_goals(&stdout);
    let success = result.status.success() && stderr.is_empty();

    if !success {
        warn!(stderr = %stderr, "Lean4 verification produced errors or open tactic goals");
    }

    Ok(VerificationResult {
        success,
        backend: "lean4".to_string(),
        stdout,
        stderr,
        tactic_goals,
        error_message: if success {
            None
        } else {
            Some("Lean4 proof incomplete or errored".to_string())
        },
    })
}
/// Writes `payload` to an isolated temp file, spawns `lean --run`, enforces timeout, returns parsed `VerificationResult`.

pub async fn run_z3(
    engine: &VerificationEngine,
    payload: &str,
) -> Result<VerificationResult, VerifyError> {
    let tmp = tempfile::Builder::new()
        .prefix("amtd_z3_")
        .suffix(".smt2")
        .tempfile_in(&engine.work_dir)
        .map_err(VerifyError::Io)?;

    let path = tmp.path().to_path_buf();
    let mut file = tokio::fs::File::create(&path).await?;
    file.write_all(payload.as_bytes()).await?;
    file.flush().await?;
    drop(file);

    info!(file = %path.display(), "Spawning Z3 verification subprocess");

    let result = timeout(
        Duration::from_secs(engine.timeout_sec),
        Command::new(&engine.z3_binary).arg(&path).output(),
    )
    .await
    .map_err(|_| VerifyError::Timeout(engine.timeout_sec))?
    .map_err(VerifyError::Io)?;

    let stdout = String::from_utf8(result.stdout)?;
    let stderr = String::from_utf8(result.stderr)?;
    let success = parse_z3_result(&stdout, &stderr);

    if !success {
        warn!(stdout = %stdout, stderr = %stderr, "Z3 did not return unsat");
    }

    Ok(VerificationResult {
        success,
        backend: "z3".to_string(),
        stdout,
        stderr,
        tactic_goals: vec![],
        error_message: if success {
            None
        } else {
            Some("Z3 returned sat or unknown".to_string())
        },
    })
}
/// Writes `payload` SMT-LIB2 to an isolated temp file, spawns `z3`, enforces timeout, parses sat/unsat result.

pub async fn run_sandbox(
    engine: &VerificationEngine,
    code: &str,
    language: &str,
    expected_output: &str,
    stdin_input: Option<&str>,
) -> Result<VerificationResult, VerifyError> {
    let (ext, interpreter) = match language {
        "python" | "python3" => (".py", "python3"),
        "bash" | "sh" => (".sh", "bash"),
        "ruby" => (".rb", "ruby"),
        "node" | "js" => (".js", "node"),
        other => return Err(VerifyError::UnsupportedLanguage(other.to_string())),
    };

    let tmp = tempfile::Builder::new()
        .prefix("amtd_sandbox_")
        .suffix(ext)
        .tempfile_in(&engine.work_dir)
        .map_err(VerifyError::Io)?;

    let path = tmp.path().to_path_buf();
    let mut file = tokio::fs::File::create(&path).await?;
    file.write_all(code.as_bytes()).await?;
    file.flush().await?;
    drop(file);

    info!(file = %path.display(), language = language, "Spawning differential sandbox subprocess");

    let mut cmd = Command::new(interpreter);
    cmd.arg(&path);

    if stdin_input.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    let result = timeout(Duration::from_secs(engine.timeout_sec), cmd.output())
        .await
        .map_err(|_| VerifyError::Timeout(engine.timeout_sec))?
        .map_err(VerifyError::Io)?;

    let stdout = String::from_utf8(result.stdout)?;
    let stderr = String::from_utf8(result.stderr)?;

    let output_matches = stdout.trim() == expected_output.trim();
    let success = result.status.success() && output_matches;

    Ok(VerificationResult {
        success,
        backend: format!("sandbox-{language}"),
        stdout: stdout.clone(),
        stderr: stderr.clone(),
        tactic_goals: vec![],
        error_message: if success {
            None
        } else if !result.status.success() {
            Some(format!(
                "Process exited with status {} — stderr: {}",
                result.status, stderr.trim()
            ))
        } else {
            Some(format!(
                "Output mismatch — expected: {:?}, got: {:?}",
                expected_output.trim(),
                stdout.trim()
            ))
        },
    })
}
/// Writes `code` to an isolated temp file, executes via language interpreter, diffs stdout against `expected_output`.

pub async fn verify(
    engine: &VerificationEngine,
    backend: VerifierBackend,
    payload: &str,
) -> Result<VerificationResult, VerifyError> {
    match backend {
        VerifierBackend::Lean4 => run_lean4(engine, payload).await,
        VerifierBackend::Z3 => run_z3(engine, payload).await,
        VerifierBackend::Sandbox {
            language,
            expected_output,
            stdin_input,
        } => run_sandbox(engine, payload, &language, &expected_output, stdin_input.as_deref()).await,
    }
}
// Dispatches `payload` to the selected `VerifierBackend` and returns a unified `VerificationResult`.
