# AMTD Controller — Autonomous Moving Target Defense for Kubernetes

AMTD Controller is a Rust-native, production-grade Kubernetes control plane designed for **Autonomous Moving Target Defense (AMTD)**. It systematically eliminates persistent container attack surfaces by rotating autonomous agent pods on configurable epoch boundaries. When a pod is terminated, its execution DAG and semantic context are securely signed, serialized, and handed off to a warm standby promoted to successor.

The framework features an integrated **Multi-Backend Formal Verification Engine** (Lean 4, Z3, isolated code sandboxes) and a dynamic **Steering Ground Vector Ledger** that enforces immutable task trajectory alignment via cosine similarity matching and real-time pod rotation.

---

## Key Capabilities

*   **Stateless Agent Rotation:** Pre-schedules warm standby pods under strict anti-affinity rules, promoting them to active instantly upon epoch boundary deletion (SIGTERM).
*   **Tamper-Proof State Hand-off:** Signs serialized state payloads with SHA-256 HMAC and cryptographically signs validated alignment trajectories with Ed25519.
*   **Steering Ground Vector Ledger:** Manages an immutable list of steering constraints. Vectors can have an `immediate` rotation policy to instantly rotate pods when new steering guidelines are injected.
*   **Multi-Backend Formal Verification Engine:** programmatically verifies logic and code via Lean 4, Z3, or isolated, multi-language differential sandboxes (Python, Bash, Node.js, Ruby).
*   **Durable Audit Trail:** Persists all formal verification attempts (including failed theories and incorrect findings) and permanently commits correct facts to a deduped fact graph.
*   **Dual-Mode CLI:** Runs as a server daemon or acts as a client client querying running controllers (with offline JSON file fallback).

---

## Core Architecture & Pipeline

```
                              ┌──────────────────────────────────────────────┐
                              │           amtd-controller Daemon             │
                              │  ┌──────────────┐    ┌────────────────────┐  │
                              │  │  Epoch Loop  │    │   Axum HTTP API    │  │
                              │  │  (rotation)  │    │  (Ingest/Verify)   │  │
                              │  └──────┬───────┘    └─────────┬──────────┘  │
                              └─────────┼──────────────────────┼─────────────┘
                     kube API Delete    │                      │ RPC / verify
                                        ▼                      ▼
┌───────────────────────────────────────┴────────┐   ┌───────────────────────┐
│              Active Agent Pod                  │   │   Standby Warm Pod    │
│  ┌─────────┐   ┌────────────────────────────┐  │   │  (anti-affinity)      │
│  │  Agent  ├───► Python/Binary Shim (Trap)  │  │   │                       │
│  └─────────┘   └─────────────┬──────────────┘  │   │                       │
└──────────────────────────────┼─────────────────┘   └─────────▲─────────────┘
                               │ POST /state/ingest            │ Promotion
                               ▼                               │ (injected payload)
                  [State Alignment Validation] ────────────────┘
```

1.  **Epoch Boundary:** The controller's loop triggers a pod rotation, issuing a `Delete` command to the active agent pod.
2.  **Signal Trapping:** The pod's execution shim catches `SIGTERM`, pauses processing, collects the execution DAG and semantic findings, hashes and signs the payload, and sends it to `/state/ingest`.
3.  **State Alignment Gate:** The controller computes a cosine similarity score between the agent's semantic findings and the primary immutable objective, factoring in the steering weights of the active ledger. If aligned, the state is cryptographically signed with Ed25519.
4.  **Promotion:** The warm pod is immediately promoted to active status with the aligned state injected as `INITIAL_STATE`.
5.  **Replenishment:** A fresh standby pod is provisioned to replenish the warm pool.

---

## Quick Start (Local Out-of-Cluster)

To test the controller without deploying it inside a Kubernetes cluster, you can run the binary locally. It will utilize your local `kubeconfig` to execute pod operations in your target namespace.

### Prerequisites
*   Rust toolchain (Cargo, stable channel)
*   Access to a Kubernetes cluster (e.g. `minikube`, `kind`, or remote)
*   Namespace named `amtd` created: `kubectl create namespace amtd`
*   (Optional) `z3` or `lean` binaries installed locally for local verification testing

### 1. Run the Controller Daemon
Start the controller locally in daemon mode:
```bash
cargo run --bin controller -- \
  --namespace amtd \
  --warm-pool-size 1 \
  --epoch 300 \
  --llm-key "my-signing-secret"
```
The HTTP API will bind to `http://127.0.0.1:8080` and the daemon will automatically provision a warm standby pod in the `amtd` namespace.

### 2. Verify with the CLI
In another terminal, use the CLI client to verify the system status:
```bash
# List all active steering ground vectors
cargo run --bin controller -- list-vectors

# List the audit trail of formal verification attempts
cargo run --bin controller -- list-history
```

---

## Command-Line Interface (CLI) Reference

The `controller` binary acts as both the control plane server and the administrative client. All query and manipulation commands automatically attempt to query the running daemon over HTTP; if the daemon is offline, they gracefully fall back to reading and writing the JSON storage files directly.

### Global Options
These options apply to both server startup and client query commands:
*   `--port <PORT>`: The controller HTTP API port (Default: `8080` / env: `CONTROLLER_PORT`).
*   `--fact-graph-path <PATH>`: Path to the fact graph JSON database (Default: `fact_graph.json` / env: `FACT_GRAPH_PATH`).
*   `--ledger-path <PATH>`: Path to the steering ledger JSON database (Default: `ledger.json` / env: `LEDGER_PATH`).

---

### Commands

#### 1. `run` (Default)
Starts the AMTD controller server daemon, starts the warm pod provisioner, and runs the epoch rotation loop.
```bash
cargo run --bin controller -- run --epoch 300 --warm-pool-size 2
```

#### 2. `query-facts`
Queries permanently committed, formally verified facts.
*   `--tag <TAG>`: Filter facts by tag.
*   `--backend <BACKEND>`: Filter by verifier backend (`lean4`, `z3`, `sandbox-python3`, etc.).
*   `--epoch <EPOCH>`: Filter by the agent epoch number in which the fact was committed.
```bash
cargo run --bin controller -- query-facts --backend z3 --epoch 3
```

#### 3. `list-history`
Lists the audit trail of all theories and findings sent to the formal verification engine (regardless of success or failure).
*   `--backend <BACKEND>`: Filter by verifier backend.
*   `--status <STATUS>`: Filter by status (`passed`, `failed`, or `all`).
*   `--epoch <EPOCH>`: Filter by agent epoch.
```bash
cargo run --bin controller -- list-history --status failed
```

#### 4. `search`
Searches for verification history, theories, and details matching a payload proof hash.
```bash
cargo run --bin controller -- search 6a2f9b8c7d...
```

#### 5. `list-vectors`
Lists all steering ground vectors inside the active alignment ledger.
```bash
cargo run --bin controller -- list-vectors
```

#### 6. `add-vector`
Injects a new steering vector into the ledger.
*   `--label <LABEL>`: Short label for the vector.
*   `--content <CONTENT>`: Contextual text guide representing steering instructions.
*   `--weight <WEIGHT>`: Steering factor weight (Default: `1.0`).
*   `--policy <POLICY>`: The rotation policy (`immediate` to trigger instant pod rotation, or `next_epoch` to wait for the standard rotation boundary).
```bash
cargo run --bin controller -- add-vector \
  --label "Memory Isolation" \
  --content "Prevent raw pointer access and favor rust-allocated variables" \
  --weight 1.5 \
  --policy immediate
```

#### 7. `neutralize`
Soft-deletes a steering ground vector in the ledger, preventing it from steering future alignment gates.
```bash
cargo run --bin controller -- neutralize <VECTOR_ID>
```

---

## HTTP API Reference

The AMTD controller exposes a high-performance REST API. In-cluster agents and shims utilize this API to submit states and programmatically verify findings.

| Method | Endpoint | Description |
|---|---|---|
| `POST` | `/state/ingest` | Serializes and ingests state from a terminating agent. Requires valid HMAC signature. |
| `POST` | `/api/v1/verify` | Multi-backend programmatic verification. Logs attempt and commits successes. |
| `GET` | `/api/v1/facts` | Returns all permanently committed, verified facts. |
| `GET` | `/api/v1/history` | Returns the history of all verification attempts (pass and fail). Filterable. |
| `GET` | `/api/v1/history/search/:hash` | Returns attempts matching a payload proof hash. |
| `GET` | `/api/v1/ledger/vectors` | Lists all steering ground vectors. |
| `POST` | `/api/v1/ledger/vectors` | Injects a new steering vector. Supports immediate rotation triggering. |
| `POST` | `/api/v1/ledger/vectors/:id/neutralize` | Soft-deletes a steering vector. |
| `GET` | `/healthz` | Kubernetes liveness probe endpoint. |

---

## In-Pod Agent Integration

For an agent pod to participate in the AMTD architecture, its container entrypoint must run through the signal-trapping shim.

### Python Shim Helper
A production-ready Python helper `shim.py` is included in [shim/shim.py](file:///Users/mozer/Documents/projects/researcher-agent-kube-environment/shim/shim.py). It:
1.  Loads the injected `INITIAL_STATE` env var upon startup.
2.  Provides standard SIGTERM/SIGINT signal trapping to gracefully halt tasks.
3.  Exposes `call_verify()` to interact with the controller's formal verification engine.
4.  Serializes the final state and posts it to the controller `/state/ingest` endpoint on termination.

#### Integration Code Example:
```python
from shim import initialize, update_dag, update_semantic, call_verify

# Initialize shim and load parent state
state = initialize()
print(f"Running agent in Epoch {state.dag.epoch}")

# Run agent loops and update state metadata
update_dag("node-2", "verification", "Running Z3 verification loop", "Active")

# Perform a verification RPC call
result = call_verify(
    backend="z3",
    payload="(declare-const a Int) (assert (> a 10)) (check-sat)"
)
if result["success"]:
    print(f"Theory verified! Hash: {result['fact_hash']}")
    update_semantic(
        summary="Verified integer constraint bounds",
        key_findings=[f"Integer a bound successfully: {result['fact_hash']}"]
    )
```

---

## Configuration Reference

Parameters can be injected via CLI flags or standard Environment Variables:

| CLI Option | Environment Variable | Default | Description |
|---|---|---|---|
| `--epoch` | `EPOCH_DURATION_SEC` | `300` | Rotation interval in seconds |
| `--llm-key` | `LLM_API_KEY` | `""` | Key used for HMAC state signatures |
| `--namespace` | `TARGET_NAMESPACE` | `"amtd"` | Kubernetes namespace for agent pods |
| `--agent-image` | `AGENT_IMAGE` | `"amtd-agent:latest"` | Container image of the promoted agent |
| `--warm-pool-size` | `WARM_POOL_SIZE` | `2` | Number of standby warm pods in pool |
| `--port` | `CONTROLLER_PORT` | `8080` | HTTP Server Port |
| `--verify-timeout` | `VERIFY_TIMEOUT_SEC` | `30` | Subprocess execution timeout |
| `--verify-work-dir` | `VERIFY_WORK_DIR` | `"/tmp/amtd-verify"` | Temporary sandbox folder |
| `--alignment-objective`| `ALIGNMENT_OBJECTIVE`| `"Autonomous research"` | Primary steering objective |
| `--alignment-threshold`| `ALIGNMENT_THRESHOLD`| `0.0` | Cosine similarity pass score threshold |

---

## Security Design

*   **Signature Enforcement:** The controller signs state payloads using SHA-256 HMAC utilizing the `LLM_API_KEY` secret. On promotion, the successor pod verifies the signature, preventing man-in-the-middle or state-hijacking attacks.
*   **Sandbox Isolation:** The verification engine runs scripts in separate subprocesses with highly restricted standard paths. In production environments, it is recommended to bind mount empty directories or run the controller inside an unprivileged namespace with restricted security context constraints.
*   **Decoupled RBAC:** The controller operates with minimal cluster capabilities, requiring only namespace-scoped pod management permissions.
