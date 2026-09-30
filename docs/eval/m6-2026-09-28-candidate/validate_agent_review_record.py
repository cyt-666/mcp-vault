#!/usr/bin/env python3
"""Fail-closed structural validation for the independent M6 review record."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import re
import subprocess
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = Path(__file__).resolve().parents[3]
CANDIDATE = HERE / "candidate.json"
PACKAGE = HERE / "holdout-independent-review.zh-CN.md"
SCHEMA = HERE / "post-run-agent-review.schema.json"
TEMPLATE = HERE / "post-run-agent-review.template.json"
EXPECTED_CANDIDATE_SHA256 = "bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2"
EXPECTED_PACKAGE_SHA256 = "8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a"
EXPECTED_COMMIT = "e046ceb59393db9a4977ba55899eba301ad28005"
FORBIDDEN_ARM_KEYS = {"arm", "arm_id", "arm_name", "comparison_arm", "arm_mapping", "mapping"}


def fail(message: str) -> None:
    raise SystemExit(message)


def check_evidence(item: dict, sources: dict[str, dict]) -> None:
    source = sources.get(item.get("source_id"))
    if source is None:
        fail("review evidence references a non-holdout source")
    if item.get("path") != source["path"]:
        fail("review evidence path does not match source manifest")
    if item.get("git_blob_oid") != source["git_blob_oid"] or item.get("content_sha256") != source["content_sha256"]:
        fail("review evidence source hash mismatch")
    line_start = item.get("line_start")
    line_end = item.get("line_end")
    if (
        isinstance(line_start, bool)
        or isinstance(line_end, bool)
        or not isinstance(line_start, int)
        or not isinstance(line_end, int)
        or not (1 <= line_start <= line_end <= source["line_count"])
    ):
        fail("review evidence line range is invalid")


def verify_source_manifest(candidate: dict) -> None:
    if candidate.get("repository_commit") != EXPECTED_COMMIT:
        fail("candidate repository commit is not the pinned source commit")
    for source in candidate.get("sources", []):
        if source.get("split") != "holdout":
            continue
        path = source.get("path")
        try:
            blob_oid = subprocess.check_output(
                ["git", "rev-parse", f"{EXPECTED_COMMIT}:{path}"], cwd=ROOT, text=True
            ).strip()
            content = subprocess.check_output(
                ["git", "show", f"{EXPECTED_COMMIT}:{path}"], cwd=ROOT
            )
        except (OSError, subprocess.CalledProcessError):
            fail("fixed candidate source blob could not be verified")
        if blob_oid != source.get("git_blob_oid"):
            fail("fixed candidate source Git blob OID mismatch")
        if hashlib.sha256(content).hexdigest() != source.get("content_sha256"):
            fail("fixed candidate source SHA-256 mismatch")
        try:
            line_count = len(content.decode("utf-8").splitlines())
        except UnicodeDecodeError:
            fail("fixed candidate source is not valid UTF-8")
        if line_count != source.get("line_count"):
            fail("fixed candidate source line count mismatch")


def validate_gold_review(review: dict, task: dict, sources: dict[str, dict], *, freeze: bool) -> None:
    field_decisions = review.get("field_decisions", {})
    required_decisions = {
        "query", "usable_information", "must_preserve", "must_not_infer",
        "status", "relations", "severity", "no_answer_scope",
    }
    if set(field_decisions) != required_decisions:
        fail("gold review is missing a required semantic decision field")

    for name, value in field_decisions.items():
        if not isinstance(value, dict):
            fail("gold review field decisions require evidence-based rationale")
        decision = value.get("decision")
        rationale = value.get("rationale")
        allowed = {"supported", "revise", "insufficient_evidence", "not_applicable"}
        if decision not in allowed or not isinstance(rationale, str) or not rationale.strip():
            fail("gold review field decisions require evidence-based rationale")
        if freeze and decision not in {"supported", "not_applicable"}:
            fail("gold freeze cannot contain unresolved or revised field decisions")

    if freeze:
        no_answer_decision = field_decisions["no_answer_scope"]["decision"]
        if task["expected_no_answer"]:
            if no_answer_decision != "supported":
                fail("no-answer gold scope must be supported before freeze")
        elif no_answer_decision != "not_applicable":
            fail("answerable task no-answer scope must be not_applicable")

    evidence_items = review.get("evidence", [])
    if not evidence_items:
        fail("gold review requires fixed-source evidence")
    reviewed_source_ids = set()
    audited_source_ids = set()
    task_source_ids = set(task["source_ids"])
    for evidence in evidence_items:
        check_evidence(evidence, sources)
        source_id = evidence["source_id"]
        if source_id not in task_source_ids:
            fail("gold review evidence references a source outside the fixed task")
        reviewed_source_ids.add(source_id)
        source = sources[source_id]
        if evidence["line_start"] == 1 and evidence["line_end"] == source["line_count"]:
            audited_source_ids.add(source_id)
    if not task_source_ids <= reviewed_source_ids:
        fail("gold review evidence does not cover every task source")
    if task["expected_no_answer"] and not task_source_ids <= audited_source_ids:
        fail("no-answer gold review lacks full-document audit evidence for every source")


def validate_gold_freeze(record: dict, holdout: dict[str, dict], sources: dict[str, dict]) -> None:
    gold_reviews = record.get("gold_reviews", [])
    if len(gold_reviews) != len(holdout) or {item.get("task_id") for item in gold_reviews} != set(holdout):
        fail("gold freeze must cover exactly all 30 holdout tasks")
    if any(item.get("decision") != "agent_reviewed" for item in gold_reviews):
        fail("gold freeze cannot contain unresolved or revised tasks")
    if record.get("quality_claim") != "not_evaluated":
        fail("pre-run gold freeze must keep quality_claim=not_evaluated")
    if record.get("human_review") is not False:
        fail("agent gold review must explicitly set human_review=false")
    producer = record.get("producer", {}).get("identity")
    reviewer = record.get("reviewer", {}).get("identity")
    if not reviewer or reviewer == "PENDING" or reviewer == producer:
        fail("gold reviewer identity must be distinct from producer")
    locked_at = record.get("gold_locked_at")
    if not isinstance(locked_at, str) or not locked_at.strip():
        fail("pre-run gold freeze requires gold_locked_at")
    try:
        parsed = dt.datetime.fromisoformat(locked_at.replace("Z", "+00:00"))
    except ValueError:
        fail("gold_locked_at must be an ISO-8601 timestamp")
    if parsed.tzinfo is None:
        fail("gold_locked_at must include a timezone")
    if record.get("artifact_scores") or record.get("blind_scores") or record.get("adjudications"):
        fail("pre-run gold freeze must not include post-run scores or adjudications")
    for review in gold_reviews:
        task_id = review.get("task_id")
        if task_id not in holdout:
            fail("gold review references a non-holdout task")
        validate_gold_review(review, holdout[task_id], sources, freeze=True)


def reject_arm_labels(value: object) -> None:
    if isinstance(value, dict):
        for key, nested in value.items():
            if key in FORBIDDEN_ARM_KEYS:
                fail("blind score record contains an arm-identifying field")
            reject_arm_labels(nested)
    elif isinstance(value, list):
        for nested in value:
            reject_arm_labels(nested)


def validate_schema_shape(schema: dict) -> None:
    definitions = schema.get("$defs", {})

    def walk(value: object) -> None:
        if isinstance(value, dict):
            reference = value.get("$ref")
            if reference is not None:
                prefix = "#/$defs/"
                if not reference.startswith(prefix) or reference[len(prefix):] not in definitions:
                    fail("review schema contains an unresolved local reference")
            if value.get("type") == "object" and "required" in value:
                if not set(value["required"]) <= set(value.get("properties", {})):
                    fail("review schema has a required field without a property definition")
            for nested in value.values():
                walk(nested)
        elif isinstance(value, list):
            for nested in value:
                walk(nested)

    walk(schema)
    if FORBIDDEN_ARM_KEYS & set(schema["$defs"]["blindScore"]["properties"]):
        fail("blind score schema exposes an arm-identifying property")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("record", nargs="?", type=Path, default=TEMPLATE)
    args = parser.parse_args()

    candidate_raw = CANDIDATE.read_bytes()
    candidate_hash = hashlib.sha256(candidate_raw).hexdigest()
    package_hash = hashlib.sha256(PACKAGE.read_bytes()).hexdigest()
    if candidate_hash != EXPECTED_CANDIDATE_SHA256:
        fail("candidate SHA-256 drift; review requires a new explicit pin")
    if package_hash != EXPECTED_PACKAGE_SHA256:
        fail("review package SHA-256 drift; review requires a new explicit pin")

    candidate = json.loads(candidate_raw)
    record = json.loads(args.record.read_text(encoding="utf-8"))
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    validate_schema_shape(schema)
    holdout = {task["id"]: task for task in candidate["tasks"] if task["split"] == "holdout"}
    sources = {source["logical_id"]: source for source in candidate["sources"] if source["split"] == "holdout"}
    verify_source_manifest(candidate)

    if record.get("schema_version") != "m6-agent-blind-review-v1":
        fail("unsupported review schema version")
    if record.get("candidate_sha256") != candidate_hash or record.get("review_package_sha256") != package_hash:
        fail("review record is not bound to current candidate and package hashes")
    if record.get("human_review") is not False:
        fail("agent review record must explicitly set human_review=false")
    reviewer = record.get("reviewer", {})
    producer = record.get("producer", {})
    if producer.get("role") != "candidate_producer" or not producer.get("identity"):
        fail("candidate producer identity is missing")
    if reviewer.get("role") != "independent_agent_reviewer" or reviewer.get("independent_from_producer") is not True:
        fail("reviewer independence declaration is missing")
    if reviewer.get("identity") in {None, "", "PENDING", producer.get("identity")}:
        if record.get("record_status") != "template_only":
            fail("reviewer identity must be distinct from candidate producer")

    gold_reviews = record.get("gold_reviews", [])
    artifact_scores = record.get("artifact_scores", [])
    blind_scores = record.get("blind_scores", [])
    status = record.get("record_status")
    if status == "template_only":
        if gold_reviews or artifact_scores or blind_scores or record.get("quality_claim") != "not_evaluated":
            fail("template_only record must contain no scores or quality claim")
    elif status == "gold_reviewed":
        validate_gold_freeze(record, holdout, sources)
    elif status == "agent_reviewed":
        if record.get("quality_claim") != "agent_reviewed":
            fail("completed review must still avoid a pass claim")
        if len(gold_reviews) != 30 or {item.get("task_id") for item in gold_reviews} != set(holdout):
            fail("completed pre-run review must cover exactly all 30 holdout tasks")
        if any(item.get("decision") != "agent_reviewed" for item in gold_reviews):
            fail("unresolved or revised gold cannot be marked fully reviewed")
        if any(item.get("final_decision") == "insufficient_evidence" for item in record.get("adjudications", [])):
            fail("unresolved adjudication cannot be marked fully reviewed")
        if not artifact_scores or any(item.get("decision") != "agent_reviewed" for item in artifact_scores):
            fail("semantic artifact review must cover exported observations/cards/relations/packs")
        if len(blind_scores) != 90:
            fail("completed blind scoring must include exactly 90 holdout outputs")
        all_blind_ids = [item.get("blind_output_id") for item in [*artifact_scores, *blind_scores]]
        if len(set(all_blind_ids)) != len(all_blind_ids):
            fail("blind output IDs must be unique")
        counts = {task_id: 0 for task_id in holdout}
        for score in blind_scores:
            task_id = score.get("task_id")
            if task_id not in counts:
                fail("blind score references a non-holdout task")
            counts[task_id] += 1
            if score.get("decision") != "agent_reviewed":
                fail("unresolved blind score cannot be marked complete")
            task = holdout[task_id]
            if len(score.get("coverage", [])) != len(task["expected_usable_information"]):
                fail("coverage denominator does not match frozen gold")
            if len(score.get("qualifiers", [])) != len(task["must_preserve"]):
                fail("qualifier denominator does not match frozen gold")
            expected_coverage_ids = {f"usable:{index}" for index in range(len(task["expected_usable_information"]))}
            expected_qualifier_ids = {f"preserve:{index}" for index in range(len(task["must_preserve"]))}
            if {item.get("gold_item_id") for item in score.get("coverage", [])} != expected_coverage_ids:
                fail("coverage item IDs do not match frozen gold")
            if {item.get("gold_item_id") for item in score.get("qualifiers", [])} != expected_qualifier_ids:
                fail("qualifier item IDs do not match frozen gold")
            if score.get("no_answer", {}).get("applicable") is not task["expected_no_answer"]:
                fail("no-answer scoring scope differs from frozen gold")
            dimensions = score.get("task_result", {})
            dimension_names = {"critical_constraints", "state", "next_step", "forbidden_inference"}
            if set(dimensions) != dimension_names or any(
                dimensions.get(key) not in {"met", "not_met"} for key in dimension_names
            ):
                fail("unresolved task dimension cannot be marked complete")
            for evidence in score.get("evidence", []):
                check_evidence(evidence, sources)
            for claim in score.get("fact_claims", []):
                for evidence in claim.get("evidence", []):
                    check_evidence(evidence, sources)
        if set(counts.values()) != {3}:
            fail("each holdout task must have three blind outputs")
        if not record.get("score_locked_at") or not record.get("arm_mapping_revealed_at"):
            fail("completed scores must lock before the arm mapping is revealed")
    elif status not in {"pending", "needs_revision", "insufficient_evidence"}:
        fail("invalid review status")

    for review in gold_reviews:
        task_id = review.get("task_id")
        if task_id not in holdout:
            fail("gold review references a non-holdout task")
        task = holdout[task_id]
        validate_gold_review(review, task, sources, freeze=(status == "gold_reviewed"))
    for score in artifact_scores:
        if score.get("source_id") not in sources:
            fail("artifact review references a non-holdout source")
        for evidence in score.get("evidence", []):
            check_evidence(evidence, sources)
        for claim in score.get("fact_claims", []):
            for evidence in claim.get("evidence", []):
                check_evidence(evidence, sources)
        for relation in score.get("relations", []):
            for evidence in relation.get("evidence", []):
                check_evidence(evidence, sources)
    for item in record.get("adjudications", []):
        if item.get("task_id") not in holdout:
            fail("adjudication references a non-holdout task")
        adjudicator = item.get("adjudicator", {})
        if adjudicator.get("independent_from_producer") is not True or adjudicator.get("identity") == producer.get("identity"):
            fail("adjudicator is not independent from producer")
        for evidence in item.get("evidence", []):
            check_evidence(evidence, sources)
    reject_arm_labels(record)
    if record.get("arm_mapping_revealed_at") and record.get("score_locked_at"):
        if record["arm_mapping_revealed_at"] < record["score_locked_at"]:
            fail("arm mapping was revealed before blind scores were locked")

    print(json.dumps({
        "status": "PASS",
        "record_status": status,
        "human_review": False,
        "candidate_sha256": candidate_hash,
        "review_package_sha256": package_hash,
        "holdout_tasks": 30,
        "blind_scores": len(blind_scores),
        "schema_refs": "verified",
    }))


if __name__ == "__main__":
    main()
