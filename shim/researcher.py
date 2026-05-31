#!/usr/bin/env python3
import time
import sys
import os
import json
import requests
import shim

RESEARCH_TOPIC = "Additive Number Theory: Partition Function Identities and Ramanujan-Like Congruences"

def query_gemini(prompt: str, api_key: str) -> str:
    # Target the Gemini 3.1 Pro model with a high thinking level
    for model in ["gemini-3.1-pro-preview", "gemini-2.5-pro"]:
        url = f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
        headers = {"Content-Type": "application/json"}
        body = {
            "contents": [{
                "parts": [{"text": prompt}]
            }],
            "generationConfig": {
                "responseMimeType": "application/json",
                "thinkingConfig": {
                    "thinkingBudget": 4096
                }
            }
        }
        try:
            print(f"[researcher] Querying model {model} (ThinkingLevel = HIGH)...", flush=True)
            resp = requests.post(url, headers=headers, json=body, timeout=90)
            if resp.status_code == 200:
                data = resp.json()
                return data["candidates"][0]["content"]["parts"][0]["text"]
            else:
                print(f"[researcher] Model {model} returned status {resp.status_code}: {resp.text}", flush=True)
        except Exception as e:
            print(f"[researcher] Query to {model} failed: {e}", flush=True)
    return ""

def sanitize_context_text(text: str) -> str:
    # Active runtime sanitization: filters out draft tokens, debug tags, or private notes
    leakage_keywords = ["TODO", "DEBUG", "DRAFT", "TEMP_NOTE", "SCRATCHPAD", "INTERNAL_RECON"]
    cleaned = text
    for kw in leakage_keywords:
        cleaned = cleaned.replace(kw, "")
        cleaned = cleaned.replace(kw.lower(), "")
    return cleaned.strip()

def main():
    shim.initialize()
    api_key = shim.LLM_API_KEY
    if not api_key:
        print("[researcher] ERROR: LLM_API_KEY environment variable is missing", file=sys.stderr, flush=True)
        sys.exit(1)

    epoch = shim._dag_state.get("epoch", 0)
    print(f"[researcher] Starting research on topic: '{RESEARCH_TOPIC}' | Epoch: {epoch} | Model: gemini-3.1-pro", flush=True)

    # Rehydrate from saved state
    raw_findings = list(shim._semantic_state.get("key_findings", []))
    findings = [sanitize_context_text(f) for f in raw_findings]
    summary = sanitize_context_text(shim._semantic_state.get("sanitized_summary", "Initial mathematical research starting."))
    
    loop_count = 0
    while True:
        loop_count += 1
        print(f"\n[researcher] --- Loop iteration {loop_count} ---", flush=True)
        
        prompt = f"""
        We are researching: "{RESEARCH_TOPIC}".
        We are currently in Epoch {epoch} of the Kubernetes agent lifecycle.
        Here is the current sanitized research summary: "{summary}"
        Here are the previous key findings: {json.dumps(findings)}

        Please analyze a novel mathematical concept or check for unique relations in additive number theory, specifically partition function p(n) values, Ramanujan-like congruences, or other modular arithmetic behaviors.
        You must decide whether you have formulated a new, verified, or interesting conjecture/insight in this step.
        Only call the validation sandbox if you think you discovered something new or formulated a proposition that requires programmatic validation.
        
        Please output your response strictly in the following JSON format:
        {{
          "has_new_discovery": true,
          "finding": "Your detailed mathematical insight or concept analyzed in this step (avoid private notes, TODOs, or draft keywords)",
          "python_code": "Your valid Python 3 script verifying the math if has_new_discovery is true, otherwise an empty string. If present, the script MUST output ONLY the string 'VERIFIED_OK' to stdout and absolutely nothing else.",
          "summary_addition": "A brief addition to the sanitized research summary"
        }}
        """

        raw_resp = query_gemini(prompt, api_key)
        if not raw_resp:
            print("[researcher] Failed to get response from Gemini Pro. Waiting to retry...", flush=True)
            time.sleep(15)
            continue

        try:
            parsed = json.loads(raw_resp)
            has_new_discovery = parsed.get("has_new_discovery", False)
            new_finding = parsed.get("finding", "")
            python_code = parsed.get("python_code", "")
            summary_addition = parsed.get("summary_addition", "")
        except Exception as e:
            print(f"[researcher] Failed to parse Gemini JSON response: {e}. Raw: {raw_resp}", flush=True)
            time.sleep(15)
            continue

        # Sanitize Gemini inputs to maintain strict context hygiene
        new_finding = sanitize_context_text(new_finding)
        python_code = python_code.strip()
        summary_addition = sanitize_context_text(summary_addition)

        print(f"[researcher] Has New Discovery: {has_new_discovery}", flush=True)
        print(f"[researcher] Finding details: {new_finding}", flush=True)

        success = False
        fact_hash = None

        if has_new_discovery and python_code:
            print(f"[researcher] Generated Python Verification Code:\n{python_code}", flush=True)
            print("[researcher] Submitting proof to RAKET Python sandbox verification engine...", flush=True)
            
            verify_result = shim.call_verify(
                backend="sandbox",
                payload=python_code,
                sandbox_language="python3",
                sandbox_expected_output="VERIFIED_OK",
                fact_tags=["raket", "math-proof"],
                epoch=epoch
            )

            success = verify_result.get("success", False)
            fact_hash = verify_result.get("fact_hash")
            
            if success:
                print(f"[researcher] SUCCESS: Proof formally verified! committed fact hash: {fact_hash}", flush=True)
                findings.append(f"Verified Finding in Epoch {epoch}: {new_finding} (hash: {fact_hash})")
                summary = f"{summary.strip()} {summary_addition.strip()}"
            else:
                err = verify_result.get("error_message", "Unknown verification failure")
                print(f"[researcher] FAILED verification: {err}", flush=True)
                findings.append(f"Failed verification attempt in Epoch {epoch}: {new_finding} (Error: {err})")
        else:
            print("[researcher] Decided not to call verification. No new mathematical conjecture claimed in this step.", flush=True)
            findings.append(f"Explored Concept in Epoch {epoch}: {new_finding}")
            summary = f"{summary.strip()} {summary_addition.strip()}"

        # Update RAKET state hooks
        nodes = list(shim._dag_state.get("nodes", []))
        node_id = f"node-epoch-{epoch}-loop-{loop_count}"
        nodes.append({
            "id": node_id,
            "task_type": "mathematical_exploration",
            "dependencies": [],
            "status": "Completed" if (not has_new_discovery or success) else "Failed",
            "metadata": {
                "finding": new_finding,
                "has_new_discovery": str(has_new_discovery),
                "fact_hash": fact_hash,
                "success": str(success)
            }
        })
        
        # Sanitize and submit state to controller
        sanitized_summary = sanitize_context_text(summary)
        sanitized_findings = [sanitize_context_text(f) for f in findings]
        
        shim.update_dag(nodes, node_id, epoch)
        shim.update_semantic(
            sanitized_summary, 
            sanitized_findings, 
            ["raket", "math", "gemini-pro"], 
            [f"https://github.com/JanosMozer/researcher-agent-kube-environment"]
        )

        # Sleep and iterate
        print("[researcher] Sleeping for 30 seconds before next loop...", flush=True)
        time.sleep(30)

if __name__ == "__main__":
    main()
