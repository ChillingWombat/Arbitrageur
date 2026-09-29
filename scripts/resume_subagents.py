#!/usr/bin/env python3
"""
Subagent State Checker & Resumption Tool.
Reads .agents/subagents_manifest.json and .agents/teamwork/orchestrator_2/
to report the live status of all active subagents, milestones completed,
verification gates, and instructions to revive them after an Antigravity app or server restart.
"""

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / ".agents" / "subagents_manifest.json"
ORCH_DIR = REPO_ROOT / ".agents" / "teamwork" / "orchestrator_2"

def load_manifest():
    if not MANIFEST_PATH.exists():
        print(f"Error: Manifest not found at {MANIFEST_PATH}", file=sys.stderr)
        sys.exit(1)
    with open(MANIFEST_PATH, "r", encoding="utf-8") as f:
        return json.load(f)

def check_progress():
    manifest = load_manifest()
    print("=" * 64)
    print(" Arbitrageur Subagents Recovery & Status Dashboard")
    print("=" * 64)
    print(f"Mission: {manifest.get('active_mission')}")
    print(f"Parent Conversation ID: {manifest.get('parent_conversation_id')}")
    print(f"Last Updated: {manifest.get('last_updated')}\n")

    print("--- Active Subagents ---")
    for agent in manifest.get("subagents", []):
        cid = agent.get("conversation_id")
        role = agent.get("role")
        status = agent.get("status")
        in_prog = ", ".join(agent.get("in_progress_milestones", [])) or "None"
        done = ", ".join(agent.get("completed_milestones", [])) or "None"
        print(f"• [{status.upper()}] {role} ({cid})")
        print(f"   In Progress: {in_prog}")
        print(f"   Completed:   {done}")
        print(f"   Revival Msg: {agent.get('revival_instruction')[:80]}...\n")

    print("--- Milestones Status ---")
    for m in manifest.get("milestones", []):
        st = m.get("status")
        symbol = "✓" if st == "completed" else "▶" if st == "in_progress" else "○"
        print(f"  {symbol} [{st.upper():^11}] {m.get('id')}: {m.get('title')}")

    if ORCH_DIR.exists():
        print("\n--- Live Verification Gate Status (Disk Check) ---")
        for verifier in ["reviewer_1", "reviewer_2", "challenger_1", "challenger_2", "auditor_1"]:
            v_dir = ORCH_DIR / verifier
            handoff = v_dir / "handoff.md"
            progress = v_dir / "progress.md"
            if handoff.exists():
                print(f"  ✓ {verifier:<15} : HANDOFF COMPLETE ({handoff.stat().st_size} bytes)")
            elif progress.exists():
                print(f"  ▶ {verifier:<15} : IN PROGRESS (progress.md active)")
            else:
                print(f"  ○ {verifier:<15} : PENDING")

    print("\n--- Recovery Protocol ---")
    for step in manifest.get("recovery_protocol", {}).get("on_restart_event", []):
        print(f"  {step}")
    print("=" * 64)

if __name__ == "__main__":
    check_progress()
