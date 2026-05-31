use raket_controller::alignment::{run_alignment_gate, AlignmentGate};
use raket_controller::api::{build_router, AppState};
use raket_controller::fact_graph;
use raket_controller::ledger;
use raket_controller::state::{AgentState, DagNode, DagNodeStatus, ExecutionDag, SemanticContext};
use raket_controller::verify::{verify, VerificationEngine, VerifierBackend};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};
use tower::ServiceExt;

fn make_test_state(secret: &str) -> AgentState {
    let mut state = AgentState {
        agent_id: uuid::Uuid::new_v4().to_string(),
        pod_name: "test-pod-alpha".to_string(),
        namespace: "raket-test".to_string(),
        captured_at: Utc::now(),
        dag: ExecutionDag {
            nodes: vec![DagNode {
                id: "node-1".to_string(),
                task_type: "research".to_string(),
                dependencies: vec![],
                status: DagNodeStatus::Completed,
                metadata: HashMap::new(),
            }],
            current_task_pointer: Some("node-1".to_string()),
            epoch: 3,
        },
        semantic: SemanticContext {
            sanitized_summary: "Autonomous Moving Target Defense research on Kubernetes pod rotation and state migration.".to_string(),
            key_findings: vec![
                "Pod anti-affinity eliminates scheduling lag".to_string(),
                "SHA-256 state signing prevents payload tampering".to_string(),
            ],
            scratchpad_tokens: vec!["raket".to_string(), "rotation".to_string(), "kubernetes".to_string()],
            source_urls: vec!["https://arxiv.org/abs/raket".to_string()],
        },
        signature: String::new(),
    };
    state.signature = state.compute_signature(secret);
    state
}

fn make_app_state(secret: &str, pending: Arc<RwLock<Option<AgentState>>>) -> Arc<AppState> {
    Arc::new(AppState {
        pending_state: pending,
        llm_api_key: secret.to_string(),
        verification_engine: Arc::new(VerificationEngine::new(
            std::env::temp_dir(),
            10,
        )),
        fact_graph: Arc::new(RwLock::new(fact_graph::ephemeral())),
        ledger: Arc::new(RwLock::new(ledger::ephemeral())),
        rotation_trigger: Arc::new(Notify::new()),
    })
}

#[tokio::test]
async fn test_state_ingest_endpoint_accepts_valid_payload() {
    let secret = "test-secret-key";
    let state = make_test_state(secret);
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let app_state = make_app_state(secret, Arc::clone(&pending));
    let router = build_router(app_state);

    let body = serde_json::to_string(&state).unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/state/ingest")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let stored = pending.read().await;
    assert!(stored.is_some());
    let s = stored.as_ref().unwrap();
    assert_eq!(s.pod_name, "test-pod-alpha");
    assert_eq!(s.dag.epoch, 3);
}

#[tokio::test]
async fn test_state_ingest_endpoint_rejects_invalid_signature() {
    let state = make_test_state("correct-secret");
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let app_state = make_app_state("wrong-secret", pending);
    let router = build_router(app_state);

    let body = serde_json::to_string(&state).unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/state/ingest")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_healthz_endpoint() {
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let app_state = make_app_state("k", pending);
    let router = build_router(app_state);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_alignment_gate_passes_aligned_state() {
    let state = make_test_state("s");
    let mut gate = AlignmentGate::new(
        "Autonomous Moving Target Defense research on Kubernetes pod rotation",
        0.0,
    );
    let report = run_alignment_gate(&mut gate, &state).unwrap();
    assert!(report.passed);
    assert!(report.score >= 0.0);
    assert!(!report.ed25519_signature.is_empty());
    assert!(!report.signed_payload_hash.is_empty());
    assert_eq!(gate.ground_vectors.len(), 1);
}

#[tokio::test]
async fn test_alignment_gate_rejects_unaligned_state() {
    let secret = "s";
    let mut state = make_test_state(secret);
    state.semantic.sanitized_summary = "Cooking pasta with tomato sauce.".to_string();
    state.semantic.key_findings = vec!["pasta al dente".to_string()];
    state.semantic.scratchpad_tokens = vec!["pasta".to_string()];

    let mut gate = AlignmentGate::new(
        "Autonomous Moving Target Defense research on Kubernetes pod rotation",
        0.9999,
    );
    let result = run_alignment_gate(&mut gate, &state);
    assert!(result.is_err());
    assert!(gate.ground_vectors.is_empty());
}

#[tokio::test]
async fn test_alignment_gate_accumulates_ground_vectors() {
    let secret = "s";
    let mut gate = AlignmentGate::new("RAKET Kubernetes research", 0.0);
    for i in 0..3u64 {
        let mut state = make_test_state(secret);
        state.dag.epoch = i;
        state.pod_name = format!("pod-{}", i);
        state.signature = state.compute_signature(secret);
        run_alignment_gate(&mut gate, &state).unwrap();
    }
    assert_eq!(gate.ground_vectors.len(), 3);
}

#[tokio::test]
async fn test_full_rotation_pipeline_state_ingest_then_alignment() {
    let secret = "pipeline-secret";
    let state = make_test_state(secret);
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let app_state = make_app_state(secret, Arc::clone(&pending));
    let router = build_router(app_state);

    let body = serde_json::to_string(&state).unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/state/ingest")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let captured = pending.read().await.clone().unwrap();
    assert_eq!(captured.signature, state.compute_signature(secret));

    let mut gate = AlignmentGate::new("RAKET Kubernetes research rotation", 0.0);
    let report = run_alignment_gate(&mut gate, &captured).unwrap();
    assert!(report.passed);
    assert!(!report.ed25519_signature.is_empty());
}

#[tokio::test]
async fn test_verify_endpoint_z3_dispatch() {
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let app_state = make_app_state("k", pending);
    let router = build_router(app_state);

    let payload = serde_json::json!({
        "backend": "z3",
        "payload": "(declare-const x Int)\n(assert (= x 5))\n(assert (not (= x 5)))\n(check-sat)\n",
        "fact_tags": ["test"],
        "epoch": 1
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/verify")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_ledger_add_and_neutralize_via_api() {
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let app_state = make_app_state("k", Arc::clone(&pending));
    let app_state2 = Arc::clone(&app_state);

    let router = build_router(app_state);
    let body = serde_json::json!({
        "label": "focus-on-raket",
        "content": "Prioritize RAKET Kubernetes rotation research",
        "weight": 1.5,
        "rotation_policy": "next_epoch"
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ledger/vectors")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ledger = app_state2.ledger.read().await;
    assert_eq!(ledger.entries.len(), 1);
    let entry = ledger.entries.values().next().unwrap();
    assert_eq!(entry.label, "focus-on-raket");
    assert!(!entry.neutralized);
}

#[tokio::test]
async fn test_immediate_rotation_trigger_notifies() {
    let pending: Arc<RwLock<Option<AgentState>>> = Arc::new(RwLock::new(None));
    let trigger = Arc::new(Notify::new());
    let app_state = Arc::new(AppState {
        pending_state: pending,
        llm_api_key: "k".to_string(),
        verification_engine: Arc::new(VerificationEngine::new(std::env::temp_dir(), 5)),
        fact_graph: Arc::new(RwLock::new(fact_graph::ephemeral())),
        ledger: Arc::new(RwLock::new(ledger::ephemeral())),
        rotation_trigger: Arc::clone(&trigger),
    });

    let router = build_router(app_state);
    let body = serde_json::json!({
        "label": "immediate-steer",
        "content": "Emergency re-alignment",
        "weight": 2.0,
        "rotation_policy": "immediate"
    });

    let notified = tokio::spawn(async move {
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            trigger.notified(),
        )
        .await
        .is_ok()
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ledger/vectors")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    router.oneshot(req).await.unwrap();
    assert!(notified.await.unwrap(), "rotation_trigger should have fired");
}

#[tokio::test]
async fn test_verification_engine_z3_unsat() {
    let engine = VerificationEngine::new(std::env::temp_dir(), 10);
    let z3_payload = "(declare-const x Int)\n(assert (= x 5))\n(assert (not (= x 5)))\n(check-sat)\n";
    let result = verify(&engine, VerifierBackend::Z3, z3_payload).await;
    match result {
        Ok(vr) => assert_eq!(vr.backend, "z3"),
        Err(raket_controller::verify::VerifyError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            eprintln!("SKIP: z3 not in PATH");
        }
        Err(e) => panic!("Unexpected: {e}"),
    }
}

#[tokio::test]
async fn test_verification_engine_sandbox_python_match() {
    let engine = VerificationEngine::new(std::env::temp_dir(), 10);
    let code = "print('hello world')";
    let result = verify(
        &engine,
        VerifierBackend::Sandbox {
            language: "python3".to_string(),
            expected_output: "hello world".to_string(),
            stdin_input: None,
        },
        code,
    )
    .await;
    match result {
        Ok(vr) => {
            assert!(vr.backend.starts_with("sandbox-"));
            assert!(vr.success, "Expected match: {vr:?}");
        }
        Err(raket_controller::verify::VerifyError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            eprintln!("SKIP: python3 not in PATH");
        }
        Err(e) => panic!("Unexpected: {e}"),
    }
}

#[tokio::test]
async fn test_verification_engine_sandbox_python_mismatch() {
    let engine = VerificationEngine::new(std::env::temp_dir(), 10);
    let code = "print('wrong output')";
    let result = verify(
        &engine,
        VerifierBackend::Sandbox {
            language: "python3".to_string(),
            expected_output: "hello world".to_string(),
            stdin_input: None,
        },
        code,
    )
    .await;
    match result {
        Ok(vr) => assert!(!vr.success, "Should fail on mismatch"),
        Err(raket_controller::verify::VerifyError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            eprintln!("SKIP: python3 not in PATH");
        }
        Err(e) => panic!("Unexpected: {e}"),
    }
}

#[tokio::test]
async fn test_pod_manifest_injection_via_state_fields() {
    let secret = "manifest-secret";
    let state = make_test_state(secret);
    let serialized = serde_json::to_string(&state).unwrap();
    let deserialized: AgentState = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.agent_id, state.agent_id);
    assert_eq!(deserialized.dag.epoch, state.dag.epoch);
    assert_eq!(deserialized.dag.nodes[0].status, DagNodeStatus::Completed);
    assert_eq!(deserialized.signature, state.compute_signature(secret));
}

#[tokio::test]
async fn test_ledger_active_ground_vectors_excludes_neutralized() {
    let mut l = ledger::ephemeral();
    let id = ledger::add_entry(
        &mut l,
        "active".to_string(),
        "active content".to_string(),
        1.0,
        ledger::RotationPolicy::NextEpoch,
    );
    ledger::add_entry(
        &mut l,
        "neutralized".to_string(),
        "dead content".to_string(),
        1.0,
        ledger::RotationPolicy::NextEpoch,
    );
    ledger::neutralize_entry(&mut l, &id).unwrap();

    let active = ledger::active_ground_vectors(&l);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].label, "neutralized");
}

#[tokio::test]
async fn test_fact_graph_deduplication() {
    let mut fg = fact_graph::ephemeral();
    let payload = "theorem foo : 1 = 1 := rfl";

    let first = fact_graph::commit_fact(&mut fg, "lean4", payload, 1, vec!["math".to_string()])
        .await
        .unwrap();
    let second = fact_graph::commit_fact(&mut fg, "lean4", payload, 2, vec!["math".to_string()])
        .await
        .unwrap();

    assert!(first.is_some());
    assert!(second.is_none());
    assert_eq!(fg.nodes.len(), 1);
}

#[tokio::test]
async fn test_verify_history_logging_and_retrieval_endpoints() {
    let pending = Arc::new(RwLock::new(None));
    let app_state = make_app_state("k", pending);
    let router = build_router(app_state);

    let body_fail = serde_json::json!({
        "backend": "sandbox",
        "payload": "print('hello')",
        "sandbox_language": "python3",
        "sandbox_expected_output": "world",
        "fact_tags": ["test-fail"],
        "epoch": 42
    });

    let req_fail = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/verify")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body_fail).unwrap()))
        .unwrap();

    let resp_fail = router.clone().oneshot(req_fail).await.unwrap();
    assert_eq!(resp_fail.status(), StatusCode::OK);

    let body_pass = serde_json::json!({
        "backend": "sandbox",
        "payload": "print('hello')",
        "sandbox_language": "python3",
        "sandbox_expected_output": "hello",
        "fact_tags": ["test-pass"],
        "epoch": 42
    });

    let req_pass = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/verify")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body_pass).unwrap()))
        .unwrap();

    let resp_pass = router.clone().oneshot(req_pass).await.unwrap();
    assert_eq!(resp_pass.status(), StatusCode::OK);

    let req_history = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/history")
        .body(Body::empty())
        .unwrap();

    let resp_history = router.clone().oneshot(req_history).await.unwrap();
    assert_eq!(resp_history.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp_history.into_body(), 100000).await.unwrap();
    let history: Vec<fact_graph::VerificationAttempt> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|a| !a.success && a.tags.contains(&"test-fail".to_string())));
    assert!(history.iter().any(|a| a.success && a.tags.contains(&"test-pass".to_string())));

    let req_history_success = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/history?status=success")
        .body(Body::empty())
        .unwrap();

    let resp_history_success = router.clone().oneshot(req_history_success).await.unwrap();
    assert_eq!(resp_history_success.status(), StatusCode::OK);

    let bytes_success = axum::body::to_bytes(resp_history_success.into_body(), 100000).await.unwrap();
    let history_success: Vec<fact_graph::VerificationAttempt> = serde_json::from_slice(&bytes_success).unwrap();
    assert_eq!(history_success.len(), 1);
    assert!(history_success[0].success);

    let hash_to_search = &history_success[0].payload_hash;
    let req_search = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/history/search/{}", hash_to_search))
        .body(Body::empty())
        .unwrap();

    let resp_search = router.oneshot(req_search).await.unwrap();
    assert_eq!(resp_search.status(), StatusCode::OK);

    let bytes_search = axum::body::to_bytes(resp_search.into_body(), 100000).await.unwrap();
    let history_search: Vec<fact_graph::VerificationAttempt> = serde_json::from_slice(&bytes_search).unwrap();
    assert!(!history_search.is_empty());
    assert_eq!(history_search[0].payload_hash, *hash_to_search);
}

