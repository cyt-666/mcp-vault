#!/usr/bin/env python3
"""Build and verify a pinned, evidence-bearing holdout review packet."""

from __future__ import annotations

import argparse
import hashlib
import html
import json
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
CANDIDATE = HERE / "candidate.json"
OUTPUT = HERE / "holdout-independent-review.zh-CN.md"
EXPECTED_COMMIT = "e046ceb59393db9a4977ba55899eba301ad28005"
EXPECTED_CANDIDATE_SHA256 = "bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2"
MAX_QUOTED_LINES_PER_REF = 12
MAX_QUOTED_CHARS_PER_LINE = 360


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def source_bytes(commit: str, path: str) -> bytes:
    return subprocess.check_output(["git", "show", f"{commit}:{path}"], cwd=ROOT)


def escape(value: object) -> str:
    return html.escape(str(value), quote=False)


def display_lines(lines: list[str]) -> str:
    rendered: list[str] = []
    for line in lines:
        if len(line) > MAX_QUOTED_CHARS_PER_LINE:
            line = line[:MAX_QUOTED_CHARS_PER_LINE] + " … [长行截断；须回看固定来源]"
        content = line.rstrip(" \t")
        trailing = line[len(content) :]
        encoded_trailing = "".join("&#32;" if char == " " else "&#9;" for char in trailing)
        escaped = escape(content).replace("[", "&#91;").replace("]", "&#93;")
        rendered.append("> " + escaped + encoded_trailing if line else ">")
    return "\n".join(rendered)


def source_map(candidate: dict) -> dict[str, dict]:
    result = {}
    for source in candidate["sources"]:
        if source["split"] != "holdout":
            continue
        content = source_bytes(candidate["repository_commit"], source["path"])
        blob_oid = git("rev-parse", f"{candidate['repository_commit']}:{source['path']}")
        digest = hashlib.sha256(content).hexdigest()
        lines = content.decode("utf-8").splitlines()
        assert blob_oid == source["git_blob_oid"], source["logical_id"]
        assert digest == source["content_sha256"], source["logical_id"]
        assert len(lines) == source["line_count"], source["logical_id"]
        result[source["logical_id"]] = {**source, "lines": lines}
    assert len(result) == 15
    return result


def evidence_block(ref: dict, sources: dict[str, dict]) -> str:
    source = sources[ref["source_id"]]
    assert ref["path"] == source["path"]
    start, end = int(ref["line_start"]), int(ref["line_end"])
    lines = source["lines"]
    assert 1 <= start <= end <= len(lines)
    cited = lines[start - 1 : end]
    basis = ref["basis"]
    citation = (
        f"`{source['path']}:{start}-{end}`; blob `{source['git_blob_oid']}`; "
        f"SHA-256 `{source['content_sha256']}`; basis `{basis}`"
    )
    if basis == "full_document_absence_candidate":
        headings = [(index + 1, line) for index, line in enumerate(lines) if line.startswith("#")]
        navigation = [(1, lines[0])] if lines else []
        navigation.extend((number, line) for number, line in headings[:8] if (number, line) not in navigation)
        excerpt = display_lines([f"L{number}: {line}" for number, line in navigation])
        return (
            f"- {citation}\n"
            "  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，"
            "不能证明不存在答案。\n"
            f"{excerpt}"
        )

    selected = cited[:MAX_QUOTED_LINES_PER_REF]
    excerpt = display_lines([f"L{start + offset}: {line}" for offset, line in enumerate(selected)])
    omitted = "" if len(cited) <= len(selected) else f"\n  - 引用范围还有 {len(cited)-len(selected)} 行未摘录；须回看该范围。"
    return f"- {citation}{omitted}\n{excerpt}"


def build(candidate: dict, candidate_sha: str) -> str:
    sources = source_map(candidate)
    tasks = [task for task in candidate["tasks"] if task["split"] == "holdout"]
    assert len(tasks) == 30
    lines = [
        "# M6 Holdout 30 独立智能体 pre-run gold 复核包",
        "",
        "> 状态：未复核、未冻结；供独立复核者使用。生成包不代表任务、gold 或 M6 已通过。",
        "> 复核主体：独立主代理；candidate producer 不得自评。输出标记 `agent_reviewed`，`human_review=false`。",
        "",
        f"- candidate SHA-256：`{candidate_sha}`",
        f"- 固定 commit：`{candidate['repository_commit']}`",
        f"- 任务数：{len(tasks)} holdout（development 30 不进入当前 live task set）",
        "- 所有摘录均由该 commit 的 Git blob 生成；长行截断仅用于阅读，必须按 blob/SHA 回看原行。",
        "- v4 按独立复核意见补充状态、来源证据和限定；全部仍 pending independent-agent-review，本包不作批准，ADR-0038 要求 `human_review=false`。",
        "",
        "## 评审记录规则",
        "",
        "每题分别判断 query 可回答性、重点/限定/禁止推断/状态、关系类型与方向、严重度及证据覆盖。"
        "对无答案任务，必须逐行检查列出的整个 ADR；标题和章节导航摘录不能证明不存在答案。"
        "证据不足标 `insufficient_evidence`，不得猜测。记录建议修订但不要改写冻结候选。",
        "",
        "## Holdout 任务",
        "",
    ]

    for task in tasks:
        cited_source_ids = {ref["source_id"] for ref in task["evidence_refs"]}
        assert set(task["source_ids"]) <= cited_source_ids, task["id"]
        for relation in task["expected_source_relations"]:
            assert set(relation["source_ids"]) <= cited_source_ids, task["id"]
        flags = []
        if task["expected_no_answer"]:
            flags.append("no-answer-full-document-check")
        if task["expected_source_relations"]:
            flags.append("cross-source-relation-review")
        if task["severity"] in {"high", "critical"}:
            flags.append(f"severity-{task['severity']}")
        if any(any(token in item.lower() for token in ["not", "never", "only", "must", "不得", "不能", "仅", "必须", "除非", "不再"])
               for item in [task["query"], *task["must_preserve"], *task["must_not_infer"]]):
            flags.append("qualifier-negation-review")
        lines += [
            f"### {task['id']}",
            "",
            f"- 风险标记：{', '.join(flags) if flags else 'standard-review'}",
            f"- 来源：{', '.join(task['source_ids'])}；split：holdout；severity：`{task['severity']}`",
            f"- query：{escape(task['query'])}",
            "- 候选 gold（只供离线复核；不得传给 Provider）：",
        ]
        for label, field in [
            ("预期可用重点", "expected_usable_information"),
            ("必须保留", "must_preserve"),
            ("禁止推断", "must_not_infer"),
        ]:
            values = task[field]
            lines.append(f"  - {label}：" + ("；".join(escape(value) for value in values) if values else "（空）"))
        lines += [
            f"  - 正确状态：`{escape(task['expected_status'])}`；no-answer：`{str(task['expected_no_answer']).lower()}`",
            "  - 预期来源关系：",
        ]
        if task["expected_source_relations"]:
            for relation in task["expected_source_relations"]:
                lines.append(f"    - kind `{escape(relation['kind'])}`；candidate 端点顺序 `{escape(' → '.join(relation['source_ids']))}`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。")
        else:
            lines.append("    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。")
        lines += ["- 冻结来源证据："]
        for ref in task["evidence_refs"]:
            lines.append(evidence_block(ref, sources))
        lines += [
            "- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。",
            "- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________",
            "- 复核者/模型版本：________；复核时间：________；`human_review=false`。",
            "- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。",
            "",
        ]
    lines += [
        "## 独立复核汇总",
        "",
        "- `candidate_sha256`：",
        "- 30 条结论计数（agent_reviewed / insufficient_evidence / pending）：",
        "- 需要修订/排除的 Task ID 与依据：",
        "- source split / 跨题材泄漏疑点：",
        "- `human_review=false`：true",
        "- 复核者及模型/版本：",
        "- 盲评映射在评分锁定后揭示：yes/no；映射证据摘要：",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--write", action="store_true", help="generate the pinned packet")
    group.add_argument("--check", action="store_true", help="verify the packet is current")
    args = parser.parse_args()

    raw = CANDIDATE.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    assert digest == EXPECTED_CANDIDATE_SHA256, "candidate hash drift; review pin must be updated deliberately"
    candidate = json.loads(raw)
    assert candidate["repository_commit"] == EXPECTED_COMMIT
    assert git("rev-parse", "HEAD") == EXPECTED_COMMIT, "repository HEAD drift"
    expected = build(candidate, digest).encode("utf-8")
    if args.write:
        OUTPUT.write_bytes(expected)
        print(json.dumps({"status": "WROTE", "holdout_tasks": 30, "candidate_sha256": digest, "output": str(OUTPUT.relative_to(ROOT))}))
    else:
        actual = OUTPUT.read_bytes()
        assert actual == expected, "packet differs from deterministic pinned-source rendering"
        print(json.dumps({"status": "PASS", "holdout_tasks": 30, "candidate_sha256": digest, "source_blobs": 15, "line_references": "verified", "output": str(OUTPUT.relative_to(ROOT))}))


if __name__ == "__main__":
    main()
