#!/usr/bin/env python3
"""Reproducible local workflow for the MechMania 32 M1 workspace."""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import traceback
import urllib.request
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".local"
RECORDS = LOCAL / "records"
ARTIFACTS = ROOT / "artifacts" / "summaries" / "m01"
UPSTREAM_LOCK = ROOT / "configs" / "upstream.lock.json"
SUITE_PATH = ROOT / "configs" / "suites" / "smoke.json"
BOT_MANIFEST = ROOT / "bot" / "Cargo.toml"
RULES_MANIFEST = ROOT / "lab" / "rules" / "Cargo.toml"
ENGINE = ROOT / "bot" / ".mm" / "bin" / "mm-engine"
MM_CLI = ROOT / ".local" / "bin" / "mm-cli"

EXIT_OK = 0
EXIT_PRECONDITION = 2
EXIT_SUBPROCESS = 3
EXIT_EVIDENCE = 4


class ToolFailure(RuntimeError):
    def __init__(self, message: str, code: int = EXIT_PRECONDITION):
        super().__init__(message)
        self.code = code


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def stamp() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def rel(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return str(path)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_tree(path: Path) -> str:
    digest = hashlib.sha256()
    if not path.exists():
        return digest.hexdigest()
    for item in sorted(p for p in path.rglob("*") if p.is_file()):
        relative = item.relative_to(path).as_posix().encode()
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        with item.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    return digest.hexdigest()


def input_fingerprints() -> dict[str, str]:
    """Hashes that bind test and run evidence to the current candidate tree."""
    return {
        "strategy_tree": sha256_tree(ROOT / "bot" / "src" / "strategy"),
        "rules_tree": sha256_tree(ROOT / "lab" / "rules"),
        "opponents_tree": sha256_tree(ROOT / "lab" / "opponents"),
        "workflow_tool": sha256_file(ROOT / "tools" / "mm32.py"),
        "upstream_lock": sha256_file(UPSTREAM_LOCK),
        "baseline": sha256_file(ROOT / "configs" / "baseline.json"),
        "smoke_suite": sha256_file(SUITE_PATH),
    }


def attempt_path(command: str) -> Path:
    return RECORDS / f"{command}-attempt-latest.json"


def write_attempt(command: str, status: str, **extra: Any) -> None:
    record = {
        "schema_version": 1,
        "command": command,
        "generated_at": utc_now(),
        "status": status,
        **extra,
    }
    write_json(attempt_path(command), record)


def load_json(path: Path) -> dict[str, Any]:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ToolFailure(f"无法读取 JSON {rel(path)}: {exc}") from exc


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp_path = Path(temporary)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(encoded)
        temp_path.replace(path)
    finally:
        temp_path.unlink(missing_ok=True)


def tool_env(target_name: str | None = None) -> dict[str, str]:
    env = os.environ.copy()
    rustup_bin = Path("/opt/homebrew/opt/rustup/bin")
    path_parts = [str(ROOT / ".local" / "bin")]
    if rustup_bin.exists():
        path_parts.append(str(rustup_bin))
    path_parts.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"])
    env["PATH"] = os.pathsep.join(path_parts)
    env["RUSTUP_HOME"] = str(LOCAL / "rustup")
    env["CARGO_HOME"] = str(LOCAL / "cargo")
    if target_name:
        env["CARGO_TARGET_DIR"] = str(LOCAL / "target" / target_name)
    env["GIT_TERMINAL_PROMPT"] = "0"
    return env


def redact_arg(value: str) -> str:
    value = value.replace(str(ROOT), "$REPO")
    value = re.sub(r"(https?://)[^/@\s]+:[^/@\s]+@", r"\1<redacted>@", value)
    if re.search(r"(?i)(token|password|secret)=", value):
        key = value.split("=", 1)[0]
        return f"{key}=<redacted>"
    return value


def run_command(
    argv: list[str],
    *,
    cwd: Path = ROOT,
    timeout: int = 300,
    env: dict[str, str] | None = None,
    log_path: Path | None = None,
    show_output: bool = False,
) -> dict[str, Any]:
    safe_argv = [redact_arg(str(arg)) for arg in argv]
    print(f"[mm32] cwd={rel(cwd)} cmd={' '.join(safe_argv)}", flush=True)
    started = time.monotonic()
    timed_out = False
    try:
        completed = subprocess.run(
            argv,
            cwd=cwd,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
        )
        output = completed.stdout
        returncode = completed.returncode
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        stdout = exc.stdout.decode(errors="replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")
        stderr = exc.stderr.decode(errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
        output = stdout + stderr
        returncode = 124
    except OSError as exc:
        output = f"failed to execute command: {exc}\n"
        returncode = 127
    duration = time.monotonic() - started

    if log_path is not None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text(output, encoding="utf-8")
    if show_output and output:
        print(output, end="" if output.endswith("\n") else "\n")

    record: dict[str, Any] = {
        "argv": safe_argv,
        "cwd": rel(cwd),
        "exit_code": returncode,
        "duration_seconds": round(duration, 6),
        "timed_out": timed_out,
    }
    if log_path is not None:
        record["log_path"] = rel(log_path)
        record["log_sha256"] = sha256_file(log_path)
        record["log_bytes"] = log_path.stat().st_size
    record["output"] = output
    return record


def require_ok(record: dict[str, Any], label: str) -> None:
    if record["exit_code"] != 0:
        log_hint = f"；完整日志 {record.get('log_path')}" if record.get("log_path") else ""
        raise ToolFailure(
            f"{label} 失败，退出码 {record['exit_code']}{log_hint}",
            EXIT_SUBPROCESS,
        )


def command_for_evidence(record: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in record.items() if key != "output"}


def cargo_command(subcommand: str, manifest: Path, *extra: str) -> list[str]:
    cargo = os.environ.get("MM32_CARGO", "cargo")
    return [cargo, "+stable", subcommand, "--locked", "--manifest-path", str(manifest), *extra]


def git_value(*args: str) -> str | None:
    record = run_command(["git", *args], timeout=30)
    if record["exit_code"] != 0:
        return None
    return record["output"].strip()


def repository_facts() -> dict[str, Any]:
    return {
        "remote_origin": git_value("remote", "get-url", "origin"),
        "branch": git_value("branch", "--show-current"),
        "head": git_value("rev-parse", "HEAD"),
        "status_porcelain": (git_value("status", "--short", "--untracked-files=all") or "").splitlines(),
    }


def evaluated_code_commit(repository: dict[str, Any]) -> str | None:
    """Return HEAD when every dirty path is generated evidence, not evaluated inputs."""
    evidence_prefixes = (
        "artifacts/feedback/",
        "artifacts/summaries/",
        "docs/reports/",
    )
    for line in repository.get("status_porcelain", []):
        path = line[3:] if len(line) >= 4 else line
        if " -> " in path:
            path = path.rsplit(" -> ", 1)[1]
        if not path.startswith(evidence_prefixes):
            return None
    return repository.get("head")


def parse_test_totals(outputs: Iterable[str]) -> dict[str, int]:
    passed = failed = ignored = 0
    pattern = re.compile(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored")
    for output in outputs:
        for match in pattern.finditer(output):
            passed += int(match.group(1))
            failed += int(match.group(2))
            ignored += int(match.group(3))
    return {"passed": passed, "failed": failed, "skipped": ignored}


def doctor(_: argparse.Namespace) -> int:
    checks: dict[str, Any] = {
        "schema_version": 1,
        "generated_at": utc_now(),
        "repository": repository_facts(),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "processor": platform.processor() or None,
        },
        "paths": {},
        "commands": {},
    }
    required = [UPSTREAM_LOCK, SUITE_PATH, BOT_MANIFEST, RULES_MANIFEST, ENGINE, MM_CLI]
    for path in required:
        checks["paths"][rel(path)] = path.exists()

    for name, argv in {
        "git": ["git", "--version"],
        "rustc_stable": ["rustc", "+stable", "--version"],
        "rustc_pinned_nightly": ["rustc", "+nightly-2024-07-25", "--version"],
        "cargo_stable": ["cargo", "+stable", "--version"],
        "mm_cli": [str(MM_CLI), "--version"],
        "engine": [str(ENGINE), "--version"],
    }.items():
        record = run_command(argv, env=tool_env(), timeout=30)
        checks["commands"][name] = {
            "exit_code": record["exit_code"],
            "output": record["output"].strip(),
        }

    network: dict[str, Any] = {"target": "github.com:443", "reachable": False, "error": None}
    try:
        with socket.create_connection(("github.com", 443), timeout=2):
            network["reachable"] = True
    except OSError as exc:
        network["error"] = str(exc)
    checks["network"] = network

    ok = all(checks["paths"].values()) and all(
        item["exit_code"] == 0 for item in checks["commands"].values()
    )
    checks["status"] = "ok" if ok else "missing_prerequisite"
    write_json(RECORDS / "doctor-latest.json", checks)
    print(json.dumps(checks, ensure_ascii=False, indent=2))
    return EXIT_OK if ok else EXIT_PRECONDITION


def read_mm_lock() -> dict[str, Any]:
    path = ROOT / "bot" / ".mm" / "lock.toml"
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ToolFailure(f"无法读取 {rel(path)}: {exc}") from exc


def ensure_upstream_snapshots(expected: dict[str, Any]) -> list[dict[str, Any]]:
    actions: list[dict[str, Any]] = []
    for name, source in expected["sources"].items():
        archive = ROOT / source["local_archive"]
        checkout = ROOT / source["local_source"]
        if not archive.exists():
            archive.parent.mkdir(parents=True, exist_ok=True)
            temporary = archive.with_suffix(archive.suffix + ".partial")
            print(f"[mm32] download pinned {name}: {source['archive_url']}", flush=True)
            try:
                with urllib.request.urlopen(source["archive_url"], timeout=60) as response:
                    with temporary.open("wb") as handle:
                        shutil.copyfileobj(response, handle)
                temporary.replace(archive)
            except (OSError, TimeoutError) as exc:
                temporary.unlink(missing_ok=True)
                raise ToolFailure(f"下载固定 {name} 归档失败：{exc}", EXIT_SUBPROCESS) from exc
            actions.append({"action": "download", "source": name, "path": rel(archive)})
        if sha256_file(archive) != source["archive_sha256"]:
            raise ToolFailure(f"固定 {name} 归档 SHA256 不匹配", EXIT_EVIDENCE)

        with tempfile.TemporaryDirectory(prefix=f"verify-{name}-", dir=LOCAL) as temporary:
            extraction = Path(temporary)
            with tarfile.open(archive, "r:gz") as bundle:
                bundle.extractall(extraction, filter="data")
            roots = [path for path in extraction.iterdir() if path.is_dir()]
            if len(roots) != 1:
                raise ToolFailure(f"固定 {name} 归档顶层目录数量异常", EXIT_EVIDENCE)
            extracted = roots[0]
            extracted_hash = sha256_tree(extracted)
            if checkout.exists():
                if sha256_tree(checkout) != extracted_hash:
                    raise ToolFailure(f"{rel(checkout)} 与固定 {name} 归档内容不一致", EXIT_EVIDENCE)
            else:
                checkout.parent.mkdir(parents=True, exist_ok=True)
                shutil.copytree(extracted, checkout)
                actions.append({"action": "extract", "source": name, "path": rel(checkout)})
    return actions


def restore_generated_bot_scaffold(starter: Path) -> list[str]:
    restored: list[str] = []
    for source in sorted(path for path in starter.rglob("*") if path.is_file()):
        relative = source.relative_to(starter)
        if relative.as_posix() in {".gitignore", "mm-config.toml"}:
            continue
        if relative.parts[:2] == ("src", "strategy"):
            continue
        destination = ROOT / "bot" / relative
        if not destination.exists() or sha256_file(destination) != sha256_file(source):
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
            restored.append(rel(destination))
    return restored


def bootstrap(_: argparse.Namespace) -> int:
    strategy = ROOT / "bot" / "src" / "strategy"
    before = sha256_tree(strategy) if strategy.exists() else None
    commands: list[dict[str, Any]] = []
    expected = load_json(UPSTREAM_LOCK)
    source_actions = ensure_upstream_snapshots(expected)
    starter = LOCAL / "upstream" / "rust-starterpack"
    bot_dir = ROOT / "bot"
    bot_dir.mkdir(parents=True, exist_ok=True)

    if not (bot_dir / "mm-config.toml").exists():
        shutil.copy2(starter / "mm-config.toml", bot_dir / "mm-config.toml")
    if not (bot_dir / ".gitignore").exists():
        shutil.copy2(starter / ".gitignore", bot_dir / ".gitignore")
    if not strategy.exists():
        shutil.copytree(starter / "src" / "strategy", strategy)
    restored_scaffold = restore_generated_bot_scaffold(starter)

    if not MM_CLI.exists():
        cli_project = LOCAL / "upstream" / "cli"
        record = run_command(
            cargo_command("build", cli_project / "Cargo.toml", "--release", "--bin", "mm-cli"),
            env=tool_env("cli"),
            timeout=900,
            log_path=RECORDS / "logs" / "bootstrap-cli-build.log",
        )
        commands.append({"label": "restore pinned mm-cli", **command_for_evidence(record)})
        require_ok(record, "构建固定 mm-cli")
        built_cli = LOCAL / "target" / "cli" / "release" / "mm-cli"
        MM_CLI.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(built_cli, MM_CLI)

    lock_template = ROOT / expected["engine_binary"]["source_lock_template_path"]
    mm_lock_path = bot_dir / ".mm" / "lock.toml"
    if not mm_lock_path.exists():
        mm_lock_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(lock_template, mm_lock_path)

    if not ENGINE.exists():
        engine_project = LOCAL / "upstream" / "engine"
        record = run_command(
            cargo_command("build", engine_project / "Cargo.toml", "--release", "--bin", "mm-engine"),
            env=tool_env("engine-bootstrap"),
            timeout=900,
            log_path=RECORDS / "logs" / "bootstrap-engine-build.log",
        )
        commands.append({"label": "restore pinned engine", **command_for_evidence(record)})
        require_ok(record, "构建固定 engine")
        built_engine = LOCAL / "target" / "engine-bootstrap" / "release" / "mm-engine"
        ENGINE.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(built_engine, ENGINE)

    after = sha256_tree(strategy)
    if before is not None and before != after:
        raise ToolFailure("bootstrap 改变了已有 strategy 源码哈希", EXIT_EVIDENCE)

    mm_lock = read_mm_lock()
    actual_engine = mm_lock.get("sources", {}).get("engine", {}).get("rev")
    actual_starter = mm_lock.get("sources", {}).get("starterpack", {}).get("rev")
    expected_engine = expected["sources"]["engine"]["commit"]
    expected_starter = expected["sources"]["starterpack"]["commit"]
    if (actual_engine, actual_starter) != (expected_engine, expected_starter):
        raise ToolFailure(
            f"bot 上游锁不匹配：engine={actual_engine}, starterpack={actual_starter}",
            EXIT_EVIDENCE,
        )
    expected_binary = expected["engine_binary"]["sha256"]
    if not ENGINE.exists() or sha256_file(ENGINE) != expected_binary:
        raise ToolFailure("官方 engine 二进制缺失或哈希不匹配", EXIT_EVIDENCE)
    if not MM_CLI.exists() or sha256_file(MM_CLI) != expected["cli_binary"]["sha256"]:
        raise ToolFailure("官方 mm-cli 二进制缺失或哈希不匹配", EXIT_EVIDENCE)
    if sha256_file(mm_lock_path) != expected["engine_binary"]["source_lock_sha256"]:
        raise ToolFailure("官方 .mm/lock.toml 缺失或哈希不匹配", EXIT_EVIDENCE)

    result = {
        "schema_version": 1,
        "generated_at": utc_now(),
        "status": "ok",
        "strategy_sha256_before": before,
        "strategy_sha256_after": after,
        "strategy_preserved": before is None or before == after,
        "engine_commit": actual_engine,
        "starterpack_commit": actual_starter,
        "source_actions": source_actions,
        "restored_scaffold": restored_scaffold,
        "commands": commands,
    }
    write_json(RECORDS / "bootstrap-latest.json", result)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT_OK


def test_all(_: argparse.Namespace) -> int:
    log_root = RECORDS / "logs" / f"test-{stamp()}"
    commands: list[dict[str, Any]] = []
    rust_sources = sorted(
        str(path)
        for root in [
            ROOT / "bot" / "src" / "strategy",
            ROOT / "lab" / "rules" / "src",
            ROOT / "lab" / "rules" / "tests",
            ROOT / "lab" / "opponents",
        ]
        for path in root.rglob("*.rs")
    )
    specs = [
        (
            "strategy rustfmt check",
            ["rustfmt", "+stable", "--edition", "2021", "--check", *rust_sources],
            ROOT,
            tool_env(),
            log_root / "rustfmt-strategy.log",
        ),
        (
            "bot tests",
            cargo_command("test", BOT_MANIFEST),
            ROOT,
            tool_env("bot"),
            log_root / "bot-tests.log",
        ),
        (
            "rules tests",
            cargo_command("test", RULES_MANIFEST),
            ROOT,
            tool_env("rules"),
            log_root / "rules-tests.log",
        ),
        (
            "bot release build",
            cargo_command("build", BOT_MANIFEST, "--release"),
            ROOT,
            tool_env("bot"),
            log_root / "bot-release.log",
        ),
        (
            "git diff check",
            ["git", "diff", "--check"],
            ROOT,
            tool_env(),
            log_root / "git-diff-check.log",
        ),
    ]

    raw_outputs: list[str] = []
    for label, argv, cwd, env, log_path in specs:
        record = run_command(
            argv,
            cwd=cwd,
            env=env,
            timeout=900,
            log_path=log_path,
            show_output=label in {"bot tests", "rules tests"},
        )
        raw_outputs.append(record["output"])
        commands.append({"label": label, **command_for_evidence(record)})
        require_ok(record, label)

    totals = parse_test_totals(raw_outputs)
    combined_output = "\n".join(raw_outputs)
    required_contracts = [
        "r01_", "r02_", "r03_", "r04_", "r05_", "r06_", "r07_", "r08_",
        "r09_", "r10_", "r13_", "r16_", "r17_", "r18_", "r19_", "r20_", "r24_",
    ]
    missing_contracts = [name for name in required_contracts if f"test {name}" not in combined_output]
    if totals["failed"] != 0 or totals["passed"] < 21 or missing_contracts:
        raise ToolFailure(
            f"测试覆盖异常：totals={totals}, missing_contracts={missing_contracts}",
            EXIT_EVIDENCE,
        )

    result = {
        "schema_version": 1,
        "generated_at": utc_now(),
        "status": "ok",
        "tests": totals,
        "required_contracts": required_contracts,
        "inputs": input_fingerprints(),
        "commands": commands,
    }
    write_json(RECORDS / "test-latest.json", result)
    print(json.dumps({"status": "ok", "tests": totals}, ensure_ascii=False))
    return EXIT_OK


def materialize_opponent(opponent: dict[str, Any], commands: list[dict[str, Any]]) -> Path:
    source = ROOT / opponent["source_path"]
    if not source.exists():
        raise ToolFailure(f"缺少对手源 {rel(source)}")
    project = ROOT / opponent["build_dir"]
    starter = LOCAL / "upstream" / "rust-starterpack"
    if not starter.exists():
        raise ToolFailure("缺少 .local/upstream/rust-starterpack；先运行 bootstrap/上游恢复")
    shutil.copytree(starter, project, dirs_exist_ok=True)
    destination = project / "src" / "strategy" / "main.rs"
    shutil.copy2(source, destination)
    if sha256_file(source) != sha256_file(destination):
        raise ToolFailure(f"对手源复制校验失败：{opponent['id']}", EXIT_EVIDENCE)

    target_name = f"opponents/{opponent['id']}"
    log_path = RECORDS / "logs" / "opponents" / f"{opponent['id']}.log"
    record = run_command(
        cargo_command("build", project / "Cargo.toml", "--release"),
        env=tool_env(target_name),
        timeout=900,
        log_path=log_path,
    )
    commands.append({"label": f"build opponent {opponent['id']}", **command_for_evidence(record)})
    require_ok(record, f"构建对手 {opponent['id']}")
    binary = LOCAL / "target" / target_name / "release" / "bot"
    if not binary.exists():
        raise ToolFailure(f"找不到对手二进制 {rel(binary)}", EXIT_EVIDENCE)
    return binary


def parse_game_log(path: Path) -> tuple[dict[str, Any] | None, dict[str, Any] | None, str | None]:
    config = result = None
    elapsed = None
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        for line_number, line in enumerate(handle):
            stripped = line.strip()
            if line_number == 0:
                try:
                    config = json.loads(stripped)
                except json.JSONDecodeError:
                    config = None
            elif stripped.startswith("# result: "):
                try:
                    result = json.loads(stripped.removeprefix("# result: "))
                except json.JSONDecodeError:
                    result = None
            elif stripped.startswith("# time elapsed: "):
                elapsed = stripped.removeprefix("# time elapsed: ")
    return config, result, elapsed


def parse_bot_summaries(path: Path) -> list[dict[str, Any]]:
    summaries: list[dict[str, Any]] = []
    marker = "MM32_SUMMARY "
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if marker not in line:
                continue
            try:
                summaries.append(json.loads(line.split(marker, 1)[1]))
            except json.JSONDecodeError:
                pass
    return summaries


def contains_fatal(path: Path) -> bool:
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        return any("###" in line for line in handle)


def file_evidence(path: Path) -> dict[str, Any]:
    return {"path": rel(path), "sha256": sha256_file(path), "bytes": path.stat().st_size}


def aggregate_matches(matches: list[dict[str, Any]]) -> tuple[dict[str, Any], dict[str, Any]]:
    by_opponent: dict[str, dict[str, Any]] = {}
    summaries: list[dict[str, Any]] = []
    for match in matches:
        bucket = by_opponent.setdefault(
            match["opponent"], {"games": 0, "wins": 0, "draws": 0, "losses": 0, "points": 0}
        )
        bucket["games"] += 1
        outcome = match["candidate_outcome"]
        if outcome == "win":
            bucket["wins"] += 1
            bucket["points"] += 3
        elif outcome == "draw":
            bucket["draws"] += 1
            bucket["points"] += 1
        else:
            bucket["losses"] += 1
        summaries.extend(match.get("candidate_telemetry", []))
    for bucket in by_opponent.values():
        bucket["average_points"] = bucket["points"] / bucket["games"] if bucket["games"] else None

    observations = sum(int(item.get("observed_charges", 0)) for item in summaries)
    weighted_sum = sum(
        float(item.get("charge_mean", 0.0)) * int(item.get("observed_charges", 0))
        for item in summaries
    )
    budget = {
        "summaries": len(summaries),
        "calls": sum(int(item.get("calls", 0)) for item in summaries),
        "observed_charges": observations,
        "charge_mean_weighted": weighted_sum / observations if observations else None,
        "charge_p95_max": max((item.get("charge_p95") for item in summaries), default=None),
        "charge_p99_max": max((item.get("charge_p99") for item in summaries), default=None),
        "charge_peak": max((item.get("charge_peak") for item in summaries), default=None),
        "min_remaining": min((item.get("min_remaining") for item in summaries), default=None),
        "skipped_ticks": sum(int(item.get("skipped_ticks", 0)) for item in summaries),
        "low_budget_ticks": sum(int(item.get("low_budget_ticks", 0)) for item in summaries),
    }
    return by_opponent, budget


def smoke(_: argparse.Namespace) -> int:
    suite = load_json(SUITE_PATH)
    matrix = suite.get("matches", [])
    expected = int(suite.get("expected_matches", -1))
    if expected != 8 or len(matrix) != 8:
        raise ToolFailure("smoke suite 必须恰好包含 8 局", EXIT_EVIDENCE)
    if not suite.get("time_enforcement") or suite.get("no_time_limit"):
        raise ToolFailure("正式 smoke 必须启用时间限制且禁止 --no-time-limit", EXIT_EVIDENCE)
    ids = [match["id"] for match in matrix]
    if len(set(ids)) != len(ids):
        raise ToolFailure("smoke match id 不唯一", EXIT_EVIDENCE)
    opponent_ids = {item["id"] for item in suite.get("opponents", [])}
    expected_cells = {
        (opponent, side, repetition)
        for opponent in opponent_ids
        for side in ("a", "b")
        for repetition in (1, 2)
    }
    actual_cells = collections.Counter(
        (match.get("opponent"), match.get("candidate_side"), match.get("repetition"))
        for match in matrix
    )
    if opponent_ids != {"starter_symmetric", "rush_simple"} or set(actual_cells) != expected_cells:
        raise ToolFailure("smoke suite 必须严格覆盖 2 对手 × 2 方向 × 2 重复", EXIT_EVIDENCE)
    if any(count != 1 for count in actual_cells.values()):
        raise ToolFailure("smoke suite 的笛卡尔单元必须各出现一次", EXIT_EVIDENCE)

    current_inputs = input_fingerprints()
    test_path = RECORDS / "test-latest.json"
    if not test_path.exists():
        raise ToolFailure("正式 smoke 前必须运行本轮 test", EXIT_EVIDENCE)
    test_record = load_json(test_path)
    if test_record.get("status") != "ok" or test_record.get("inputs") != current_inputs:
        raise ToolFailure("test 证据与当前源码/配置哈希不匹配；请先重跑 test", EXIT_EVIDENCE)

    commands: list[dict[str, Any]] = []
    candidate_build = run_command(
        cargo_command("build", BOT_MANIFEST, "--release"),
        env=tool_env("bot"),
        timeout=900,
        log_path=RECORDS / "logs" / "smoke-candidate-build.log",
    )
    commands.append({"label": "build candidate", **command_for_evidence(candidate_build)})
    require_ok(candidate_build, "构建候选 bot")
    candidate = LOCAL / "target" / "bot" / "release" / "bot"

    opponents: dict[str, Path] = {}
    opponent_specs = {item["id"]: item for item in suite["opponents"]}
    for opponent in suite["opponents"]:
        opponents[opponent["id"]] = materialize_opponent(opponent, commands)

    batch = ROOT / suite["artifacts"]["run_root"] / f"run-{stamp()}"
    batch.mkdir(parents=True, exist_ok=False)
    matches: list[dict[str, Any]] = []
    progress_path = RECORDS / "smoke-progress-latest.json"
    write_json(
        progress_path,
        {
            "schema_version": 1,
            "generated_at": utc_now(),
            "status": "running",
            "run_root": rel(batch),
            "inputs": current_inputs,
            "formal_matches_expected": expected,
            "formal_matches_completed": 0,
            "matches": [],
        },
    )
    config_expected_sha = load_json(ROOT / "configs" / "baseline.json")["game_config_fixture"]["sha256"]

    for index, spec in enumerate(matrix, start=1):
        opponent = opponents[spec["opponent"]]
        candidate_side = spec["candidate_side"].upper()
        if candidate_side not in {"A", "B"}:
            raise ToolFailure(f"非法 candidate_side: {candidate_side}", EXIT_EVIDENCE)
        bot_a, bot_b = (candidate, opponent) if candidate_side == "A" else (opponent, candidate)
        match_dir = batch / spec["id"]
        match_dir.mkdir(parents=True)
        gamelog = match_dir / "game.mmgl"
        a_log = match_dir / "a.log"
        b_log = match_dir / "b.log"
        runner_log = match_dir / "runner.log"
        print(f"[mm32] formal match {index}/8: {spec['id']}", flush=True)
        argv = [
            str(ENGINE),
            str(bot_a),
            str(bot_b),
            "-o",
            f"g:{gamelog}",
            "-o",
            f"a,ae:{a_log}",
            "-o",
            f"b,be:{b_log}",
        ]
        record = run_command(
            argv,
            timeout=int(suite["match_timeout_seconds"]),
            log_path=runner_log,
        )
        commands.append({"label": spec["id"], **command_for_evidence(record)})

        if not all(path.exists() for path in (gamelog, a_log, b_log, runner_log)):
            raise ToolFailure(f"比赛 {spec['id']} 缺少日志", EXIT_EVIDENCE)
        config, result, engine_elapsed = parse_game_log(gamelog)
        fatal = contains_fatal(a_log) or contains_fatal(b_log) or "###" in record["output"]
        candidate_log = a_log if candidate_side == "A" else b_log
        telemetry = parse_bot_summaries(candidate_log)
        valid = record["exit_code"] == 0 and result is not None and not fatal and len(telemetry) == 1

        config_line = json.dumps(config, ensure_ascii=False, separators=(",", ":")) + "\n" if config else ""
        config_sha = hashlib.sha256(config_line.encode()).hexdigest() if config_line else None
        if config_sha != config_expected_sha:
            valid = False

        winner = result.get("winner") if result else None
        if winner is None:
            outcome = "draw"
        elif winner == candidate_side:
            outcome = "win"
        else:
            outcome = "loss"
        entry = {
            **spec,
            "candidate_side": candidate_side.lower(),
            "valid": valid,
            "exit_code": record["exit_code"],
            "wall_seconds": record["duration_seconds"],
            "game_config": config,
            "engine_result": result,
            "result_line_present": result is not None,
            "official_error_log_clean": not fatal,
            "winner": winner,
            "candidate_outcome": outcome,
            "end_reason": result.get("reason") if result else None,
            "final_tick": result.get("tick") if result else None,
            "engine_elapsed": engine_elapsed,
            "time_enforcement": True,
            "fatal_marker": fatal,
            "config_sha256": config_sha,
            "candidate_binary_sha256": sha256_file(candidate),
            "opponent_binary_sha256": sha256_file(opponent),
            "candidate_telemetry": telemetry,
            "callback_coverage_complete": bool(
                result
                and len(telemetry) == 1
                and telemetry[0].get("first_tick") == 0
                and telemetry[0].get("last_tick") == result.get("tick", 0) - 1
                and telemetry[0].get("calls") == result.get("tick")
            ),
            "logs": {
                "gamelog": file_evidence(gamelog),
                "bot_a": file_evidence(a_log),
                "bot_b": file_evidence(b_log),
                "runner": file_evidence(runner_log),
            },
        }
        matches.append(entry)
        write_json(
            progress_path,
            {
                "schema_version": 1,
                "generated_at": utc_now(),
                "status": "running" if valid and index < expected else ("ok" if valid else "failed"),
                "run_root": rel(batch),
                "inputs": current_inputs,
                "formal_matches_expected": expected,
                "formal_matches_completed": sum(1 for match in matches if match["valid"]),
                "matches": matches,
            },
        )
        if not valid:
            raise ToolFailure(f"比赛 {spec['id']} 证据不完整或包含 fatal error", EXIT_EVIDENCE)

    by_opponent, budget = aggregate_matches(matches)
    metrics = {
        "schema_version": 1,
        "round": 1,
        "status": "ok",
        "generated_at": utc_now(),
        "inputs": current_inputs,
        "tests": test_record["tests"],
        "formal_matches_expected": expected,
        "formal_matches_completed": sum(1 for match in matches if match["valid"]),
        "seed": suite["seed"],
        "matches": matches,
        "by_opponent": by_opponent,
        "budget": budget,
        "manual_selfplay": {
            "count": 1 if (LOCAL / "runs" / "manual-selfplay" / "game.mmgl").exists() else 0,
            "path": ".local/runs/manual-selfplay",
            "included_in_formal_metrics": False,
        },
    }
    write_json(ARTIFACTS / "metrics.json", metrics)

    upstream = load_json(UPSTREAM_LOCK)
    repo = repository_facts()
    manifest = {
        "schema_version": 1,
        "round": 1,
        "generated_at": utc_now(),
        "repository": repo,
        "evaluated_code_commit": evaluated_code_commit(repo),
        "working_tree_strategy_sha256": sha256_tree(ROOT / "bot" / "src" / "strategy"),
        "inputs": current_inputs,
        "upstream": upstream,
        "environment": {
            "os": platform.system(),
            "os_release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "rust_toolchain_used": "stable",
            "pinned_nightly_diagnostic": ".local/diagnostics/nightly-bot-test.log",
        },
        "binaries": {
            "engine": file_evidence(ENGINE),
            "candidate": file_evidence(candidate),
            "opponents": {name: file_evidence(path) for name, path in opponents.items()},
        },
        "configs": {
            "upstream_lock": file_evidence(UPSTREAM_LOCK),
            "baseline": file_evidence(ROOT / "configs" / "baseline.json"),
            "smoke_suite": file_evidence(SUITE_PATH),
            "prompt": file_evidence(ROOT / "docs" / "prompts" / "mechmania32_prompt1.md"),
            "plan": file_evidence(ROOT / "docs" / "plans" / "mechmania32_plan1.md"),
        },
        "seed": suite["seed"],
        "time_enforcement": True,
        "formal_match_count": len(matches),
        "run_root": rel(batch),
        "workflow_records": {
            name: file_evidence(RECORDS / f"{name}-latest.json")
            for name in ("doctor", "bootstrap", "test")
            if (RECORDS / f"{name}-latest.json").exists()
        },
        "commands": (test_record.get("commands", []) if test_record else []) + commands,
    }
    write_json(ARTIFACTS / "manifest.json", manifest)
    print(json.dumps({"completed": len(matches), "by_opponent": by_opponent, "budget": budget}, ensure_ascii=False, indent=2))
    return EXIT_OK


def packcheck(_: argparse.Namespace) -> int:
    starter = LOCAL / "upstream" / "rust-starterpack"
    strategy = ROOT / "bot" / "src" / "strategy"
    if not starter.exists() or not strategy.exists():
        raise ToolFailure("packcheck 缺少 starterpack 或 strategy")
    temp_root = LOCAL / "tmp"
    temp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="packcheck-", dir=temp_root) as temporary:
        project = Path(temporary) / "bot"
        shutil.copytree(starter, project)
        copied = project / "src" / "strategy"
        shutil.rmtree(copied)
        shutil.copytree(strategy, copied)
        source_hash = sha256_tree(strategy)
        copied_hash = sha256_tree(copied)
        if source_hash != copied_hash:
            raise ToolFailure("packcheck 策略复制哈希不一致", EXIT_EVIDENCE)
        target_name = f"packcheck/{stamp()}"
        record = run_command(
            cargo_command("build", project / "Cargo.toml", "--release"),
            env=tool_env(target_name),
            timeout=900,
            log_path=RECORDS / "logs" / "packcheck.log",
            show_output=True,
        )
        require_ok(record, "packcheck release build")
        binary = LOCAL / "target" / target_name / "release" / "bot"
        result = {
            "schema_version": 1,
            "generated_at": utc_now(),
            "status": "ok",
            "declared_upstream_toolchain": "nightly-2024-07-25",
            "build_toolchain_used": "stable",
            "toolchain_override_reason": (
                "The pinned nightly Cargo cannot parse current locked edition-2024 dependencies; "
                "see .local/diagnostics/nightly-bot-test.log."
            ),
            "strategy_source_sha256": source_hash,
            "strategy_copied_sha256": copied_hash,
            "only_strategy_restored": True,
            "server_submission_validation": "not_run_requires_shi_authorization",
            "binary": file_evidence(binary),
            "command": command_for_evidence(record),
        }
    write_json(RECORDS / "packcheck-latest.json", result)
    manifest_path = ARTIFACTS / "manifest.json"
    if manifest_path.exists():
        manifest = load_json(manifest_path)
        manifest["packcheck"] = result
        manifest.setdefault("commands", []).append(
            {"label": "packcheck release build", **result["command"]}
        )
        write_json(manifest_path, manifest)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT_OK


def candidate_files() -> list[Path]:
    record = run_command(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], timeout=30
    )
    require_ok(record, "枚举 Git 候选文件")
    return [ROOT / item for item in record["output"].split("\0") if item]


def scan_secrets(paths: list[Path]) -> list[dict[str, Any]]:
    patterns = [
        re.compile(rb"ghp_[A-Za-z0-9]{20,}"),
        re.compile(rb"github_pat_[A-Za-z0-9_]{20,}"),
        re.compile(rb"AKIA[0-9A-Z]{16}"),
        re.compile(rb"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        re.compile(rb"https?://[^\s/:@]+:[^\s/@]+@"),
        re.compile(rb"(?i)password\s*[:=]\s*['\"][^'\"]{6,}['\"]"),
    ]
    findings: list[dict[str, Any]] = []
    for path in paths:
        if not path.is_file() or path.stat().st_size > 5 * 1024 * 1024:
            continue
        data = path.read_bytes()
        if any(pattern.search(data) for pattern in patterns):
            findings.append({"path": rel(path), "reason": "credential-like pattern"})
    return findings


def scan_source_whitespace(paths: list[Path]) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    checked_suffixes = {".rs", ".py", ".json", ".toml"}
    for path in paths:
        if not path.is_file() or (path.suffix not in checked_suffixes and path.name != ".gitignore"):
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        trailing = [index for index, line in enumerate(content.splitlines(), start=1) if line.rstrip() != line]
        if trailing:
            findings.append({"path": rel(path), "reason": "trailing whitespace", "lines": trailing[:20]})
        if content and not content.endswith("\n"):
            findings.append({"path": rel(path), "reason": "missing final newline"})
    return findings


def audit(_: argparse.Namespace) -> int:
    required = [
        ROOT / "README.md",
        ROOT / "bot" / "mm-config.toml",
        ROOT / "bot" / "src" / "strategy" / "controller.rs",
        RULES_MANIFEST,
        ROOT / "lab" / "rules" / "Cargo.lock",
        ROOT / "lab" / "rules" / "tests" / "contracts.rs",
        UPSTREAM_LOCK,
        ROOT / "configs" / "baseline.json",
        SUITE_PATH,
        ROOT / "docs" / "rules" / "mechmania32_rules1.md",
        ROOT / "docs" / "reports" / "mechmania32_report1.md",
        ARTIFACTS / "manifest.json",
        ARTIFACTS / "metrics.json",
        ROOT / "artifacts" / "feedback" / "latest.json",
    ]
    missing = [rel(path) for path in required if not path.exists()]
    json_paths = [UPSTREAM_LOCK, ROOT / "configs" / "baseline.json", SUITE_PATH]
    json_paths.extend(
        path
        for path in [
            ARTIFACTS / "manifest.json",
            ARTIFACTS / "metrics.json",
            ROOT / "artifacts" / "feedback" / "latest.json",
        ]
        if path.exists()
    )
    invalid_json: list[str] = []
    for path in json_paths:
        try:
            json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            invalid_json.append(rel(path))

    files = candidate_files()
    secret_findings = scan_secrets(files)
    whitespace_findings = scan_source_whitespace(files)
    large_files = [
        {"path": rel(path), "bytes": path.stat().st_size}
        for path in files
        if path.is_file() and path.stat().st_size > 1_000_000
    ]
    ignored_checks: dict[str, bool] = {}
    for item in [".local/bin/mm-cli", "bot/Cargo.toml", "bot/Cargo.lock", "bot/.mm/bin/mm-engine", "bot/src/core.rs"]:
        record = run_command(["git", "check-ignore", "-q", item], timeout=30)
        ignored_checks[item] = record["exit_code"] == 0

    diff_check = run_command(["git", "diff", "--check"], timeout=30)
    pack_record = load_json(RECORDS / "packcheck-latest.json") if (RECORDS / "packcheck-latest.json").exists() else None
    bootstrap_record = load_json(RECORDS / "bootstrap-latest.json") if (RECORDS / "bootstrap-latest.json").exists() else None
    test_record = load_json(RECORDS / "test-latest.json") if (RECORDS / "test-latest.json").exists() else None
    metrics = load_json(ARTIFACTS / "metrics.json") if (ARTIFACTS / "metrics.json").exists() else None
    manifest = load_json(ARTIFACTS / "manifest.json") if (ARTIFACTS / "manifest.json").exists() else None
    upstream = load_json(UPSTREAM_LOCK)
    current_inputs = input_fingerprints()
    expected_combo = (
        upstream["bot_dependency"]["locked_rev"],
        upstream["engine_binary"]["source_commit"],
    )
    mm_lock = read_mm_lock()
    cargo_lock_text = (ROOT / "bot" / "Cargo.lock").read_text(encoding="utf-8")
    live_version_checks = {
        "expected_client_commit": expected_combo[0] == "879578d448251d0acc7f10eef3d7fe5450758096",
        "expected_runtime_commit": expected_combo[1] == "4cbd3467e6617e330d42cb78ae3327ab2c88926a",
        "client_commit_in_cargo_lock": expected_combo[0] in cargo_lock_text,
        "bot_manifest_sha256": sha256_file(BOT_MANIFEST) == upstream["bot_dependency"]["manifest_sha256"],
        "bot_lockfile_sha256": sha256_file(ROOT / "bot" / "Cargo.lock")
        == upstream["bot_dependency"]["lockfile_sha256"],
        "mm_lock_sha256": sha256_file(ROOT / "bot" / ".mm" / "lock.toml")
        == upstream["engine_binary"]["source_lock_sha256"],
        "mm_lock_engine_commit": mm_lock.get("sources", {}).get("engine", {}).get("rev")
        == upstream["engine_binary"]["source_commit"],
        "mm_lock_starter_commit": mm_lock.get("sources", {}).get("starterpack", {}).get("rev")
        == upstream["sources"]["starterpack"]["commit"],
        "engine_binary_sha256": ENGINE.exists()
        and sha256_file(ENGINE) == upstream["engine_binary"]["sha256"],
        "cli_binary_sha256": MM_CLI.exists()
        and sha256_file(MM_CLI) == upstream["cli_binary"]["sha256"],
        "compatibility_audited": upstream["client_runtime_compatibility"]["status"] == "accepted",
    }
    for source in ("engine", "wiki", "starterpack", "cli"):
        archive = ROOT / upstream["sources"][source]["local_archive"]
        live_version_checks[f"{source}_archive_sha256"] = (
            archive.exists() and sha256_file(archive) == upstream["sources"][source]["archive_sha256"]
        )
    versions_ok = all(live_version_checks.values())

    evidence_checks = {
        "bootstrap_preserved_current_strategy": bool(
            bootstrap_record
            and bootstrap_record.get("status") == "ok"
            and bootstrap_record.get("strategy_preserved") is True
            and bootstrap_record.get("strategy_sha256_after") == current_inputs["strategy_tree"]
        ),
        "test_current": bool(
            test_record and test_record.get("status") == "ok" and test_record.get("inputs") == current_inputs
        ),
        "metrics_current": bool(
            metrics
            and metrics.get("status") == "ok"
            and metrics.get("inputs") == current_inputs
            and metrics.get("formal_matches_completed") == 8
            and len(metrics.get("matches", [])) == 8
            and all(match.get("valid") for match in metrics.get("matches", []))
        ),
        "manifest_current": bool(
            manifest
            and manifest.get("inputs") == current_inputs
            and manifest.get("working_tree_strategy_sha256") == current_inputs["strategy_tree"]
            and manifest.get("formal_match_count") == 8
        ),
        "packcheck_current": bool(
            pack_record
            and pack_record.get("status") == "ok"
            and pack_record.get("strategy_source_sha256") == current_inputs["strategy_tree"]
            and pack_record.get("strategy_source_sha256") == pack_record.get("strategy_copied_sha256")
        ),
    }
    latest_attempts: dict[str, Any] = {}
    for command in ("doctor", "bootstrap", "test", "smoke", "packcheck"):
        path = attempt_path(command)
        latest_attempts[command] = load_json(path) if path.exists() else None
    attempts_ok = all(
        record and record.get("status") == "ok" for record in latest_attempts.values()
    )

    failures = []
    if missing:
        failures.append("missing_required_paths")
    if invalid_json:
        failures.append("invalid_json")
    if secret_findings:
        failures.append("secret_findings")
    if large_files:
        failures.append("large_tracked_or_candidate_files")
    if whitespace_findings:
        failures.append("untracked_or_tracked_source_whitespace")
    if not all(ignored_checks.values()):
        failures.append("generated_boundary")
    if diff_check["exit_code"] != 0:
        failures.append("git_diff_check")
    if not pack_record or pack_record.get("status") != "ok":
        failures.append("packcheck")
    if not versions_ok:
        failures.append("version_combo")
    if not all(evidence_checks.values()):
        failures.append("stale_or_incomplete_evidence")
    if not attempts_ok:
        failures.append("latest_attempt_failed_or_missing")

    repo = repository_facts()
    evaluated_commit = evaluated_code_commit(repo)
    warnings = []
    if repo["status_porcelain"] and evaluated_commit:
        warnings.append("仅报告/机器证据晚于代码提交；evaluated_code_commit 绑定当前 HEAD")
    elif repo["status_porcelain"]:
        warnings.append("存在未提交的评估输入；evaluated_code_commit 必须保持 null")
    warnings.append("上游声明 nightly-2024-07-25，但当前 lock 依赖需 edition2024；构建与 packcheck 使用 stable")
    result = {
        "schema_version": 1,
        "round": 1,
        "generated_at": utc_now(),
        "status": "pass_with_warnings" if not failures else "fail",
        "failures": failures,
        "warnings": warnings,
        "required_paths": {rel(path): path.exists() for path in required},
        "invalid_json": invalid_json,
        "generated_paths_ignored": ignored_checks,
        "secret_findings": secret_findings,
        "large_files": large_files,
        "source_whitespace_findings": whitespace_findings,
        "git_diff_check": command_for_evidence(diff_check),
        "packcheck": pack_record,
        "evidence_checks": evidence_checks,
        "latest_attempts": latest_attempts,
        "version_combo": {
            "client": expected_combo[0],
            "runtime": expected_combo[1],
            "accepted": versions_ok,
            "live_checks": live_version_checks,
        },
        "repository": repo,
        "evaluated_code_commit": evaluated_commit,
        "formal_submission": "not_run_requires_shi_authorization",
        "branch_push_pr": "external_delivery_state_not_audited",
    }
    write_json(ARTIFACTS / "audit.json", result)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT_OK if not failures else EXIT_EVIDENCE


def parser() -> argparse.ArgumentParser:
    cli = argparse.ArgumentParser(description=__doc__)
    sub = cli.add_subparsers(dest="command", required=True)
    sub.add_parser("doctor", help="检查工具链、版本、平台和网络可用性").set_defaults(func=doctor)
    sub.add_parser("bootstrap", help="可重复恢复官方工程且保护策略源码").set_defaults(func=bootstrap)
    sub.add_parser("test", help="格式、bot、规则、release 与 diff 检查").set_defaults(func=test_all)
    sub.add_parser("smoke", help="串行运行正式计时 8 局 smoke").set_defaults(func=smoke)
    sub.add_parser("packcheck", help="干净官方工程中的策略打包闭包检查").set_defaults(func=packcheck)
    sub.add_parser("audit", help="目录、版本、秘密、大文件和证据审计").set_defaults(func=audit)
    return cli


def main() -> int:
    args = parser().parse_args()
    write_attempt(args.command, "running")
    try:
        code = int(args.func(args))
        write_attempt(args.command, "ok" if code == 0 else "failed", exit_code=code)
        return code
    except ToolFailure as exc:
        write_attempt(args.command, "failed", exit_code=exc.code, error=str(exc))
        print(f"[mm32] ERROR: {exc}", file=sys.stderr)
        return exc.code
    except KeyboardInterrupt:
        write_attempt(args.command, "failed", exit_code=130, error="interrupted")
        print("[mm32] ERROR: interrupted", file=sys.stderr)
        return 130
    except Exception as exc:  # defensive boundary for actionable CLI failures
        log_path = RECORDS / "logs" / f"{args.command}-unexpected.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text(traceback.format_exc(), encoding="utf-8")
        write_attempt(
            args.command,
            "failed",
            exit_code=1,
            error=f"{type(exc).__name__}: {exc}",
            traceback_log=file_evidence(log_path),
        )
        print(f"[mm32] ERROR: unexpected {type(exc).__name__}: {exc}; see {rel(log_path)}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
