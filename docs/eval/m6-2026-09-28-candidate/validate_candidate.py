#!/usr/bin/env python3
"""Deterministically validate the static M6 candidate's source/task fences."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
EXPECTED_COMMIT = "e046ceb59393db9a4977ba55899eba301ad28005"


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main() -> None:
    candidate = json.loads((HERE / "candidate.json").read_text(encoding="utf-8"))
    assert git("rev-parse", "HEAD") == EXPECTED_COMMIT == candidate["repository_commit"]

    sources = {source["logical_id"]: source for source in candidate["sources"]}
    assert len(sources) == 30
    assert len({source["path"] for source in candidate["sources"]}) == 30
    assert sum(source["split"] == "development" for source in sources.values()) == 15
    assert sum(source["split"] == "holdout" for source in sources.values()) == 15

    for source in sources.values():
        path = source["path"]
        assert path.startswith("docs/adr/") and path.endswith(".md")
        content = subprocess.check_output(
            ["git", "show", f"{candidate['repository_commit']}:{path}"], cwd=ROOT
        )
        blob_oid = git("rev-parse", f"{candidate['repository_commit']}:{path}")
        assert blob_oid == source["git_blob_oid"]
        assert hashlib.sha256(content).hexdigest() == source["content_sha256"]
        assert len(content.decode("utf-8").splitlines()) == source["line_count"]

    tasks = candidate["tasks"]
    assert len(tasks) == 60 and len({task["id"] for task in tasks}) == 60
    query_refs = [ref for task in tasks for ref in task["query_refs"]]
    assert len(query_refs) == 60 and len(set(query_refs)) == 60
    assert sum(task["split"] == "development" for task in tasks) == 30
    assert sum(task["split"] == "holdout" for task in tasks) == 30
    assert sum(task["severity"] in {"high", "critical"} for task in tasks) >= 20
    assert sum(task["expected_no_answer"] for task in tasks) >= 10

    b_sources = set(candidate["b_source_ids"])
    assert len(b_sources) == 8 and b_sources <= set(sources)
    coverage = dict.fromkeys(sources, 0)
    for task in tasks:
        assert task["candidate_review_state"] == "pending-independent-agent-review"
        assert task["must_preserve"] and task["must_not_infer"]
        assert task["expected_status"] and task["evidence_refs"]
        assert task["expected_usable_information"] or task["expected_no_answer"]

        for source_id in task["source_ids"]:
            assert source_id in sources
            assert sources[source_id]["split"] == task["split"]
            coverage[source_id] += 1
        if task["split"] == "holdout":
            assert sum(source_id in b_sources for source_id in task["source_ids"]) == 1

        for relation in task["expected_source_relations"]:
            assert relation["source_ids"]
            assert set(relation["source_ids"]) <= set(task["source_ids"])
        for evidence in task["evidence_refs"]:
            source = sources[evidence["source_id"]]
            assert evidence["path"] == source["path"]
            assert 1 <= evidence["line_start"] <= evidence["line_end"] <= source["line_count"]
            if evidence["basis"] == "full_document_absence_candidate":
                assert evidence["line_start"] == 1
                assert evidence["line_end"] == source["line_count"]

    assert all(count > 0 for count in coverage.values())
    assert candidate["candidate_status"] == "candidate_pending_independent_agent_review"
    assert candidate["prepare_cli_ready"] is False
    assert candidate["original_live_dataset_recovered"] is False
    forbidden_keys = {
        "run_root",
        "master_key_path",
        "provider_secret",
        "api_key",
        "seal",
        "claim",
        "live-config.json",
    }
    assert forbidden_keys.isdisjoint(candidate)

    holdout_tasks = [task for task in tasks if task["split"] == "holdout"]
    holdout_sources = {source_id for task in holdout_tasks for source_id in task["source_ids"]}
    print(
        json.dumps(
            {
                "status": "PASS",
                "sources": len(sources),
                "development_sources": 15,
                "holdout_sources": 15,
                "tasks": len(tasks),
                "development_tasks": 30,
                "holdout_tasks": 30,
                "holdout_no_answer": sum(task["expected_no_answer"] for task in holdout_tasks),
                "holdout_high_or_critical": sum(
                    task["severity"] in {"high", "critical"} for task in holdout_tasks
                ),
                "holdout_relation_tasks": sum(
                    bool(task["expected_source_relations"]) for task in holdout_tasks
                ),
                "holdout_sources_covered": len(holdout_sources),
                "b_sources": len(b_sources),
                "high_or_critical": sum(
                    task["severity"] in {"high", "critical"} for task in tasks
                ),
                "no_answer_candidates": sum(task["expected_no_answer"] for task in tasks),
                "source_blob_and_sha256_fences": "verified",
                "split_id_b_and_evidence_constraints": "verified",
                "provider_run_material": "absent",
            },
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
