#!/usr/bin/env python3
"""
RAKET Agent SIGTERM Shim
Intercepts SIGTERM, serializes structural DAG + semantic state,
and POSTs signed payload to the external controller before container exit.
"""
import signal
import sys
import os
import json
import hashlib
import threading
import datetime
import requests

CONTROLLER_ENDPOINT = os.environ.get("CONTROLLER_ENDPOINT", "http://raket-controller:8080")
POD_NAME = os.environ.get("POD_NAME", "unknown-pod")
POD_NAMESPACE = os.environ.get("POD_NAMESPACE", "raket")
LLM_API_KEY = os.environ.get("LLM_API_KEY", "")
AGENT_ID = os.environ.get("AGENT_ID", "")
TRANSMIT_TIMEOUT = int(os.environ.get("SHIM_TRANSMIT_TIMEOUT", "10"))

if not AGENT_ID:
    import uuid
    AGENT_ID = str(uuid.uuid4())

_state_lock = threading.Lock()

_dag_state = {
    "nodes": [],
    "current_task_pointer": None,
    "epoch": 0,
}

_semantic_state = {
    "sanitized_summary": "",
    "key_findings": [],
    "scratchpad_tokens": [],
    "source_urls": [],
}

def load_initial_state():
    raw = os.environ.get("INITIAL_STATE")
    if not raw:
        return
    try:
        parsed = json.loads(raw)
        global _dag_state, _semantic_state
        if "dag" in parsed:
            _dag_state.update(parsed["dag"])
        if "semantic" in parsed:
            _semantic_state.update(parsed["semantic"])
        print(f"[shim] Rehydrated state from INITIAL_STATE (epoch={_dag_state.get('epoch', 0)})", flush=True)
    except Exception as e:
        print(f"[shim] Failed to parse INITIAL_STATE: {e}", file=sys.stderr, flush=True)


def update_dag(nodes, pointer, epoch):
    """External hook for the agent to update structural DAG state."""
    with _state_lock:
        _dag_state["nodes"] = nodes
        _dag_state["current_task_pointer"] = pointer
        _dag_state["epoch"] = epoch


def update_semantic(summary, findings, tokens, urls):
    """External hook for the agent to update semantic context state."""
    with _state_lock:
        _semantic_state["sanitized_summary"] = summary
        _semantic_state["key_findings"] = findings
        _semantic_state["scratchpad_tokens"] = tokens
        _semantic_state["source_urls"] = urls


def _compute_signature(agent_id, pod_name, epoch, secret):
    payload = f"{agent_id}:{pod_name}:{epoch}:{secret}"
    return hashlib.sha256(payload.encode()).hexdigest()


def _serialize_state():
    with _state_lock:
        dag = dict(_dag_state)
        semantic = dict(_semantic_state)

    epoch = dag.get("epoch", 0)
    sig = _compute_signature(AGENT_ID, POD_NAME, epoch, LLM_API_KEY)

    return {
        "agent_id": AGENT_ID,
        "pod_name": POD_NAME,
        "namespace": POD_NAMESPACE,
        "captured_at": datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%S.%f") + "Z",
        "dag": dag,
        "semantic": semantic,
        "signature": sig,
    }


def _transmit_state():
    payload = _serialize_state()
    url = f"{CONTROLLER_ENDPOINT}/state/ingest"
    try:
        resp = requests.post(url, json=payload, timeout=TRANSMIT_TIMEOUT)
        resp.raise_for_status()
        print(
            f"[shim] State transmitted to {url} (epoch={payload['dag']['epoch']}, status={resp.status_code})",
            flush=True,
        )
    except requests.exceptions.RequestException as e:
        print(f"[shim] CRITICAL: State transmission failed: {e}", file=sys.stderr, flush=True)


def _sigterm_handler(signum, frame):
    print("[shim] SIGTERM received — serializing and transmitting state", flush=True)
    _transmit_state()
    sys.exit(0)


def _sigint_handler(signum, frame):
    print("[shim] SIGINT received — serializing and transmitting state", flush=True)
    _transmit_state()
    sys.exit(0)


def call_verify(backend: str, payload: str, **kwargs) -> dict:
    """
    Agent-callable: sends a verification request to the controller.
    Returns the full VerifyApiResponse dict for self-correction loops.
    """
    body = {"backend": backend, "payload": payload, **kwargs}
    url = f"{CONTROLLER_ENDPOINT}/api/v1/verify"
    try:
        resp = requests.post(url, json=body, timeout=TRANSMIT_TIMEOUT)
        resp.raise_for_status()
        return resp.json()
    except Exception as e:
        return {"success": False, "error_message": str(e)}


def initialize():
    """Call at agent startup to register signal handlers and load state."""
    load_initial_state()
    signal.signal(signal.SIGTERM, _sigterm_handler)
    signal.signal(signal.SIGINT, _sigint_handler)
    print(
        f"[shim] Initialized — pod={POD_NAME} agent_id={AGENT_ID} epoch={_dag_state['epoch']}",
        flush=True,
    )


if __name__ == "__main__":
    initialize()
    print("[shim] Running as standalone — waiting for signal", flush=True)
    signal.pause()
