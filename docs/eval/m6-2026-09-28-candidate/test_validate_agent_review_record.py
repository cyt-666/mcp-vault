"""Negative and positive checks for pre-run gold freeze validation."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import validate_agent_review_record as validator  # noqa: E402


class CompletedTaskResultValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        candidate = json.loads((HERE / "candidate.json").read_text(encoding="utf-8"))
        source = next(source for source in candidate["sources"] if source["split"] == "holdout")
        self.sources = {source["logical_id"]: source}
        self.result = {
            "critical_constraints": "met",
            "state": "not_met",
            "next_step": "met",
            "forbidden_inference": "met",
            "evidence": [{
                "source_id": source["logical_id"],
                "path": source["path"],
                "line_start": 1,
                "line_end": source["line_count"],
                "git_blob_oid": source["git_blob_oid"],
                "content_sha256": source["content_sha256"],
            }],
        }

    def test_schema_required_evidence_is_accepted_with_terminal_dimensions(self) -> None:
        schema = json.loads((HERE / "post-run-agent-review.schema.json").read_text(encoding="utf-8"))
        expected = schema["$defs"]["blindScore"]["properties"]["task_result"]["required"]
        self.assertEqual(set(self.result), set(expected))
        validator.validate_completed_task_result(self.result, self.sources)

    def test_incomplete_or_unknown_dimensions_cannot_be_marked_complete(self) -> None:
        for change in ("missing_evidence", "extra_dimension", "unresolved"):
            with self.subTest(change=change):
                result = copy.deepcopy(self.result)
                if change == "missing_evidence":
                    del result["evidence"]
                elif change == "extra_dimension":
                    result["extra"] = "met"
                else:
                    result["state"] = "insufficient_evidence"
                with self.assertRaises(SystemExit):
                    validator.validate_completed_task_result(result, self.sources)

    def test_nested_task_evidence_is_validated(self) -> None:
        for change in ("not_array", "not_record", "wrong_hash", "invalid_lines"):
            with self.subTest(change=change):
                result = copy.deepcopy(self.result)
                if change == "not_array":
                    result["evidence"] = {}
                elif change == "not_record":
                    result["evidence"] = ["not evidence"]
                elif change == "wrong_hash":
                    result["evidence"][0]["content_sha256"] = "0" * 64
                else:
                    result["evidence"][0]["line_start"] = 0
                with self.assertRaises(SystemExit):
                    validator.validate_completed_task_result(result, self.sources)


class GoldFreezeValidationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.candidate = json.loads((HERE / "candidate.json").read_text(encoding="utf-8"))
        cls.holdout = {
            task["id"]: task for task in cls.candidate["tasks"] if task["split"] == "holdout"
        }
        cls.sources = {
            source["logical_id"]: source
            for source in cls.candidate["sources"]
            if source["split"] == "holdout"
        }
        validator.verify_source_manifest(cls.candidate)

    @classmethod
    def make_record(cls) -> dict:
        reviews = []
        field_names = (
            "query", "usable_information", "must_preserve", "must_not_infer",
            "status", "relations", "severity", "no_answer_scope",
        )
        for task in cls.holdout.values():
            decisions = {
                field: {
                    "decision": "supported",
                    "rationale": "Synthetic validator fixture only; not a semantic review.",
                }
                for field in field_names
            }
            if not task["expected_no_answer"]:
                decisions["no_answer_scope"]["decision"] = "not_applicable"
            evidence = []
            for ref in task["evidence_refs"]:
                source = cls.sources[ref["source_id"]]
                evidence.append({
                    "source_id": source["logical_id"],
                    "path": source["path"],
                    "line_start": ref["line_start"],
                    "line_end": ref["line_end"],
                    "git_blob_oid": source["git_blob_oid"],
                    "content_sha256": source["content_sha256"],
                })
            reviews.append({
                "task_id": task["id"],
                "decision": "agent_reviewed",
                "field_decisions": decisions,
                "evidence": evidence,
                "risk_notes": [],
            })
        return {
            "record_status": "gold_reviewed",
            "human_review": False,
            "quality_claim": "not_evaluated",
            "producer": {"identity": "producer-test"},
            "reviewer": {"identity": "independent-reviewer-test"},
            "gold_locked_at": "2026-09-28T12:00:00Z",
            "gold_reviews": reviews,
            "artifact_scores": [],
            "blind_scores": [],
            "adjudications": [],
        }

    def assert_rejected(self, record: dict, expected: str) -> None:
        with self.assertRaisesRegex(SystemExit, expected):
            validator.validate_gold_freeze(record, self.holdout, self.sources)

    def test_valid_synthetic_freeze_passes(self) -> None:
        validator.validate_gold_freeze(self.make_record(), self.holdout, self.sources)

    def test_schema_freeze_is_terminal_and_has_no_post_run_arrays(self) -> None:
        schema = json.loads((HERE / "post-run-agent-review.schema.json").read_text(encoding="utf-8"))
        validator.validate_schema_shape(schema)
        freeze = next(
            item for item in schema["allOf"]
            if item["if"]["properties"]["record_status"]["const"] == "gold_reviewed"
        )["then"]["properties"]
        self.assertEqual(freeze["blind_scores"]["maxItems"], 0)
        self.assertEqual(freeze["artifact_scores"]["maxItems"], 0)
        self.assertEqual(freeze["adjudications"]["maxItems"], 0)
        self.assertEqual(freeze["gold_reviews"]["items"]["properties"]["decision"]["const"], "agent_reviewed")

    def test_revise_field_blocks_freeze(self) -> None:
        record = self.make_record()
        record["gold_reviews"][0]["field_decisions"]["query"]["decision"] = "revise"
        self.assert_rejected(record, "cannot contain unresolved or revised field decisions")

    def test_blank_rationale_blocks_freeze(self) -> None:
        record = self.make_record()
        record["gold_reviews"][0]["field_decisions"]["query"]["rationale"] = "  "
        self.assert_rejected(record, "require evidence-based rationale")

    def test_no_answer_requires_supported_scope(self) -> None:
        record = self.make_record()
        task_id = next(task_id for task_id, task in self.holdout.items() if task["expected_no_answer"])
        review = next(item for item in record["gold_reviews"] if item["task_id"] == task_id)
        review["field_decisions"]["no_answer_scope"]["decision"] = "not_applicable"
        self.assert_rejected(record, "no-answer gold scope must be supported")

    def test_answerable_task_requires_not_applicable_no_answer_scope(self) -> None:
        record = self.make_record()
        task_id = next(task_id for task_id, task in self.holdout.items() if not task["expected_no_answer"])
        review = next(item for item in record["gold_reviews"] if item["task_id"] == task_id)
        review["field_decisions"]["no_answer_scope"]["decision"] = "supported"
        self.assert_rejected(record, "must be not_applicable")

    def test_no_answer_requires_full_document_evidence_for_every_source(self) -> None:
        record = self.make_record()
        task_id = next(task_id for task_id, task in self.holdout.items() if task["expected_no_answer"])
        task = self.holdout[task_id]
        review = next(item for item in record["gold_reviews"] if item["task_id"] == task_id)
        first_source = task["source_ids"][0]
        source = self.sources[first_source]
        review["evidence"] = [
            item for item in review["evidence"]
            if not (item["source_id"] == first_source and item["line_start"] == 1 and item["line_end"] == source["line_count"])
        ]
        self.assert_rejected(record, "lacks full-document audit evidence")

    def test_evidence_must_belong_to_fixed_task_sources(self) -> None:
        record = self.make_record()
        task = next(task for task in self.holdout.values() if len(task["source_ids"]) == 1)
        review = next(item for item in record["gold_reviews"] if item["task_id"] == task["id"])
        other_source = next(source for source_id, source in self.sources.items() if source_id not in task["source_ids"])
        review["evidence"][0].update({
            "source_id": other_source["logical_id"],
            "path": other_source["path"],
            "git_blob_oid": other_source["git_blob_oid"],
            "content_sha256": other_source["content_sha256"],
            "line_end": other_source["line_count"],
        })
        self.assert_rejected(record, "outside the fixed task")

    def test_evidence_hash_and_line_bounds_are_checked(self) -> None:
        record = self.make_record()
        evidence = record["gold_reviews"][0]["evidence"][0]
        evidence["content_sha256"] = "0" * 64
        self.assert_rejected(record, "source hash mismatch")

        record = self.make_record()
        evidence = record["gold_reviews"][0]["evidence"][0]
        evidence["line_end"] = self.sources[evidence["source_id"]]["line_count"] + 1
        self.assert_rejected(record, "line range is invalid")

    def test_freeze_requires_reviewer_lock_and_empty_post_run_arrays(self) -> None:
        record = self.make_record()
        record["reviewer"]["identity"] = record["producer"]["identity"]
        self.assert_rejected(record, "distinct from producer")

        record = self.make_record()
        record["human_review"] = True
        self.assert_rejected(record, "human_review=false")

        record = self.make_record()
        record["gold_locked_at"] = "not-a-date"
        self.assert_rejected(record, "ISO-8601")

        record = self.make_record()
        record["adjudications"].append({})
        self.assert_rejected(record, "must not include post-run scores or adjudications")


if __name__ == "__main__":
    unittest.main()
