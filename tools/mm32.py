#!/usr/bin/env python3
"""Reproducible local workflow for the MechMania 32 workspace.

The checked-in files describe source inputs.  Host-specific compiler selection,
build fingerprints, binaries, logs, and attempts live below ``.local``.
"""

from __future__ import annotations

import argparse
import datetime as dt
import difflib
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
UPSTREAM_LOCK = ROOT / "configs" / "upstream.lock.json"
TOOLCHAIN_LOCK = ROOT / "configs" / "toolchain.lock.json"
DEFAULT_SUITE_PATH = ROOT / "configs" / "suites" / "m02_regression.json"
BOT_MANIFEST = ROOT / "bot" / "Cargo.toml"
RULES_MANIFEST = ROOT / "lab" / "rules" / "Cargo.toml"
OPPONENTS_MANIFEST = ROOT / "lab" / "opponents" / "Cargo.toml"
M01_DELIVERY_COMMIT = "2894d8e514846c9265b609c348a8ca27d9415aa5"
ENGINE = ROOT / "bot" / ".mm" / "bin" / "mm-engine"
MM_CLI = ROOT / ".local" / "bin" / "mm-cli"
DEFAULT_ROUND = "m02"

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


def sha256_python_sources(path: Path) -> str:
    """Hash Python test sources without interpreter-generated ``__pycache__`` files."""
    digest = hashlib.sha256()
    for item in sorted(path.rglob("*.py")):
        relative = item.relative_to(path).as_posix().encode()
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        with item.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    return digest.hexdigest()


def canonical_sha256(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def file_sha_or_none(path: Path) -> str | None:
    return sha256_file(path) if path.is_file() else None


def recorded_file_matches(record: dict[str, Any] | None) -> bool:
    """Revalidate a ``file_evidence`` record against the file now on disk."""
    if not record or not isinstance(record.get("path"), str):
        return False
    path = resolve_repo_path(record["path"])
    return bool(
        path.is_file()
        and record.get("sha256") == sha256_file(path)
        and record.get("bytes") == path.stat().st_size
    )


def normalize_round(value: str | int) -> str:
    text = str(value).strip().lower()
    if text.startswith("m"):
        text = text[1:]
    if not text.isdigit() or int(text) < 1:
        raise ToolFailure(f"非法 round: {value!r}")
    return f"m{int(text):02d}"


def safe_id(value: str, label: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,95}", value):
        raise ToolFailure(f"非法 {label}: {value!r}")
    return value


def resolve_repo_path(value: str | Path) -> Path:
    path = Path(value)
    return path if path.is_absolute() else ROOT / path


def archived_source_tree_sha256(archive: Path) -> str:
    """Hash the single source tree carried by a pinned upstream archive."""
    LOCAL.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="source-tree-", dir=LOCAL) as temporary:
        extraction = Path(temporary)
        with tarfile.open(archive, "r:gz") as bundle:
            bundle.extractall(extraction, filter="data")
        roots = [path for path in extraction.iterdir() if path.is_dir()]
        if len(roots) != 1:
            raise ToolFailure(
                f"固定来源归档顶层目录数量异常: {rel(archive)}", EXIT_EVIDENCE
            )
        return sha256_tree(roots[0])


def validate_upstream_source_snapshots(
    upstream: dict[str, Any], names: Iterable[str] | None = None
) -> dict[str, bool]:
    """Verify each extracted checkout byte-for-byte against its pinned archive."""
    selected = set(names) if names is not None else set(upstream["sources"])
    checks: dict[str, bool] = {}
    for name, source in upstream["sources"].items():
        if name not in selected:
            continue
        archive = resolve_repo_path(source["local_archive"])
        checkout = resolve_repo_path(source["local_source"])
        archive_ok = archive.is_file() and sha256_file(archive) == source["archive_sha256"]
        checks[f"{name}_archive"] = archive_ok
        checkout_ok = checkout.is_dir()
        checks[f"{name}_checkout_present"] = checkout_ok
        tree_matches = False
        if archive_ok and checkout_ok:
            tree_matches = archived_source_tree_sha256(archive) == sha256_tree(checkout)
        checks[f"{name}_checkout_matches_archive_tree"] = tree_matches
        for locked_path, locked_sha in source.get("file_sha256", {}).items():
            path = checkout / locked_path
            checks[f"{name}:{locked_path}"] = (
                path.is_file() and sha256_file(path) == locked_sha
            )
    return checks


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


def _configured_path(value: str | None) -> str | None:
    if not value:
        return None
    return str(resolve_repo_path(value))


def _find_executable(
    name: str,
    config: dict[str, Any],
    environ: dict[str, str],
) -> tuple[str | None, str]:
    env_name = f"MM32_{name.upper()}"
    if environ.get(env_name):
        return environ[env_name], f"environment:{env_name}"

    configured = _configured_path(config.get("executables", {}).get(name))
    if configured:
        return configured, "toolchain_lock"

    caller = shutil.which(name, path=environ.get("PATH"))
    if caller:
        return caller, "caller_path"

    for raw_directory in config.get("shared_search_paths", []):
        candidate = Path(os.path.expandvars(os.path.expanduser(raw_directory))) / name
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate), "configured_shared_search"

    project_bin = resolve_repo_path(config.get("project_local_bin", ".local/toolchain/bin"))
    candidate = project_bin / name
    if candidate.is_file() and os.access(candidate, os.X_OK):
        return str(candidate), "project_local"
    return None, "missing"


def select_toolchain(environ: dict[str, str] | None = None) -> dict[str, Any]:
    """Resolve a reproducible toolchain without discarding the caller's PATH.

    Explicit MM32_* paths and checked-in executable paths take precedence.  The
    caller's PATH is then respected, followed by narrowly configured discovery
    locations and a project-local proxy directory.
    """
    caller = dict(os.environ if environ is None else environ)
    config = load_json(TOOLCHAIN_LOCK)
    executables: dict[str, str | None] = {}
    sources: dict[str, str] = {}
    for name in ("cargo", "rustc", "rustup", "rustfmt"):
        executable, source = _find_executable(name, config, caller)
        executables[name] = executable
        sources[name] = source

    channel = caller.get("MM32_RUST_TOOLCHAIN") or str(config["selection"]["channel"])
    profile = caller.get("MM32_BUILD_PROFILE") or str(config["selection"]["build_profile"])
    if profile not in {"debug", "release"}:
        raise ToolFailure(f"不支持的构建 profile: {profile}")

    configured_homes = config.get("homes", {})
    home_mode = configured_homes.get("mode", "caller_or_project_local")
    caller_toolchain = all(
        sources[name] == "caller_path" for name in ("cargo", "rustc")
    )
    caller_home = Path(caller.get("HOME", str(Path.home())))
    project_homes = configured_homes.get("project_local", {})

    def select_home(variable: str, default_name: str) -> tuple[str, str]:
        if caller.get(variable):
            return caller[variable], f"environment:{variable}"
        if home_mode == "caller" or (
            home_mode == "caller_or_project_local" and caller_toolchain
        ):
            return str(caller_home / default_name), "inferred_caller_default"
        configured = _configured_path(project_homes.get(default_name.removeprefix(".")))
        if configured is None:
            raise ToolFailure(
                f"toolchain.lock homes.project_local 缺少 {default_name.removeprefix('.')}"
            )
        return configured, "toolchain_lock_project_local"

    rustup_home, rustup_home_source = select_home("RUSTUP_HOME", ".rustup")
    cargo_home, cargo_home_source = select_home("CARGO_HOME", ".cargo")

    def scope(path: str | None) -> str:
        if path is None:
            return "missing"
        try:
            Path(path).resolve().relative_to(LOCAL.resolve())
            return "project_local"
        except ValueError:
            return "shared"

    overrides = {
        key: caller[key]
        for key in (
            "MM32_CARGO",
            "MM32_RUSTC",
            "MM32_RUSTUP",
            "MM32_RUSTFMT",
            "MM32_RUST_TOOLCHAIN",
            "MM32_BUILD_PROFILE",
            "RUSTUP_HOME",
            "CARGO_HOME",
            "CARGO_NET_OFFLINE",
        )
        if key in caller
    }
    return {
        "schema_version": 1,
        "channel": channel,
        "build_profile": profile,
        "executables": executables,
        "executable_sources": sources,
        "executable_scope": {name: scope(path) for name, path in executables.items()},
        "rustup_home": rustup_home,
        "cargo_home": cargo_home,
        "rustup_home_scope": scope(rustup_home),
        "cargo_home_scope": scope(cargo_home),
        "rustup_home_source": rustup_home_source,
        "cargo_home_source": cargo_home_source,
        "caller_toolchain_detected": caller_toolchain,
        "caller_path": caller.get("PATH", ""),
        "environment_overrides": overrides,
        "lock_path": rel(TOOLCHAIN_LOCK),
        "lock_sha256": sha256_file(TOOLCHAIN_LOCK),
        "official_declared_channel": config["official_declared_toolchain"]["channel"],
        "official_compatibility": config["official_declared_toolchain"]["compatibility"],
    }


def tool_env(
    target_name: str | None = None,
    selection: dict[str, Any] | None = None,
) -> dict[str, str]:
    selection = selection or select_toolchain()
    env = os.environ.copy()
    caller_path = env.get("PATH", "")
    path_parts = [str(LOCAL / "bin")]
    path_parts.extend(
        str(Path(path).parent)
        for path in selection["executables"].values()
        if path is not None
    )
    path_parts.extend(part for part in caller_path.split(os.pathsep) if part)
    env["PATH"] = os.pathsep.join(dict.fromkeys(path_parts))
    env["RUSTUP_HOME"] = selection["rustup_home"]
    env["CARGO_HOME"] = selection["cargo_home"]
    if target_name:
        env["CARGO_TARGET_DIR"] = str(LOCAL / "target" / target_name)
    env["GIT_TERMINAL_PROMPT"] = "0"
    return env


def redact_arg(value: str) -> str:
    value = value.replace(str(ROOT), "$REPO")
    value = value.replace(str(Path.home()), "$HOME")
    value = re.sub(r"(https?://)[^/@\s]+:[^/@\s]+@", r"\1<redacted>@", value)
    if re.search(r"(?i)(token|password|secret)=", value):
        key = value.split("=", 1)[0]
        return f"{key}=<redacted>"
    return value


def redact_record(value: Any) -> Any:
    if isinstance(value, str):
        return redact_arg(value)
    if isinstance(value, list):
        return [redact_record(item) for item in value]
    if isinstance(value, dict):
        return {key: redact_record(item) for key, item in value.items() if key != "caller_path"}
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


def cargo_command(
    subcommand: str,
    manifest: Path,
    *extra: str,
    selection: dict[str, Any] | None = None,
) -> list[str]:
    selection = selection or select_toolchain()
    cargo = selection["executables"].get("cargo")
    if not cargo:
        raise ToolFailure("找不到 Cargo；可用 MM32_CARGO 或 configs/toolchain.lock.json 显式指定")
    profile_args = ["--release"] if selection["build_profile"] == "release" and subcommand == "build" else []
    channel_args = [f"+{selection['channel']}"] if selection["executables"].get("rustup") else []
    return [
        cargo,
        *channel_args,
        subcommand,
        "--locked",
        "--manifest-path",
        str(manifest),
        *profile_args,
        *extra,
    ]


def inspect_toolchain(selection: dict[str, Any]) -> dict[str, Any]:
    env = tool_env(selection=selection)
    commands: dict[str, dict[str, Any]] = {}
    channel_args = [f"+{selection['channel']}"] if selection["executables"].get("rustup") else []
    specs = {
        "rustc_vv": (selection["executables"].get("rustc"), [*channel_args, "-Vv"]),
        "cargo_version": (
            selection["executables"].get("cargo"),
            [*channel_args, "--version", "--verbose"],
        ),
        "rustup_active": (selection["executables"].get("rustup"), ["show", "active-toolchain"]),
    }
    for name, (executable, extra) in specs.items():
        if executable is None:
            commands[name] = {"exit_code": 127, "output": "missing executable"}
            continue
        record = run_command([executable, *extra], env=env, timeout=30)
        commands[name] = {
            "exit_code": record["exit_code"],
            "output": record["output"].strip(),
        }
    rustc_output = commands["rustc_vv"]["output"]
    host_match = re.search(r"^host:\s*(\S+)", rustc_output, re.MULTILINE)
    details = {
        **selection,
        "target_triple": host_match.group(1) if host_match else None,
        "os": platform.system(),
        "os_release": platform.release(),
        "architecture": platform.machine(),
        "commands": commands,
    }
    details["usable"] = all(
        commands[name]["exit_code"] == 0 for name in ("rustc_vv", "cargo_version")
    )
    details["compiler_fingerprint"] = canonical_sha256(
        {
            "channel": selection["channel"],
            "profile": selection["build_profile"],
            "rustc": commands["rustc_vv"],
            "cargo": commands["cargo_version"],
            "target": details["target_triple"],
        }
    )
    details["fingerprint"] = canonical_sha256(
        {
            "channel": selection["channel"],
            "profile": selection["build_profile"],
            "rustc": commands["rustc_vv"],
            "cargo": commands["cargo_version"],
            "target": details["target_triple"],
            "rustup_home_scope": selection["rustup_home_scope"],
            "cargo_home_scope": selection["cargo_home_scope"],
            "rustup_home_source": selection["rustup_home_source"],
            "cargo_home_source": selection["cargo_home_source"],
        }
    )
    return details


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
        "docs/mechmania32_index",
        "docs/reports/",
        "docs/rules/",
    )
    for line in repository.get("status_porcelain", []):
        path = line[3:] if len(line) >= 4 else line
        if " -> " in path:
            path = path.rsplit(" -> ", 1)[1]
        if not path.startswith(evidence_prefixes):
            return None
    return repository.get("head")


def suite_path_from_args(args: argparse.Namespace) -> Path:
    return resolve_repo_path(getattr(args, "suite", str(DEFAULT_SUITE_PATH)))


def load_and_validate_suite(path: Path, round_id: str) -> dict[str, Any]:
    suite = load_json(path)
    required = {
        "schema_version",
        "round",
        "suite_id",
        "time_enforcement",
        "match_timeout_seconds",
        "expected_matches",
        "candidates",
        "opponents",
        "matches",
        "fixture_path",
    }
    missing = sorted(required - suite.keys())
    if missing:
        raise ToolFailure(f"suite 缺少字段: {missing}", EXIT_EVIDENCE)
    suite_round = normalize_round(suite["round"])
    if suite_round != normalize_round(round_id):
        raise ToolFailure(f"suite round={suite_round} 与命令 round={round_id} 不一致", EXIT_EVIDENCE)
    safe_id(str(suite["suite_id"]), "suite_id")
    if suite.get("time_enforcement") is not True or suite.get("no_time_limit", False):
        raise ToolFailure("正式 suite 必须启用计时且禁止 no-time-limit", EXIT_EVIDENCE)
    matrix = suite["matches"]
    if int(suite["expected_matches"]) != len(matrix) or not matrix:
        raise ToolFailure("expected_matches 必须等于非空 matches 长度", EXIT_EVIDENCE)
    if int(suite["match_timeout_seconds"]) <= 0:
        raise ToolFailure("match_timeout_seconds 必须为正数", EXIT_EVIDENCE)

    candidates = {str(item["id"]): item for item in suite["candidates"]}
    opponents = {str(item["id"]): item for item in suite["opponents"]}
    if len(candidates) != len(suite["candidates"]) or len(opponents) != len(suite["opponents"]):
        raise ToolFailure("candidate/opponent id 必须唯一", EXIT_EVIDENCE)
    if not candidates or not opponents:
        raise ToolFailure("suite 至少需要一个 candidate 和一个 opponent", EXIT_EVIDENCE)
    ids: set[str] = set()
    cells: set[tuple[str, str, str, int]] = set()
    for match in matrix:
        match_id = safe_id(str(match.get("id", "")), "match id")
        if match_id in ids:
            raise ToolFailure(f"重复 match id: {match_id}", EXIT_EVIDENCE)
        ids.add(match_id)
        candidate = str(match.get("candidate", ""))
        opponent = str(match.get("opponent", ""))
        side = str(match.get("candidate_side", "")).lower()
        repetition = int(match.get("repetition", 0))
        if candidate not in candidates or opponent not in opponents:
            raise ToolFailure(f"比赛 {match_id} 引用了未知策略", EXIT_EVIDENCE)
        if side not in {"a", "b"} or repetition < 1:
            raise ToolFailure(f"比赛 {match_id} 的 side/repetition 非法", EXIT_EVIDENCE)
        cell = (candidate, opponent, side, repetition)
        if cell in cells:
            raise ToolFailure(f"重复 matrix cell: {cell}", EXIT_EVIDENCE)
        cells.add(cell)
    fixture = resolve_repo_path(str(suite["fixture_path"]))
    if not fixture.is_file():
        raise ToolFailure(f"suite fixture 不存在: {rel(fixture)}", EXIT_EVIDENCE)
    engine = suite.get("engine", {"kind": "locked_default"})
    if engine.get("kind") not in {"locked_default", "fixture_build"}:
        raise ToolFailure(f"suite engine.kind 非法: {engine.get('kind')!r}", EXIT_EVIDENCE)
    default_fixture = ROOT / "lab" / "fixtures" / "default_config.json"
    if engine.get("kind") == "locked_default" and sha256_file(fixture) != sha256_file(default_fixture):
        raise ToolFailure(
            "非默认 fixture 必须显式使用 engine.kind=fixture_build，避免只校验未实际加载的配置",
            EXIT_EVIDENCE,
        )
    if engine.get("kind") == "fixture_build" and engine.get("base_source_commit") is None:
        raise ToolFailure("fixture_build 必须声明 base_source_commit", EXIT_EVIDENCE)
    return suite


def _git_strategy_metadata(source: dict[str, Any]) -> dict[str, Any]:
    commit = str(source["commit"])
    path = str(source.get("path", "bot/src/strategy")).rstrip("/")
    resolved = git_value("rev-parse", f"{commit}^{{commit}}")
    tree_oid = git_value("rev-parse", f"{commit}:{path}")
    if resolved is None or tree_oid is None:
        raise ToolFailure(f"无法解析 Git strategy tree {commit}:{path}", EXIT_EVIDENCE)
    return {
        "kind": "git_strategy_tree",
        "requested_commit": commit,
        "resolved_commit": resolved,
        "path": path,
        "git_tree_oid": tree_oid,
    }


def strategy_source_metadata(spec: dict[str, Any]) -> dict[str, Any]:
    source = spec.get("source", {})
    kind = source.get("kind")
    if kind == "git_strategy_tree":
        return _git_strategy_metadata(source)
    if kind == "working_strategy_tree":
        path = resolve_repo_path(source.get("path", "bot/src/strategy"))
        if not path.is_dir():
            raise ToolFailure(f"策略目录不存在: {rel(path)}", EXIT_EVIDENCE)
        return {"kind": kind, "path": rel(path), "sha256": sha256_tree(path)}
    if kind == "single_file":
        path = resolve_repo_path(source["path"])
        if not path.is_file():
            raise ToolFailure(f"单文件对手不存在: {rel(path)}", EXIT_EVIDENCE)
        return {"kind": kind, "path": rel(path), "sha256": sha256_file(path)}
    raise ToolFailure(f"策略 {spec.get('id')} 的 source.kind 非法: {kind!r}", EXIT_EVIDENCE)


def suite_input_fingerprints(
    suite_path: Path,
    suite: dict[str, Any],
    selection: dict[str, Any] | None = None,
) -> dict[str, Any]:
    selection = selection or select_toolchain()
    toolchain = inspect_toolchain(selection)
    fixture = resolve_repo_path(str(suite["fixture_path"]))
    source_metadata = {
        f"candidate:{item['id']}": strategy_source_metadata(item)
        for item in suite["candidates"]
    }
    source_metadata.update(
        {
            f"opponent:{item['id']}": strategy_source_metadata(item)
            for item in suite["opponents"]
        }
    )
    upstream = load_json(UPSTREAM_LOCK)
    upstream_source_trees = {
        name: {
            "path": source["local_source"],
            "sha256": (
                sha256_tree(resolve_repo_path(source["local_source"]))
                if resolve_repo_path(source["local_source"]).is_dir()
                else None
            ),
        }
        for name, source in upstream["sources"].items()
    }
    values: dict[str, Any] = {
        "round": normalize_round(suite["round"]),
        "suite": {"path": rel(suite_path), "sha256": sha256_file(suite_path)},
        "fixture": {"path": rel(fixture), "sha256": sha256_file(fixture)},
        "workflow_tool": sha256_file(ROOT / "tools" / "mm32.py"),
        "workflow_tests": sha256_python_sources(ROOT / "tools" / "tests"),
        "toolchain_lock": sha256_file(TOOLCHAIN_LOCK),
        "toolchain_selection": redact_record(
            {
                "channel": selection["channel"],
                "build_profile": selection["build_profile"],
                "executables": selection["executables"],
                "rustup_home_scope": selection["rustup_home_scope"],
                "cargo_home_scope": selection["cargo_home_scope"],
                "rustup_home_source": selection["rustup_home_source"],
                "cargo_home_source": selection["cargo_home_source"],
                "environment_overrides": selection["environment_overrides"],
                "fingerprint": toolchain["fingerprint"],
                "target_triple": toolchain["target_triple"],
                "rustc": toolchain["commands"]["rustc_vv"],
                "cargo": toolchain["commands"]["cargo_version"],
            }
        ),
        "upstream_lock": sha256_file(UPSTREAM_LOCK),
        "upstream_source_trees": upstream_source_trees,
        "rules_tree": sha256_tree(ROOT / "lab" / "rules"),
        "opponents_test_tree": sha256_tree(ROOT / "lab" / "opponents"),
        "strategy_sources": source_metadata,
    }
    values["digest"] = canonical_sha256(values)
    return values


def evidence_is_current(
    record: dict[str, Any] | None,
    attempt: dict[str, Any] | None,
    inputs: dict[str, Any],
) -> bool:
    return bool(
        record
        and attempt
        and record.get("status") == "ok"
        and attempt.get("status") == "ok"
        and record.get("inputs") == inputs
        and attempt.get("inputs_digest") == inputs.get("digest")
    )


def parse_test_totals(outputs: Iterable[str]) -> dict[str, int]:
    passed = failed = ignored = 0
    pattern = re.compile(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored")
    for output in outputs:
        for match in pattern.finditer(output):
            passed += int(match.group(1))
            failed += int(match.group(2))
            ignored += int(match.group(3))
    return {"passed": passed, "failed": failed, "skipped": ignored}


def parse_unittest_totals(output: str, exit_code: int) -> dict[str, int]:
    match = re.search(r"Ran (\d+) tests?", output)
    passed = int(match.group(1)) if match and exit_code == 0 else 0
    skipped_match = re.search(r"OK \(skipped=(\d+)\)", output)
    skipped = int(skipped_match.group(1)) if skipped_match else 0
    return {"passed": max(0, passed - skipped), "failed": 0 if exit_code == 0 else 1, "skipped": skipped}


def doctor(args: argparse.Namespace) -> int:
    round_id = normalize_round(getattr(args, "round", DEFAULT_ROUND))
    suite_path = suite_path_from_args(args)
    suite = load_and_validate_suite(suite_path, round_id)
    selection = select_toolchain()
    details = inspect_toolchain(selection)
    paths = {
        rel(path): path.exists()
        for path in (UPSTREAM_LOCK, TOOLCHAIN_LOCK, suite_path, RULES_MANIFEST, OPPONENTS_MANIFEST)
    }
    generated = {
        rel(ENGINE): ENGINE.exists(),
        rel(MM_CLI): MM_CLI.exists(),
        rel(BOT_MANIFEST): BOT_MANIFEST.exists(),
    }
    git_record = run_command(["git", "--version"], timeout=30)
    local_status = "usable" if details["usable"] else "unusable"
    official = {
        "declared_channel": selection["official_declared_channel"],
        "status": selection["official_compatibility"]["status"],
        "reason": selection["official_compatibility"]["reason"],
        "used_for_local_builds": selection["channel"] == selection["official_declared_channel"],
    }
    checks: dict[str, Any] = {
        "schema_version": 2,
        "generated_at": utc_now(),
        "round": round_id,
        "suite": {"id": suite["suite_id"], "path": rel(suite_path)},
        "repository": repository_facts(),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "processor": platform.processor() or None,
        },
        "source_paths": paths,
        "generated_workspace_paths": generated,
        "local_selected_toolchain": {"status": local_status, **details},
        "official_declared_toolchain_compatibility": official,
        "git": {"exit_code": git_record["exit_code"], "output": git_record["output"].strip()},
    }
    network: dict[str, Any] = {"target": "github.com:443", "reachable": False, "error": None}
    try:
        with socket.create_connection(("github.com", 443), timeout=2):
            network["reachable"] = True
    except OSError as exc:
        network["error"] = str(exc)
    checks["network"] = network
    ok = all(paths.values()) and details["usable"] and git_record["exit_code"] == 0
    checks["workspace_ready"] = all(generated.values())
    checks["status"] = "ok" if ok else "missing_prerequisite"
    write_json(RECORDS / "doctor-latest.json", checks)
    args._attempt_inputs_digest = suite_input_fingerprints(suite_path, suite, selection)["digest"]
    print(json.dumps(checks, ensure_ascii=False, indent=2))
    return EXIT_OK if ok else EXIT_PRECONDITION


def ensure_toolchain_initialized(
    selection: dict[str, Any], commands: list[dict[str, Any]]
) -> dict[str, Any]:
    rustup = selection["executables"].get("rustup")
    if not rustup:
        if selection["rustup_home_scope"] == "project_local":
            raise ToolFailure("项目本地 toolchain 初始化需要 rustup；可用 MM32_RUSTUP 显式指定")
        return {
            "channel": selection["channel"],
            "rustup_home": selection["rustup_home"],
            "rustup_home_scope": selection["rustup_home_scope"],
            "initialized": False,
            "reason": "shared direct compiler selection does not require rustup initialization",
        }
    env = tool_env(selection=selection)
    list_record = run_command([rustup, "toolchain", "list"], env=env, timeout=60)
    commands.append({"label": "list rustup toolchains", **command_for_evidence(list_record)})
    require_ok(list_record, "读取 rustup toolchain")
    channel = selection["channel"]
    installed = any(line.split("-", 1)[0] == channel for line in list_record["output"].splitlines())
    initialized = False
    if not installed:
        install_record = run_command(
            [rustup, "toolchain", "install", channel, "--profile", "minimal", "--component", "rustfmt"],
            env=env,
            timeout=900,
            log_path=RECORDS / "logs" / "bootstrap-toolchain-install.log",
        )
        commands.append({"label": "initialize project toolchain", **command_for_evidence(install_record)})
        require_ok(install_record, "初始化本地 Rust 工具链")
        initialized = True
    return {
        "channel": channel,
        "rustup_home": selection["rustup_home"],
        "rustup_home_scope": selection["rustup_home_scope"],
        "initialized": initialized,
    }


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
        for locked_path, locked_sha in source.get("file_sha256", {}).items():
            actual_path = checkout / locked_path
            if not actual_path.is_file() or sha256_file(actual_path) != locked_sha:
                raise ToolFailure(
                    f"固定 {name} 源文件哈希不匹配: {locked_path}", EXIT_EVIDENCE
                )
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


def build_record_path(component: str) -> Path:
    return RECORDS / "builds" / f"{component}.json"


def ensure_component_build(
    component: str,
    build_spec: dict[str, Any],
    selection: dict[str, Any],
    toolchain: dict[str, Any],
    commands: list[dict[str, Any]],
) -> dict[str, Any]:
    source = resolve_repo_path(build_spec["source_path"])
    manifest = source / "Cargo.toml"
    lockfile = source / "Cargo.lock"
    destination = resolve_repo_path(build_spec["output_path"])
    profile = selection["build_profile"]
    source_fingerprint = {
        "source_commit": build_spec["source_commit"],
        "source_tree_sha256": sha256_tree(source),
        "manifest_sha256": sha256_file(manifest),
        "cargo_lock_sha256": sha256_file(lockfile),
        "binary_name": build_spec["binary_name"],
        "profile": profile,
        "toolchain_fingerprint": toolchain["fingerprint"],
        "target_triple": toolchain["target_triple"],
    }
    fingerprint = canonical_sha256(source_fingerprint)
    record_path = build_record_path(component)
    previous = load_json(record_path) if record_path.exists() else None
    reusable = bool(
        previous
        and previous.get("status") == "ok"
        and previous.get("build_fingerprint") == fingerprint
        and previous.get("target_triple") == toolchain["target_triple"]
        and destination.is_file()
        and previous.get("binary", {}).get("sha256") == sha256_file(destination)
    )
    if reusable:
        return {**previous, "reuse": "matched_target_fingerprint_and_binary_sha256"}

    target_name = f"builds/{component}-{fingerprint[:12]}"
    extra = ["--bin", build_spec["binary_name"]]
    command = cargo_command("build", manifest, *extra, selection=selection)
    record = run_command(
        command,
        env=tool_env(target_name, selection),
        timeout=900,
        log_path=RECORDS / "logs" / f"bootstrap-{component}-build.log",
    )
    commands.append({"label": f"build {component}", **command_for_evidence(record)})
    require_ok(record, f"构建固定 {component}")
    profile_dir = "release" if profile == "release" else "debug"
    built = LOCAL / "target" / target_name / profile_dir / build_spec["binary_name"]
    if not built.is_file():
        raise ToolFailure(f"找不到构建产物 {rel(built)}", EXIT_EVIDENCE)
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(built, destination)
    result = {
        "schema_version": 1,
        "component": component,
        "generated_at": utc_now(),
        "status": "ok",
        "reuse": "rebuilt",
        "build_fingerprint": fingerprint,
        "source": source_fingerprint,
        "platform": {
            "os": platform.system(),
            "os_release": platform.release(),
            "architecture": platform.machine(),
        },
        "target_triple": toolchain["target_triple"],
        "toolchain": toolchain,
        "binary": file_evidence(destination),
        "command": command_for_evidence(record),
    }
    write_json(record_path, result)
    return result


def bootstrap(args: argparse.Namespace) -> int:
    strategy = ROOT / "bot" / "src" / "strategy"
    before = sha256_tree(strategy) if strategy.exists() else None
    commands: list[dict[str, Any]] = []
    expected = load_json(UPSTREAM_LOCK)
    selection = select_toolchain()
    initialization = ensure_toolchain_initialized(selection, commands)
    toolchain = inspect_toolchain(selection)
    if not toolchain["usable"]:
        raise ToolFailure("所选本地 Rust 工具链不可用", EXIT_PRECONDITION)
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

    lock_template = ROOT / expected["builds"]["engine"]["source_lock_template_path"]
    mm_lock_path = bot_dir / ".mm" / "lock.toml"
    if not mm_lock_path.exists():
        mm_lock_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(lock_template, mm_lock_path)

    build_records = {
        name: ensure_component_build(name, spec, selection, toolchain, commands)
        for name, spec in expected["builds"].items()
    }

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
    if sha256_file(mm_lock_path) != expected["builds"]["engine"]["source_lock_sha256"]:
        raise ToolFailure("官方 .mm/lock.toml 缺失或哈希不匹配", EXIT_EVIDENCE)

    result = {
        "schema_version": 2,
        "generated_at": utc_now(),
        "status": "ok",
        "strategy_sha256_before": before,
        "strategy_sha256_after": after,
        "strategy_preserved": before is None or before == after,
        "engine_commit": actual_engine,
        "starterpack_commit": actual_starter,
        "source_actions": source_actions,
        "restored_scaffold": restored_scaffold,
        "toolchain_initialization": initialization,
        "toolchain": toolchain,
        "build_records": build_records,
        "commands": commands,
    }
    write_json(RECORDS / "bootstrap-latest.json", result)
    args._attempt_inputs_digest = canonical_sha256(
        {
            "upstream_lock": sha256_file(UPSTREAM_LOCK),
            "toolchain_lock": sha256_file(TOOLCHAIN_LOCK),
            "strategy_before": before,
        }
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT_OK


def test_all(args: argparse.Namespace) -> int:
    round_id = normalize_round(getattr(args, "round", DEFAULT_ROUND))
    suite_path = suite_path_from_args(args)
    suite = load_and_validate_suite(suite_path, round_id)
    selection = select_toolchain()
    inputs = suite_input_fingerprints(suite_path, suite, selection)
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
    rustfmt = selection["executables"].get("rustfmt")
    if not rustfmt:
        raise ToolFailure("找不到 rustfmt；bootstrap 会初始化所选工具链")
    specs = [
        (
            "strategy rustfmt check",
            [
                rustfmt,
                *([f"+{selection['channel']}"] if selection["executables"].get("rustup") else []),
                "--edition",
                "2021",
                "--check",
                *rust_sources,
            ],
            ROOT,
            tool_env(selection=selection),
            log_root / "rustfmt-strategy.log",
        ),
        (
            "bot tests",
            cargo_command("test", BOT_MANIFEST, selection=selection),
            ROOT,
            tool_env("bot", selection),
            log_root / "bot-tests.log",
        ),
        (
            "rules tests",
            cargo_command("test", RULES_MANIFEST, selection=selection),
            ROOT,
            tool_env("rules", selection),
            log_root / "rules-tests.log",
        ),
        (
            "opponent tests",
            cargo_command("test", OPPONENTS_MANIFEST, selection=selection),
            ROOT,
            tool_env("opponents", selection),
            log_root / "opponent-tests.log",
        ),
        (
            "tool tests",
            [sys.executable, "-m", "unittest", "discover", "-s", "tools/tests", "-p", "test_*.py", "-v"],
            ROOT,
            tool_env(selection=selection),
            log_root / "tool-tests.log",
        ),
        (
            "bot release build",
            cargo_command("build", BOT_MANIFEST, selection=selection),
            ROOT,
            tool_env("bot", selection),
            log_root / "bot-release.log",
        ),
    ]
    if (ROOT / ".git").exists():
        specs.append(
            (
                "git diff check",
                ["git", "diff", "--check"],
                ROOT,
                tool_env(selection=selection),
                log_root / "git-diff-check.log",
            )
        )

    outputs: dict[str, str] = {}
    for label, argv, cwd, env, log_path in specs:
        record = run_command(
            argv,
            cwd=cwd,
            env=env,
            timeout=900,
            log_path=log_path,
            show_output=label in {"bot tests", "rules tests", "opponent tests"},
        )
        outputs[label] = record["output"]
        commands.append({"label": label, **command_for_evidence(record)})
        require_ok(record, label)

    bot_totals = parse_test_totals([outputs["bot tests"]])
    rules_totals = parse_test_totals([outputs["rules tests"]])
    opponent_totals = parse_test_totals([outputs["opponent tests"]])
    tool_record = next(command for command in commands if command["label"] == "tool tests")
    tool_totals = parse_unittest_totals(outputs["tool tests"], tool_record["exit_code"])
    by_category = {
        "bot": bot_totals,
        "rules": rules_totals,
        "opponents": opponent_totals,
        "tools": tool_totals,
    }
    totals = {
        key: sum(category[key] for category in by_category.values())
        for key in ("passed", "failed", "skipped")
    }
    combined_output = outputs["bot tests"] + "\n" + outputs["rules tests"]
    required_contracts = [
        "r01_", "r02_", "r03_", "r04_", "r05_", "r06_", "r07_", "r08_",
        "r09_", "r10_", "r13_", "r16_", "r17_", "r18_", "r19_", "r20_", "r24_",
    ]
    missing_contracts = [name for name in required_contracts if f"test {name}" not in combined_output]
    if (
        totals["failed"] != 0
        or bot_totals["passed"] < 4
        or rules_totals["passed"] < 17
        or opponent_totals["passed"] < 5
        or missing_contracts
    ):
        raise ToolFailure(
            f"测试覆盖异常：totals={totals}, missing_contracts={missing_contracts}",
            EXIT_EVIDENCE,
        )

    result = {
        "schema_version": 2,
        "generated_at": utc_now(),
        "status": "ok",
        "round": round_id,
        "suite": {"id": suite["suite_id"], "path": rel(suite_path)},
        "tests": totals,
        "tests_by_category": by_category,
        "required_contracts": required_contracts,
        "inputs": inputs,
        "toolchain": inspect_toolchain(selection),
        "commands": commands,
    }
    write_json(RECORDS / "test-latest.json", result)
    args._attempt_inputs_digest = inputs["digest"]
    print(json.dumps({"status": "ok", "tests": totals, "tests_by_category": by_category}, ensure_ascii=False))
    return EXIT_OK


def extract_git_strategy(spec: dict[str, Any]) -> tuple[Path, dict[str, Any]]:
    metadata = _git_strategy_metadata(spec["source"])
    cache = LOCAL / "strategies" / f"{safe_id(str(spec['id']), 'strategy id')}-{metadata['resolved_commit'][:12]}"
    destination = cache / "src" / "strategy"
    meta_path = cache / "source.json"
    if destination.is_dir() and meta_path.is_file():
        cached = load_json(meta_path)
        if (
            cached.get("resolved_commit") == metadata["resolved_commit"]
            and cached.get("git_tree_oid") == metadata["git_tree_oid"]
            and cached.get("strategy_sha256") == sha256_tree(destination)
        ):
            return destination, cached
        raise ToolFailure(f"Git 策略缓存与来源记录不一致: {rel(cache)}", EXIT_EVIDENCE)

    prefix = metadata["path"] + "/"
    listing = run_command(
        ["git", "ls-tree", "-r", "--name-only", metadata["resolved_commit"], "--", metadata["path"]],
        timeout=30,
    )
    require_ok(listing, f"枚举 Git 策略 {spec['id']}")
    files = [line for line in listing["output"].splitlines() if line.startswith(prefix)]
    if not files:
        raise ToolFailure(f"Git strategy tree 为空: {metadata['path']}", EXIT_EVIDENCE)
    destination.mkdir(parents=True, exist_ok=False)
    for git_path in files:
        relative = Path(git_path.removeprefix(prefix))
        if relative.is_absolute() or ".." in relative.parts:
            raise ToolFailure(f"Git strategy tree 含非法路径: {git_path}", EXIT_EVIDENCE)
        blob = subprocess.run(
            ["git", "show", f"{metadata['resolved_commit']}:{git_path}"],
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if blob.returncode != 0:
            raise ToolFailure(f"提取 Git 策略文件失败: {git_path}", EXIT_SUBPROCESS)
        output = destination / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(blob.stdout)
    result = {**metadata, "strategy_sha256": sha256_tree(destination), "cache_path": rel(cache)}
    write_json(meta_path, result)
    return destination, result


def resolve_strategy_source(spec: dict[str, Any]) -> tuple[str, Path, dict[str, Any]]:
    source = spec["source"]
    kind = source["kind"]
    if kind == "git_strategy_tree":
        tree, metadata = extract_git_strategy(spec)
        return "tree", tree, metadata
    if kind == "working_strategy_tree":
        tree = resolve_repo_path(source.get("path", "bot/src/strategy"))
        return "tree", tree, strategy_source_metadata(spec)
    if kind == "single_file":
        path = resolve_repo_path(source["path"])
        return "single_file", path, strategy_source_metadata(spec)
    raise ToolFailure(f"未知 strategy source kind: {kind}", EXIT_EVIDENCE)


def materialize_strategy(
    spec: dict[str, Any],
    role: str,
    batch: Path,
    selection: dict[str, Any],
    commands: list[dict[str, Any]],
) -> tuple[Path, dict[str, Any]]:
    starter = LOCAL / "upstream" / "rust-starterpack"
    if not starter.is_dir():
        raise ToolFailure("缺少固定 starterpack；先运行 bootstrap")
    strategy_id = safe_id(str(spec["id"]), "strategy id")
    project = batch / "projects" / f"{role}-{strategy_id}"
    if project.exists():
        raise ToolFailure(f"临时策略工程已存在: {rel(project)}", EXIT_EVIDENCE)
    shutil.copytree(starter, project)
    mode, source, metadata = resolve_strategy_source(spec)
    destination = project / "src" / "strategy"
    if mode == "tree":
        shutil.rmtree(destination)
        shutil.copytree(source, destination)
    else:
        shutil.copy2(source, destination / "main.rs")
    copied_sha = sha256_tree(destination)
    expected_sha = metadata.get("strategy_sha256") or sha256_tree(destination)
    if mode == "tree" and copied_sha != expected_sha:
        raise ToolFailure(f"策略树复制校验失败: {strategy_id}", EXIT_EVIDENCE)

    target_name = f"runs/{batch.parent.name}/{batch.name}/{role}-{strategy_id}"
    log_path = batch / "build-logs" / f"{role}-{strategy_id}.log"
    record = run_command(
        cargo_command("build", project / "Cargo.toml", selection=selection),
        env=tool_env(target_name, selection),
        timeout=900,
        log_path=log_path,
    )
    commands.append({"label": f"build {role} {strategy_id}", **command_for_evidence(record)})
    require_ok(record, f"构建 {role} {strategy_id}")
    profile_dir = "release" if selection["build_profile"] == "release" else "debug"
    binary = LOCAL / "target" / target_name / profile_dir / "bot"
    if not binary.is_file():
        raise ToolFailure(f"找不到策略二进制 {rel(binary)}", EXIT_EVIDENCE)
    return binary, {
        "id": strategy_id,
        "role": role,
        "source": metadata,
        "copied_strategy_sha256": copied_sha,
        "project": rel(project),
        "binary": file_evidence(binary),
    }


def materialize_suite_engine(
    suite: dict[str, Any],
    batch: Path,
    selection: dict[str, Any],
    commands: list[dict[str, Any]],
) -> tuple[Path, dict[str, Any]]:
    engine_spec = suite.get("engine", {"kind": "locked_default"})
    if engine_spec["kind"] == "locked_default":
        record_path = build_record_path("engine")
        if not record_path.is_file() or not ENGINE.is_file():
            raise ToolFailure("缺少 engine 本机构建记录；请先运行 bootstrap", EXIT_EVIDENCE)
        record = load_json(record_path)
        toolchain = inspect_toolchain(selection)
        build_checks = validate_local_build_records(load_json(UPSTREAM_LOCK), toolchain)
        engine_checks = {
            key: value for key, value in build_checks.items() if key.startswith("engine_")
        }
        if not engine_checks or not all(engine_checks.values()):
            raise ToolFailure(
                f"engine 来源/target/工具链/二进制记录不匹配；请重跑 bootstrap: {engine_checks}",
                EXIT_EVIDENCE,
            )
        return ENGINE, {"kind": "locked_default", "build_record": record}

    upstream = load_json(UPSTREAM_LOCK)
    locked = upstream["builds"]["engine"]
    if engine_spec["base_source_commit"] != locked["source_commit"]:
        raise ToolFailure("scenario engine 的 base_source_commit 与来源锁不一致", EXIT_EVIDENCE)
    source_checks = validate_upstream_source_snapshots(upstream, {"engine"})
    if not source_checks or not all(source_checks.values()):
        raise ToolFailure(
            f"scenario engine checkout 与固定归档不一致: {source_checks}", EXIT_EVIDENCE
        )
    source = resolve_repo_path(locked["source_path"])
    project = batch / "projects" / "engine-fixture"
    shutil.copytree(source, project)
    fixture = resolve_repo_path(suite["fixture_path"])
    copied_fixture = project / "scenario-config.json"
    shutil.copy2(fixture, copied_fixture)
    engine_rs = project / "src" / "engine.rs"
    original = engine_rs.read_text(encoding="utf-8")
    start_marker = "    let conf = GameConfig {"
    end_marker = "\n\n    send!("
    start = original.find(start_marker)
    end = original.find(end_marker, start)
    if start < 0 or end < 0:
        raise ToolFailure("无法定位固定 engine 的 GameConfig 构造块", EXIT_EVIDENCE)
    replacement = (
        '    let conf: GameConfig = serde_json::from_str(include_str!("../scenario-config.json"))?;'
    )
    patched = original[:start] + replacement + original[end:]
    engine_rs.write_text(patched, encoding="utf-8")
    patch_text = "".join(
        difflib.unified_diff(
            original.splitlines(keepends=True),
            patched.splitlines(keepends=True),
            fromfile="a/src/engine.rs",
            tofile="b/src/engine.rs",
        )
    )
    patch_path = batch / "engine-fixture.patch"
    patch_path.write_text(patch_text, encoding="utf-8")
    toolchain = inspect_toolchain(selection)
    build_fingerprint_values = {
        "base_source_commit": locked["source_commit"],
        "base_source_tree_sha256": sha256_tree(source),
        "base_source_checks": source_checks,
        "cargo_lock_sha256": sha256_file(project / "Cargo.lock"),
        "fixture_sha256": sha256_file(copied_fixture),
        "patch_sha256": sha256_file(patch_path),
        "toolchain_fingerprint": toolchain["fingerprint"],
        "target_triple": toolchain["target_triple"],
        "profile": selection["build_profile"],
    }
    target_name = f"runs/{batch.parent.name}/{batch.name}/engine-fixture"
    record = run_command(
        cargo_command(
            "build", project / "Cargo.toml", "--bin", "mm-engine", selection=selection
        ),
        env=tool_env(target_name, selection),
        timeout=900,
        log_path=batch / "build-logs" / "engine-fixture.log",
    )
    commands.append({"label": "build fixture engine", **command_for_evidence(record)})
    require_ok(record, "构建 scenario fixture engine")
    profile_dir = "release" if selection["build_profile"] == "release" else "debug"
    binary = LOCAL / "target" / target_name / profile_dir / "mm-engine"
    if not binary.is_file():
        raise ToolFailure(f"找不到 scenario engine 二进制: {rel(binary)}", EXIT_EVIDENCE)
    return binary, {
        "kind": "fixture_build",
        "base_source_commit": locked["source_commit"],
        "base_source_tree_sha256": sha256_tree(source),
        "patched_source_tree_sha256": sha256_tree(project),
        "build_fingerprint": canonical_sha256(build_fingerprint_values),
        "build_fingerprint_inputs": build_fingerprint_values,
        "patch": {
            "path": "src/engine.rs",
            "description": "Replace the fixed GameConfig literal with compile-time deserialization of scenario-config.json.",
            "patched_file_sha256": sha256_file(engine_rs),
            "diff": file_evidence(patch_path),
        },
        "fixture": file_evidence(fixture),
        "binary": file_evidence(binary),
        "command": command_for_evidence(record),
    }


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


def callback_coverage_complete(
    result: dict[str, Any] | None, telemetry: list[dict[str, Any]]
) -> bool:
    if result is None or len(telemetry) != 1:
        return False
    final_tick = result.get("tick")
    summary = telemetry[0]
    return bool(
        isinstance(final_tick, int)
        and final_tick >= 0
        and summary.get("first_tick") == 0
        and summary.get("last_tick") == final_tick - 1
        and summary.get("calls") == final_tick
        and summary.get("skipped_ticks") == 0
    )


def suite_matrix_matches(suite: dict[str, Any], matches: list[dict[str, Any]]) -> bool:
    """Require artifact rows to preserve the declared matrix, including order."""
    fields = ("id", "candidate", "opponent", "candidate_side", "repetition")

    def project(match: dict[str, Any]) -> dict[str, Any]:
        value = {field: match.get(field) for field in fields}
        if isinstance(value["candidate_side"], str):
            value["candidate_side"] = value["candidate_side"].lower()
        return value

    return [project(match) for match in matches] == [
        project(match) for match in suite["matches"]
    ]


def long_game_coverage(
    suite: dict[str, Any], matches: list[dict[str, Any]]
) -> dict[str, Any]:
    """Summarize and enforce the M2 soak lifecycle coverage contract."""
    final_ticks = [
        match.get("final_tick")
        for match in matches
        if isinstance(match.get("final_tick"), int)
        and not isinstance(match.get("final_tick"), bool)
    ]
    required = suite.get("classification") == "soak_scenario"
    coverage = {
        "required": required,
        "final_ticks": final_ticks,
        "games_at_or_after_6000": sum(tick >= 6_000 for tick in final_ticks),
        "games_at_or_after_9000": sum(tick >= 9_000 for tick in final_ticks),
    }
    coverage["complete"] = bool(
        not required
        or (
            coverage["games_at_or_after_6000"] >= 2
            and coverage["games_at_or_after_9000"] >= 1
        )
    )
    return coverage


def contains_fatal(path: Path) -> bool:
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        return any("###" in line for line in handle)


def file_evidence(path: Path) -> dict[str, Any]:
    return {"path": rel(path), "sha256": sha256_file(path), "bytes": path.stat().st_size}


def aggregate_behavior_summaries(summaries: list[dict[str, Any]]) -> dict[str, Any]:
    def total(field: str) -> float | int:
        values = [item.get(field, 0) for item in summaries]
        return sum(value for value in values if isinstance(value, (int, float)) and not isinstance(value, bool))

    def maximum(field: str) -> float | int | None:
        values = [
            item[field]
            for item in summaries
            if isinstance(item.get(field), (int, float)) and not isinstance(item.get(field), bool)
        ]
        return max(values, default=None)

    def weighted_mean(mean_field: str, samples_field: str) -> float | None:
        samples = total(samples_field)
        if not samples:
            return None
        weighted = sum(
            float(item[mean_field]) * int(item.get(samples_field, 0))
            for item in summaries
            if isinstance(item.get(mean_field), (int, float))
            and isinstance(item.get(samples_field), (int, float))
        )
        return weighted / samples

    def ratio(numerator: str, denominator: str) -> float | None:
        denominator_value = total(denominator)
        return total(numerator) / denominator_value if denominator_value else None

    first_support = [
        int(item["first_healer_support_radius"])
        for item in summaries
        if isinstance(item.get("first_healer_support_radius"), int)
    ]
    thresholds = sorted(
        {
            item.get("high_population_threshold")
            for item in summaries
            if isinstance(item.get("high_population_threshold"), int)
        }
    )
    neighbor_distances = sorted(
        {
            item.get("near_neighbor_distance")
            for item in summaries
            if isinstance(item.get("near_neighbor_distance"), (int, float))
        }
    )
    return {
        "telemetry_schema": "m02_behavior_aggregate_v1",
        "summaries": len(summaries),
        "fire": {
            "requested": total("fire_requests"),
            "feedback_observable": total("fire_feedback_observable"),
            "observed": total("fire_observed"),
            "not_observed": total("fire_not_observed"),
            "censored": total("fire_feedback_censored"),
            "pending": total("fire_feedback_pending"),
            "observed_rate": ratio("fire_observed", "fire_feedback_observable"),
            "prediction_samples": total("shot_prediction_samples"),
            "prediction_missing": total("shot_prediction_missing"),
            "candidates_evaluated": total("fire_candidates_evaluated"),
            "zero_marginal_candidates_skipped": total("zero_marginal_candidates_skipped"),
            "geometry_uncertain_candidates": total("geometry_uncertain_candidates"),
            "zero_marginal_fire_suppressions": total("zero_marginal_fire_suppressions"),
        },
        "blast_error": {
            "samples": total("blast_error_samples"),
            "mean": weighted_mean("blast_error_mean", "blast_error_samples"),
            "max": maximum("blast_error_max"),
            "censored": total("blast_error_censored"),
            "pending": total("blast_error_pending"),
        },
        "damage": {
            "predicted_new_targets": total("predicted_new_damage_targets"),
            "predicted_new_targets_pending": total("predicted_new_damage_targets_pending"),
            "feedback_frames": total("damage_feedback_frames"),
            "feedback_complete_frames": total("damage_feedback_complete_frames"),
            "feedback_censored_frames": total("damage_feedback_censored_frames"),
            "transition_observable": total("damage_transition_observable"),
            "transition_censored": total("damage_transition_censored"),
            "transition_pending": total("damage_transition_pending"),
            "actual_new_damaged_targets": total("actual_new_damaged_targets"),
        },
        "healing": {
            "requested": total("heal_requests"),
            "request_ticks": total("heal_request_ticks"),
            "feedback_observable": total("heal_feedback_observable"),
            "observed_channels": total("heal_observed_channels"),
            "not_observed": total("heal_not_observed"),
            "target_matches": total("heal_target_matches"),
            "target_mismatches": total("heal_target_mismatches"),
            "censored": total("heal_feedback_censored"),
            "pending": total("heal_feedback_pending"),
            "observed_ticks": total("heal_observed_ticks"),
            "planned_effective_channels": total("planned_effective_heal_channels"),
            "channel_tick_slots": total("healer_channel_tick_slots"),
            "demand_limited_capacity_slots": total("demand_limited_heal_capacity_slots"),
            "effective_capacity_occupancy": ratio(
                "planned_effective_heal_channels", "demand_limited_heal_capacity_slots"
            ),
        },
        "support": {
            "planned_travelers": total("planned_support_travelers"),
            "travel_ticks": total("support_travel_ticks"),
            "anchor_samples": total("support_anchor_samples"),
            "first_radius_observed_matches": len(first_support),
            "first_radius_tick_min": min(first_support, default=None),
            "first_radius_tick_max": max(first_support, default=None),
            "full_health_follow_samples": total("full_health_follow_samples"),
            "full_health_follow_distance_mean": weighted_mean(
                "full_health_follow_distance_mean", "full_health_follow_samples"
            ),
            "full_health_follow_distance_max": maximum("full_health_follow_distance_max"),
            "far_travelers": total("far_support_travelers"),
            "far_travel_ticks": total("far_support_travel_ticks"),
        },
        "control_and_density": {
            "observation_ticks": total("control_observation_ticks"),
            "friendly_presence_ticks": total("friendly_control_presence_ticks"),
            "enemy_presence_ticks": total("enemy_control_presence_ticks"),
            "contested_ticks": total("contested_control_ticks"),
            "high_population_ticks": total("high_population_ticks"),
            "high_population_thresholds": thresholds,
            "max_friendly_population": maximum("max_friendly_population"),
            "pair_samples": total("friendly_pair_samples"),
            "near_neighbor_pairs": total("near_neighbor_pairs"),
            "near_neighbor_pair_rate": ratio("near_neighbor_pairs", "friendly_pair_samples"),
            "near_neighbor_ticks": total("near_neighbor_ticks"),
            "near_neighbor_distances": neighbor_distances,
        },
    }


def aggregate_matches(
    matches: list[dict[str, Any]],
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    by_opponent: dict[str, dict[str, Any]] = {}
    summaries: list[dict[str, Any]] = []
    for match in matches:
        key = f"{match['candidate']}__vs__{match['opponent']}"
        bucket = by_opponent.setdefault(
            key,
            {
                "candidate": match["candidate"],
                "opponent": match["opponent"],
                "games": 0,
                "wins": 0,
                "draws": 0,
                "losses": 0,
                "points": 0,
            },
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
    return by_opponent, budget, aggregate_behavior_summaries(summaries)


def smoke(args: argparse.Namespace) -> int:
    round_id = normalize_round(args.round)
    suite_path = suite_path_from_args(args)
    suite = load_and_validate_suite(suite_path, round_id)
    selection = select_toolchain()
    inputs = suite_input_fingerprints(suite_path, suite, selection)
    expected = int(suite["expected_matches"])
    test_path = RECORDS / "test-latest.json"
    test_record = load_json(test_path) if test_path.exists() else None
    test_attempt_path = attempt_path("test")
    test_attempt = load_json(test_attempt_path) if test_attempt_path.exists() else None
    if not evidence_is_current(test_record, test_attempt, inputs):
        raise ToolFailure("test 的成功 attempt 与当前 suite/input fingerprint 不匹配", EXIT_EVIDENCE)

    run_id = safe_id(args.run_id or f"run-{stamp()}", "run-id")
    suite_id = safe_id(str(suite["suite_id"]), "suite_id")
    batch = LOCAL / "runs" / round_id / suite_id / run_id
    artifact_dir = ROOT / "artifacts" / "summaries" / round_id / suite_id / run_id
    if batch.exists() or artifact_dir.exists():
        raise ToolFailure(f"run-id 已存在，拒绝覆盖: {run_id}", EXIT_EVIDENCE)
    batch.mkdir(parents=True)
    artifact_dir.mkdir(parents=True)

    commands: list[dict[str, Any]] = []
    engine_binary, engine_record = materialize_suite_engine(suite, batch, selection, commands)
    candidate_binaries: dict[str, Path] = {}
    opponent_binaries: dict[str, Path] = {}
    strategy_records: dict[str, Any] = {}
    for candidate_spec in suite["candidates"]:
        binary, source_record = materialize_strategy(
            candidate_spec, "candidate", batch, selection, commands
        )
        candidate_binaries[str(candidate_spec["id"])] = binary
        strategy_records[f"candidate:{candidate_spec['id']}"] = source_record
    for opponent_spec in suite["opponents"]:
        binary, source_record = materialize_strategy(
            opponent_spec, "opponent", batch, selection, commands
        )
        opponent_binaries[str(opponent_spec["id"])] = binary
        strategy_records[f"opponent:{opponent_spec['id']}"] = source_record

    matches: list[dict[str, Any]] = []
    progress_path = RECORDS / "smoke-progress-latest.json"
    progress_base = {
        "schema_version": 2,
        "round": round_id,
        "suite_id": suite_id,
        "run_id": run_id,
        "run_root": rel(batch),
        "artifact_root": rel(artifact_dir),
        "inputs": inputs,
        "formal_matches_expected": expected,
    }
    write_json(
        progress_path,
        {
            **progress_base,
            "generated_at": utc_now(),
            "status": "running",
            "formal_matches_completed": 0,
            "matches": [],
        },
    )
    fixture_path = resolve_repo_path(suite["fixture_path"])
    expected_config_sha = sha256_file(fixture_path)

    for index, spec in enumerate(suite["matches"], start=1):
        candidate = candidate_binaries[str(spec["candidate"])]
        opponent = opponent_binaries[str(spec["opponent"])]
        candidate_side = str(spec["candidate_side"]).upper()
        bot_a, bot_b = (candidate, opponent) if candidate_side == "A" else (opponent, candidate)
        match_dir = batch / "matches" / str(spec["id"])
        match_dir.mkdir(parents=True)
        gamelog = match_dir / "game.mmgl"
        a_log = match_dir / "a.log"
        b_log = match_dir / "b.log"
        runner_log = match_dir / "runner.log"
        print(f"[mm32] timed match {index}/{expected}: {spec['id']}", flush=True)
        argv = [
            str(engine_binary),
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
        commands.append({"label": str(spec["id"]), **command_for_evidence(record)})
        if not all(path.exists() for path in (gamelog, a_log, b_log, runner_log)):
            raise ToolFailure(f"比赛 {spec['id']} 缺少日志", EXIT_EVIDENCE)
        config, result, engine_elapsed = parse_game_log(gamelog)
        fatal = contains_fatal(a_log) or contains_fatal(b_log) or "###" in record["output"]
        candidate_log = a_log if candidate_side == "A" else b_log
        telemetry = parse_bot_summaries(candidate_log)
        config_line = (
            json.dumps(config, ensure_ascii=False, separators=(",", ":")) + "\n" if config else ""
        )
        config_sha = hashlib.sha256(config_line.encode()).hexdigest() if config_line else None
        complete_callbacks = callback_coverage_complete(result, telemetry)
        valid = bool(
            record["exit_code"] == 0
            and result is not None
            and not fatal
            and len(telemetry) == 1
            and config_sha == expected_config_sha
            and complete_callbacks
        )
        winner = result.get("winner") if result else None
        outcome = "draw" if winner is None else ("win" if winner == candidate_side else "loss")
        entry = {
            **spec,
            "candidate_side": candidate_side.lower(),
            "valid": valid,
            "exit_code": record["exit_code"],
            "wall_seconds": record["duration_seconds"],
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
            "callback_coverage_complete": complete_callbacks,
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
                **progress_base,
                "generated_at": utc_now(),
                "status": "running" if valid and index < expected else ("ok" if valid else "failed"),
                "formal_matches_completed": sum(1 for match in matches if match["valid"]),
                "matches": matches,
            },
        )
        if not valid:
            raise ToolFailure(f"比赛 {spec['id']} 证据不完整、配置漂移或含 fatal error", EXIT_EVIDENCE)

    coverage = long_game_coverage(suite, matches)
    if not coverage["complete"]:
        raise ToolFailure(
            f"soak 生命周期覆盖不足: {coverage}", EXIT_EVIDENCE
        )
    by_pair, budget, behavior = aggregate_matches(matches)
    metrics = {
        "schema_version": 2,
        "round": round_id,
        "suite_id": suite_id,
        "run_id": run_id,
        "classification": suite.get("classification", "formal"),
        "status": "ok",
        "generated_at": utc_now(),
        "inputs_digest": inputs["digest"],
        "batch_inputs": {
            "suite": {"path": rel(suite_path), "sha256": sha256_file(suite_path)},
            "fixture": {"path": rel(fixture_path), "sha256": expected_config_sha},
        },
        "tests": test_record["tests"],
        "tests_by_category": test_record.get("tests_by_category", {}),
        "formal_matches_expected": expected,
        "formal_matches_completed": len(matches),
        "seed": suite.get("seed"),
        "matches": matches,
        "long_game_coverage": coverage,
        "by_candidate_opponent": by_pair,
        "budget": budget,
        "behavior": behavior,
    }
    write_json(artifact_dir / "metrics.json", metrics)

    repo = repository_facts()
    manifest = {
        "schema_version": 2,
        "round": round_id,
        "suite_id": suite_id,
        "run_id": run_id,
        "generated_at": utc_now(),
        "repository": repo,
        "evaluated_code_commit": evaluated_code_commit(repo),
        "inputs": inputs,
        "upstream_source_lock": load_json(UPSTREAM_LOCK),
        "toolchain": redact_record(inspect_toolchain(selection)),
        "engine_build_record": redact_record(engine_record),
        "strategies": strategy_records,
        "binaries": {
            "engine": file_evidence(engine_binary),
            "candidates": {key: file_evidence(value) for key, value in candidate_binaries.items()},
            "opponents": {key: file_evidence(value) for key, value in opponent_binaries.items()},
        },
        "configs": {
            "upstream_lock": file_evidence(UPSTREAM_LOCK),
            "toolchain_lock": file_evidence(TOOLCHAIN_LOCK),
            "suite": file_evidence(suite_path),
            "fixture": file_evidence(fixture_path),
        },
        "time_enforcement": True,
        "formal_match_count": len(matches),
        "run_root": rel(batch),
        "artifact_root": rel(artifact_dir),
        "workflow_records": {
            name: file_evidence(RECORDS / f"{name}-latest.json")
            for name in ("doctor", "bootstrap", "test")
            if (RECORDS / f"{name}-latest.json").exists()
        },
        "commands": commands,
    }
    write_json(artifact_dir / "manifest.json", manifest)
    latest = {
        "schema_version": 2,
        "generated_at": utc_now(),
        "status": "ok",
        "round": round_id,
        "suite_id": suite_id,
        "run_id": run_id,
        "inputs_digest": inputs["digest"],
        "run_root": rel(batch),
        "artifact_root": rel(artifact_dir),
        "metrics": file_evidence(artifact_dir / "metrics.json"),
        "manifest": file_evidence(artifact_dir / "manifest.json"),
    }
    write_json(RECORDS / "smoke-latest.json", latest)
    args._attempt_inputs_digest = inputs["digest"]
    print(
        json.dumps(
            {
                "completed": len(matches),
                "artifact_root": rel(artifact_dir),
                "by_pair": by_pair,
                "budget": budget,
                "behavior": behavior,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return EXIT_OK


def packcheck(args: argparse.Namespace) -> int:
    round_id = normalize_round(getattr(args, "round", DEFAULT_ROUND))
    suite_path = suite_path_from_args(args)
    suite = load_and_validate_suite(suite_path, round_id)
    run_id = safe_id(args.run_id or f"pack-{stamp()}", "run-id")
    selection = select_toolchain()
    inputs = suite_input_fingerprints(suite_path, suite, selection)
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
        target_name = f"packcheck/{run_id}"
        record = run_command(
            cargo_command("build", project / "Cargo.toml", selection=selection),
            env=tool_env(target_name, selection),
            timeout=900,
            log_path=RECORDS / "logs" / "packcheck.log",
            show_output=True,
        )
        require_ok(record, "packcheck release build")
        profile_dir = "release" if selection["build_profile"] == "release" else "debug"
        binary = LOCAL / "target" / target_name / profile_dir / "bot"
        result = {
            "schema_version": 2,
            "generated_at": utc_now(),
            "status": "ok",
            "round": round_id,
            "run_id": run_id,
            "suite": {"id": suite["suite_id"], "path": rel(suite_path)},
            "inputs_digest": inputs["digest"],
            "declared_upstream_toolchain": selection["official_declared_channel"],
            "build_toolchain": redact_record(inspect_toolchain(selection)),
            "toolchain_override_reason": selection["official_compatibility"]["reason"],
            "strategy_source_sha256": source_hash,
            "strategy_copied_sha256": copied_hash,
            "only_strategy_restored": True,
            "server_submission_validation": "not_run_requires_shi_authorization",
            "binary": file_evidence(binary),
            "command": command_for_evidence(record),
        }
    write_json(RECORDS / "packcheck-latest.json", result)
    latest_path = RECORDS / "smoke-latest.json"
    if latest_path.exists():
        latest = load_json(latest_path)
        same_batch = (
            latest.get("round") == round_id
            and latest.get("suite_id") == suite["suite_id"]
            and latest.get("inputs_digest") == inputs["digest"]
            and latest.get("run_id") == run_id
        )
        if same_batch:
            manifest_path = resolve_repo_path(latest["artifact_root"]) / "manifest.json"
            if manifest_path.exists():
                manifest = load_json(manifest_path)
                manifest["packcheck"] = result
                manifest.setdefault("commands", []).append(
                    {"label": "packcheck build", **result["command"]}
                )
                write_json(manifest_path, manifest)
                latest["manifest"] = file_evidence(manifest_path)
                write_json(latest_path, latest)
    args._attempt_inputs_digest = inputs["digest"]
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


def validate_local_build_records(
    upstream: dict[str, Any], toolchain: dict[str, Any]
) -> dict[str, bool]:
    checks: dict[str, bool] = {}
    for name, spec in upstream["builds"].items():
        record_path = build_record_path(name)
        destination = resolve_repo_path(spec["output_path"])
        source = resolve_repo_path(spec["source_path"])
        record = load_json(record_path) if record_path.exists() else None
        expected_source = {
            "source_commit": spec["source_commit"],
            "source_tree_sha256": sha256_tree(source),
            "manifest_sha256": file_sha_or_none(source / "Cargo.toml"),
            "cargo_lock_sha256": file_sha_or_none(source / "Cargo.lock"),
            "binary_name": spec["binary_name"],
            "profile": toolchain["build_profile"],
            "toolchain_fingerprint": toolchain["fingerprint"],
            "target_triple": toolchain["target_triple"],
        }
        checks[f"{name}_record_present"] = record is not None
        checks[f"{name}_status"] = bool(record and record.get("status") == "ok")
        checks[f"{name}_component"] = bool(
            record and record.get("component") == name
        )
        checks[f"{name}_command"] = bool(
            record
            and record.get("command", {}).get("exit_code") == 0
            and record.get("command", {}).get("timed_out") is False
        )
        checks[f"{name}_source_commit"] = bool(
            record and record.get("source", {}).get("source_commit") == spec["source_commit"]
        )
        checks[f"{name}_target_triple"] = bool(
            record and record.get("target_triple") == toolchain["target_triple"]
        )
        checks[f"{name}_toolchain"] = bool(
            record
            and record.get("source", {}).get("toolchain_fingerprint") == toolchain["fingerprint"]
        )
        checks[f"{name}_source_fingerprint"] = bool(
            record
            and record.get("source") == expected_source
            and record.get("build_fingerprint") == canonical_sha256(expected_source)
        )
        checks[f"{name}_binary"] = bool(
            record
            and destination.is_file()
            and record.get("binary", {}).get("sha256") == sha256_file(destination)
            and record.get("binary", {}).get("bytes") == destination.stat().st_size
            and record.get("binary", {}).get("path") == rel(destination)
        )
    return checks


def validate_suite_artifacts(
    round_id: str,
    suite_path: Path,
    selection: dict[str, Any],
    evaluated_commit: str | None,
) -> dict[str, Any]:
    """Find an immutable successful run bound to the current inputs and code commit."""
    suite = load_and_validate_suite(suite_path, round_id)
    inputs = suite_input_fingerprints(suite_path, suite, selection)
    suite_id = safe_id(str(suite["suite_id"]), "suite_id")
    root = ROOT / "artifacts" / "summaries" / round_id / suite_id
    candidates: list[dict[str, Any]] = []
    if root.is_dir():
        for run_dir in sorted(path for path in root.iterdir() if path.is_dir()):
            metrics_path = run_dir / "metrics.json"
            manifest_path = run_dir / "manifest.json"
            if not metrics_path.is_file() or not manifest_path.is_file():
                continue
            metrics = load_json(metrics_path)
            manifest = load_json(manifest_path)
            matches = metrics.get("matches", [])
            matrix_current = isinstance(matches, list) and suite_matrix_matches(
                suite, matches
            )
            coverage = (
                long_game_coverage(suite, matches)
                if isinstance(matches, list)
                else {"required": True, "complete": False, "final_ticks": []}
            )
            current = bool(
                evaluated_commit is not None
                and metrics.get("status") == "ok"
                and metrics.get("round") == round_id
                and metrics.get("suite_id") == suite_id
                and metrics.get("run_id") == run_dir.name
                and metrics.get("inputs_digest") == inputs["digest"]
                and metrics.get("formal_matches_completed") == suite["expected_matches"]
                and len(matches) == suite["expected_matches"]
                and matrix_current
                and coverage["complete"] is True
                and all(
                    match.get("valid") is True
                    and match.get("callback_coverage_complete") is True
                    for match in matches
                )
                and metrics.get("classification", "formal")
                == suite.get("classification", "formal")
                and manifest.get("round") == round_id
                and manifest.get("suite_id") == suite_id
                and manifest.get("run_id") == run_dir.name
                and manifest.get("inputs", {}).get("digest") == inputs["digest"]
                and manifest.get("formal_match_count") == suite["expected_matches"]
                and manifest.get("time_enforcement") is True
                and manifest.get("evaluated_code_commit") == evaluated_commit
            )
            candidates.append(
                {
                    "run_id": run_dir.name,
                    "current": current,
                    "metrics": file_evidence(metrics_path),
                    "manifest": file_evidence(manifest_path),
                    "generated_at": metrics.get("generated_at"),
                    "matrix_current": matrix_current,
                    "long_game_coverage": coverage,
                }
            )
    current_runs = [candidate for candidate in candidates if candidate["current"]]
    selected = current_runs[-1] if current_runs else None
    return {
        "suite_id": suite_id,
        "suite": file_evidence(suite_path),
        "expected_matches": suite["expected_matches"],
        "classification": suite.get("classification", "formal"),
        "inputs_digest": inputs["digest"],
        "current": selected is not None,
        "selected": selected,
        "examined_runs": candidates,
    }


def validate_cleancheck_artifact(
    round_id: str,
    evaluated_commit: str | None,
    toolchain: dict[str, Any] | None = None,
) -> dict[str, Any]:
    expected_absence = {
        ".local": True,
        "bot/Cargo.toml": True,
        "bot/Cargo.lock": True,
        "bot/src/core.rs": True,
        "engine_binary": True,
    }
    expected_commands = [
        "bootstrap_first",
        "test",
        "timed_match",
        "packcheck",
        "bootstrap_second",
    ]
    clean_suite_path = ROOT / "configs" / "suites" / "m02_cleancheck.json"
    root = ROOT / "artifacts" / "summaries" / round_id / "cleancheck"
    candidates: list[dict[str, Any]] = []
    if root.is_dir():
        for run_dir in sorted(path for path in root.iterdir() if path.is_dir()):
            manifest_path = run_dir / "manifest.json"
            if not manifest_path.is_file():
                continue
            manifest = load_json(manifest_path)
            commands = manifest.get("commands", [])
            command_labels = [command.get("label") for command in commands]
            toolchain_matches = bool(
                toolchain
                and manifest.get("toolchain_compiler_fingerprint")
                == toolchain.get("compiler_fingerprint")
            )
            current = bool(
                evaluated_commit is not None
                and manifest.get("status") == "ok"
                and manifest.get("round") == round_id
                and manifest.get("suite_id") == "m02-cleancheck"
                and manifest.get("run_id") == run_dir.name
                and manifest.get("source_commit") == evaluated_commit
                and manifest.get("suite", {}).get("path")
                == rel(clean_suite_path)
                and clean_suite_path.is_file()
                and manifest.get("suite", {}).get("sha256")
                == sha256_file(clean_suite_path)
                and manifest.get("initial_absence") == expected_absence
                and manifest.get("second_bootstrap_idempotent") is True
                and manifest.get("shared_cache_conditions", {}).get(
                    "network_disabled_for_cargo"
                )
                is True
                and command_labels == expected_commands
                and all(
                    command.get("exit_code") == 0
                    and command.get("timed_out") is False
                    for command in commands
                )
                and toolchain_matches
            )
            candidates.append(
                {
                    "run_id": run_dir.name,
                    "current": current,
                    "manifest": file_evidence(manifest_path),
                    "generated_at": manifest.get("generated_at"),
                    "toolchain_matches": toolchain_matches,
                    "command_labels": command_labels,
                }
            )
    current_runs = [candidate for candidate in candidates if candidate["current"]]
    selected = current_runs[-1] if current_runs else None
    latest_path = RECORDS / "cleancheck-latest.json"
    latest_matches = False
    if selected and latest_path.is_file():
        latest = load_json(latest_path)
        latest_matches = bool(
            latest == load_json(root / selected["run_id"] / "manifest.json")
            and latest.get("status") == "ok"
            and latest.get("run_id") == selected["run_id"]
            and latest.get("source_commit") == evaluated_commit
        )
    return {
        "current": selected is not None and latest_matches,
        "latest_record_matches": latest_matches,
        "selected": selected,
        "examined_runs": candidates,
    }


def audit(args: argparse.Namespace) -> int:
    round_id = normalize_round(args.round)
    suite_path = suite_path_from_args(args)
    suite = load_and_validate_suite(suite_path, round_id)
    selection = select_toolchain()
    toolchain = inspect_toolchain(selection)
    inputs = suite_input_fingerprints(suite_path, suite, selection)
    required_suite_paths = [
        ROOT / "configs" / "suites" / "m02_regression.json",
        ROOT / "configs" / "suites" / "m02_challenge.json",
        ROOT / "configs" / "suites" / "m02_soak.json",
    ]
    required = [
        ROOT / "README.md",
        ROOT / "bot" / "mm-config.toml",
        ROOT / "bot" / "src" / "strategy",
        RULES_MANIFEST,
        ROOT / "lab" / "rules" / "Cargo.lock",
        OPPONENTS_MANIFEST,
        ROOT / "lab" / "opponents" / "Cargo.lock",
        UPSTREAM_LOCK,
        TOOLCHAIN_LOCK,
        suite_path,
        *required_suite_paths,
    ]
    missing = [rel(path) for path in required if not path.exists()]
    files = candidate_files()
    secret_findings = scan_secrets(files)
    whitespace_findings = scan_source_whitespace(files)
    large_files = [
        {"path": rel(path), "bytes": path.stat().st_size}
        for path in files
        if path.is_file() and path.stat().st_size > 1_000_000
    ]
    ignored_checks: dict[str, bool] = {}
    for item in [
        ".local/bin/mm-cli",
        ".local/records/builds/engine.json",
        "bot/Cargo.toml",
        "bot/Cargo.lock",
        "bot/.mm/bin/mm-engine",
        "bot/src/core.rs",
    ]:
        record = run_command(["git", "check-ignore", "-q", item], timeout=30)
        ignored_checks[item] = record["exit_code"] == 0
    diff_check = run_command(["git", "diff", "--check"], timeout=30)
    repo = repository_facts()
    evaluated_commit = evaluated_code_commit(repo)
    suite_artifact_checks = {
        path.stem: validate_suite_artifacts(round_id, path, selection, evaluated_commit)
        for path in required_suite_paths
        if path.is_file()
    }
    cleancheck_artifact = validate_cleancheck_artifact(
        round_id, evaluated_commit, toolchain
    )
    m01_history = run_command(
        [
            "git",
            "diff",
            "--quiet",
            M01_DELIVERY_COMMIT,
            "--",
            "artifacts/summaries/m01",
            "docs/reports/mechmania32_report1.md",
            "docs/rules/mechmania32_rules1.md",
            "docs/mechmania32_index1.md",
        ],
        timeout=30,
    )
    m01_untracked = run_command(
        [
            "git",
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            "artifacts/summaries/m01",
            "docs/reports/mechmania32_report1.md",
            "docs/rules/mechmania32_rules1.md",
            "docs/mechmania32_index1.md",
        ],
        timeout=30,
    )

    def record_or_none(name: str) -> dict[str, Any] | None:
        path = RECORDS / f"{name}-latest.json"
        return load_json(path) if path.exists() else None

    bootstrap_record = record_or_none("bootstrap")
    test_record = record_or_none("test")
    pack_record = record_or_none("packcheck")
    smoke_record = record_or_none("smoke")
    test_attempt = load_json(attempt_path("test")) if attempt_path("test").exists() else None
    smoke_attempt = load_json(attempt_path("smoke")) if attempt_path("smoke").exists() else None
    pack_attempt = load_json(attempt_path("packcheck")) if attempt_path("packcheck").exists() else None
    latest_artifact = None
    metrics = manifest = None
    if smoke_record and smoke_record.get("artifact_root"):
        candidate_dir = resolve_repo_path(smoke_record["artifact_root"])
        if (candidate_dir / "metrics.json").exists() and (candidate_dir / "manifest.json").exists():
            latest_artifact = candidate_dir
            metrics = load_json(candidate_dir / "metrics.json")
            manifest = load_json(candidate_dir / "manifest.json")

    upstream = load_json(UPSTREAM_LOCK)
    source_checks = validate_upstream_source_snapshots(upstream)
    build_checks = validate_local_build_records(upstream, toolchain)
    evidence_checks = {
        "bootstrap_preserved_current_strategy": bool(
            bootstrap_record
            and bootstrap_record.get("status") == "ok"
            and bootstrap_record.get("strategy_preserved") is True
            and bootstrap_record.get("strategy_sha256_after")
            == sha256_tree(ROOT / "bot" / "src" / "strategy")
        ),
        "test_current": evidence_is_current(test_record, test_attempt, inputs),
        "smoke_current": bool(
            smoke_record
            and smoke_attempt
            and smoke_record.get("status") == "ok"
            and smoke_record.get("round") == round_id
            and smoke_record.get("suite_id") == suite["suite_id"]
            and smoke_record.get("inputs_digest") == inputs["digest"]
            and smoke_attempt.get("status") == "ok"
            and smoke_attempt.get("inputs_digest") == inputs["digest"]
        ),
        "metrics_current": bool(
            metrics
            and metrics.get("status") == "ok"
            and metrics.get("round") == round_id
            and metrics.get("suite_id") == suite["suite_id"]
            and smoke_record
            and metrics.get("run_id") == smoke_record.get("run_id")
            and metrics.get("inputs_digest") == inputs["digest"]
            and metrics.get("formal_matches_completed") == suite["expected_matches"]
            and isinstance(metrics.get("matches"), list)
            and len(metrics.get("matches", [])) == suite["expected_matches"]
            and suite_matrix_matches(suite, metrics.get("matches", []))
            and long_game_coverage(suite, metrics.get("matches", []))["complete"]
            and all(
                match.get("valid") is True
                and match.get("callback_coverage_complete") is True
                for match in metrics.get("matches", [])
            )
        ),
        "manifest_current": bool(
            manifest
            and manifest.get("round") == round_id
            and manifest.get("suite_id") == suite["suite_id"]
            and smoke_record
            and manifest.get("run_id") == smoke_record.get("run_id")
            and manifest.get("inputs", {}).get("digest") == inputs["digest"]
            and manifest.get("formal_match_count") == suite["expected_matches"]
            and manifest.get("evaluated_code_commit") == evaluated_commit
            and smoke_record.get("manifest")
            and recorded_file_matches(smoke_record.get("manifest"))
        ),
        "packcheck_current": bool(
            pack_record
            and pack_attempt
            and smoke_record
            and manifest
            and pack_record.get("status") == "ok"
            and pack_record.get("round") == round_id
            and pack_record.get("suite", {}).get("id") == suite["suite_id"]
            and pack_record.get("run_id") == smoke_record.get("run_id")
            and manifest.get("run_id") == pack_record.get("run_id")
            and pack_record.get("inputs_digest") == inputs["digest"]
            and pack_attempt.get("status") == "ok"
            and pack_attempt.get("inputs_digest") == inputs["digest"]
            and pack_record.get("strategy_source_sha256") == pack_record.get("strategy_copied_sha256")
            and pack_record.get("command", {}).get("exit_code") == 0
            and pack_record.get("command", {}).get("timed_out") is False
            and recorded_file_matches(pack_record.get("binary"))
            and manifest.get("packcheck") == pack_record
        ),
        "all_required_suite_artifacts_current": bool(
            len(suite_artifact_checks) == len(required_suite_paths)
            and all(check["current"] for check in suite_artifact_checks.values())
        ),
        "cleancheck_current": cleancheck_artifact["current"],
        "m01_history_unchanged": m01_history["exit_code"] == 0,
        "m01_no_untracked_additions": bool(
            m01_untracked["exit_code"] == 0 and not m01_untracked["output"].strip()
        ),
    }
    failures: list[str] = []
    for condition, label in [
        (not missing, "missing_required_paths"),
        (not secret_findings, "secret_findings"),
        (not whitespace_findings, "source_whitespace"),
        (not large_files, "large_tracked_or_candidate_files"),
        (all(ignored_checks.values()), "generated_boundary"),
        (diff_check["exit_code"] == 0, "git_diff_check"),
        (toolchain["usable"], "toolchain_unusable"),
        (all(source_checks.values()), "source_lock"),
        (all(build_checks.values()), "local_build_records"),
        (all(evidence_checks.values()), "stale_or_incomplete_evidence"),
    ]:
        if not condition:
            failures.append(label)
    warnings: list[str] = []
    if repo["status_porcelain"] and evaluated_commit:
        warnings.append("仅报告/机器证据晚于代码提交；evaluated_code_commit 绑定当前 HEAD")
    elif repo["status_porcelain"]:
        warnings.append("存在未提交评估输入；evaluated_code_commit 保持 null")
    if selection["channel"] != selection["official_declared_channel"]:
        warnings.append(selection["official_compatibility"]["reason"])
    result = {
        "schema_version": 2,
        "round": round_id,
        "suite_id": suite["suite_id"],
        "generated_at": utc_now(),
        "status": "pass_with_warnings" if not failures else "fail",
        "failures": failures,
        "warnings": warnings,
        "inputs": inputs,
        "required_paths": {rel(path): path.exists() for path in required},
        "generated_paths_ignored": ignored_checks,
        "secret_findings": secret_findings,
        "large_files": large_files,
        "source_whitespace_findings": whitespace_findings,
        "git_diff_check": command_for_evidence(diff_check),
        "source_lock_checks": source_checks,
        "local_build_record_checks": build_checks,
        "evidence_checks": evidence_checks,
        "required_suite_artifact_checks": suite_artifact_checks,
        "cleancheck_artifact_check": cleancheck_artifact,
        "m01_history_check": command_for_evidence(m01_history),
        "m01_untracked_check": {
            **command_for_evidence(m01_untracked),
            "paths": m01_untracked["output"].splitlines(),
        },
        "latest_artifact_root": rel(latest_artifact) if latest_artifact else None,
        "toolchain": redact_record(toolchain),
        "repository": repo,
        "evaluated_code_commit": evaluated_commit,
        "formal_submission": "not_run_requires_shi_authorization",
    }
    audit_path = ROOT / "artifacts" / "summaries" / round_id / "audit.json"
    write_json(audit_path, result)
    if latest_artifact:
        write_json(latest_artifact / "audit.json", result)
    args._attempt_inputs_digest = inputs["digest"]
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT_OK if not failures else EXIT_EVIDENCE


def cleancheck(args: argparse.Namespace) -> int:
    """Exercise tracked inputs in an isolated project with declared shared caches."""
    round_id = normalize_round(args.round)
    suite_path = suite_path_from_args(args)
    suite = load_and_validate_suite(suite_path, round_id)
    selection = select_toolchain()
    run_id = safe_id(args.run_id or f"clean-{stamp()}", "run-id")
    source_ref = args.source_ref
    source_commit = git_value("rev-parse", f"{source_ref}^{{commit}}")
    if source_commit is None:
        raise ToolFailure(f"无法解析 cleancheck source ref: {source_ref}", EXIT_EVIDENCE)
    clean_root = LOCAL / "cleanroom" / run_id
    project = clean_root / "repo"
    if clean_root.exists():
        raise ToolFailure(f"cleancheck run-id 已存在: {run_id}", EXIT_EVIDENCE)
    clean_root.mkdir(parents=True)
    archive_path = clean_root / "tracked.tar"
    with archive_path.open("wb") as handle:
        archive = subprocess.run(
            ["git", "archive", "--format=tar", source_commit],
            cwd=ROOT,
            stdout=handle,
            stderr=subprocess.PIPE,
            check=False,
        )
    if archive.returncode != 0:
        raise ToolFailure(
            f"创建 tracked-only clean copy 失败: {archive.stderr.decode(errors='replace')}",
            EXIT_SUBPROCESS,
        )
    project.mkdir()
    with tarfile.open(archive_path, "r:") as bundle:
        bundle.extractall(project, filter="data")
    initial_absence = {
        ".local": not (project / ".local").exists(),
        "bot/Cargo.toml": not (project / "bot" / "Cargo.toml").exists(),
        "bot/Cargo.lock": not (project / "bot" / "Cargo.lock").exists(),
        "bot/src/core.rs": not (project / "bot" / "src" / "core.rs").exists(),
        "engine_binary": not (project / "bot" / ".mm" / "bin" / "mm-engine").exists(),
    }
    if not all(initial_absence.values()):
        raise ToolFailure(f"clean copy 起始条件不满足: {initial_absence}", EXIT_EVIDENCE)

    source_lock = load_json(UPSTREAM_LOCK)
    seeded_archives: list[dict[str, Any]] = []
    download_dir = project / ".local" / "downloads"
    download_dir.mkdir(parents=True)
    for name, source in source_lock["sources"].items():
        origin = resolve_repo_path(source["local_archive"])
        if not origin.is_file() or sha256_file(origin) != source["archive_sha256"]:
            raise ToolFailure(f"cleancheck 固定来源缓存缺失: {name}", EXIT_EVIDENCE)
        destination = project / source["local_archive"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(origin, destination)
        seeded_archives.append({"source": name, **file_evidence(origin)})

    child_env = os.environ.copy()
    for name, path in selection["executables"].items():
        if path:
            child_env[f"MM32_{name.upper()}"] = path
    child_env["MM32_RUST_TOOLCHAIN"] = selection["channel"]
    child_env["MM32_BUILD_PROFILE"] = selection["build_profile"]
    child_env["RUSTUP_HOME"] = selection["rustup_home"]
    child_env["CARGO_HOME"] = selection["cargo_home"]
    child_env["CARGO_NET_OFFLINE"] = "true"
    try:
        child_suite = suite_path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError as exc:
        raise ToolFailure("cleancheck suite 必须位于 Git 仓库内", EXIT_EVIDENCE) from exc
    python = sys.executable
    sequence = [
        ("bootstrap_first", [python, "tools/mm32.py", "bootstrap"]),
        (
            "test",
            [python, "tools/mm32.py", "test", "--round", round_id, "--suite", child_suite],
        ),
        (
            "timed_match",
            [
                python,
                "tools/mm32.py",
                "smoke",
                "--round",
                round_id,
                "--suite",
                child_suite,
                "--run-id",
                "acceptance",
            ],
        ),
        (
            "packcheck",
            [
                python,
                "tools/mm32.py",
                "packcheck",
                "--round",
                round_id,
                "--suite",
                child_suite,
                "--run-id",
                "acceptance",
            ],
        ),
        ("bootstrap_second", [python, "tools/mm32.py", "bootstrap"]),
    ]
    commands: list[dict[str, Any]] = []
    first_bootstrap: dict[str, Any] | None = None
    second_bootstrap: dict[str, Any] | None = None
    for label, argv in sequence:
        record = run_command(
            argv,
            cwd=project,
            env=child_env,
            timeout=1800,
            log_path=clean_root / "logs" / f"{label}.log",
        )
        commands.append({"label": label, **command_for_evidence(record)})
        require_ok(record, f"cleancheck {label}")
        if label == "bootstrap_first":
            first_bootstrap = load_json(project / ".local" / "records" / "bootstrap-latest.json")
        elif label == "bootstrap_second":
            second_bootstrap = load_json(project / ".local" / "records" / "bootstrap-latest.json")
    assert first_bootstrap is not None and second_bootstrap is not None
    second_build_records = second_bootstrap.get("build_records", {})
    idempotent = bool(
        second_bootstrap.get("source_actions") == []
        and second_bootstrap.get("restored_scaffold") == []
        and second_bootstrap.get("strategy_sha256_before")
        == second_bootstrap.get("strategy_sha256_after")
        and set(second_build_records) == set(source_lock["builds"])
        and all(
            record.get("reuse") == "matched_target_fingerprint_and_binary_sha256"
            for record in second_build_records.values()
        )
    )
    if not idempotent:
        raise ToolFailure("第二次 bootstrap 未通过幂等检查", EXIT_EVIDENCE)
    child_toolchain = first_bootstrap.get("toolchain", {})
    compiler_fingerprint = child_toolchain.get("compiler_fingerprint")
    if not compiler_fingerprint:
        raise ToolFailure("cleancheck 缺少子环境工具链身份", EXIT_EVIDENCE)
    result = {
        "schema_version": 2,
        "generated_at": utc_now(),
        "status": "ok",
        "round": round_id,
        "suite_id": suite["suite_id"],
        "run_id": run_id,
        "suite": file_evidence(suite_path),
        "source_ref": source_ref,
        "source_commit": source_commit,
        "toolchain_compiler_fingerprint": compiler_fingerprint,
        "tracked_archive_sha256": sha256_file(archive_path),
        "project_root": rel(project),
        "initial_absence": initial_absence,
        "shared_cache_conditions": {
            "network_disabled_for_cargo": True,
            "fixed_source_archives_seeded_after_initial_check": seeded_archives,
            "cargo_home": redact_arg(selection["cargo_home"]),
            "cargo_home_scope": selection["cargo_home_scope"],
            "rustup_home": redact_arg(selection["rustup_home"]),
            "rustup_home_scope": selection["rustup_home_scope"],
        },
        "strategy_sha256_before": first_bootstrap.get("strategy_sha256_before"),
        "strategy_sha256_after_first": first_bootstrap.get("strategy_sha256_after"),
        "strategy_sha256_after_second": second_bootstrap.get("strategy_sha256_after"),
        "first_restore": {
            "source_actions": first_bootstrap.get("source_actions"),
            "restored_scaffold": first_bootstrap.get("restored_scaffold"),
            "build_records": redact_record(first_bootstrap.get("build_records")),
        },
        "second_bootstrap_idempotent": idempotent,
        "commands": commands,
    }
    write_json(RECORDS / "cleancheck-latest.json", result)
    artifact_dir = ROOT / "artifacts" / "summaries" / round_id / "cleancheck" / run_id
    write_json(artifact_dir / "manifest.json", result)
    args._attempt_inputs_digest = canonical_sha256(
        {
            "source_commit": source_commit,
            "suite_sha256": sha256_file(suite_path),
            "toolchain_lock": sha256_file(TOOLCHAIN_LOCK),
        }
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT_OK


def parser() -> argparse.ArgumentParser:
    cli = argparse.ArgumentParser(description=__doc__)
    sub = cli.add_subparsers(dest="command", required=True)
    def add_suite_args(command: argparse.ArgumentParser, *, run_id: bool = False) -> None:
        command.add_argument("--round", default=DEFAULT_ROUND, help="证据轮次，如 m02 或 2")
        command.add_argument(
            "--suite", default=str(DEFAULT_SUITE_PATH), help="仓库相对或绝对 suite JSON 路径"
        )
        if run_id:
            command.add_argument("--run-id", help="唯一批次 ID；已存在时拒绝覆盖")

    doctor_parser = sub.add_parser("doctor", help="分开检查本地工具链与官方声明兼容性")
    add_suite_args(doctor_parser)
    doctor_parser.set_defaults(func=doctor)
    sub.add_parser("bootstrap", help="按来源锁恢复工程并生成本机构建记录").set_defaults(func=bootstrap)
    test_parser = sub.add_parser("test", help="格式、bot、规则、工具、release 与 diff 检查")
    add_suite_args(test_parser)
    test_parser.set_defaults(func=test_all)
    smoke_parser = sub.add_parser("smoke", help="按显式 round/suite/run-id 串行运行正式计时比赛")
    add_suite_args(smoke_parser, run_id=True)
    smoke_parser.set_defaults(func=smoke)
    pack_parser = sub.add_parser("packcheck", help="干净官方工程中的策略打包闭包检查")
    add_suite_args(pack_parser, run_id=True)
    pack_parser.set_defaults(func=packcheck)
    audit_parser = sub.add_parser("audit", help="来源、构建、秘密、边界和证据新鲜度审计")
    add_suite_args(audit_parser)
    audit_parser.set_defaults(func=audit)
    clean_parser = sub.add_parser("cleancheck", help="tracked-only 独立副本的冷恢复验收")
    add_suite_args(clean_parser, run_id=True)
    clean_parser.set_defaults(func=cleancheck)
    clean_parser.add_argument("--source-ref", default="HEAD", help="用于 git archive 的提交或 ref")
    return cli


def initial_attempt_digest(args: argparse.Namespace) -> str | None:
    try:
        if args.command == "bootstrap":
            return canonical_sha256(
                {
                    "upstream_lock": file_sha_or_none(UPSTREAM_LOCK),
                    "toolchain_lock": file_sha_or_none(TOOLCHAIN_LOCK),
                    "strategy": sha256_tree(ROOT / "bot" / "src" / "strategy"),
                }
            )
        if hasattr(args, "suite"):
            round_id = normalize_round(args.round)
            suite_path = suite_path_from_args(args)
            suite = load_and_validate_suite(suite_path, round_id)
            return suite_input_fingerprints(suite_path, suite)["digest"]
    except (OSError, ToolFailure, KeyError, TypeError):
        return None
    return None


def main() -> int:
    args = parser().parse_args()
    attempt_digest = initial_attempt_digest(args)
    attempt_base = {
        "argv": [redact_arg(item) for item in sys.argv[1:]],
        "inputs_digest": attempt_digest,
    }
    write_attempt(args.command, "running", **attempt_base)
    try:
        code = int(args.func(args))
        attempt_base["inputs_digest"] = getattr(args, "_attempt_inputs_digest", attempt_digest)
        write_attempt(
            args.command,
            "ok" if code == 0 else "failed",
            exit_code=code,
            **attempt_base,
        )
        return code
    except ToolFailure as exc:
        attempt_base["inputs_digest"] = getattr(args, "_attempt_inputs_digest", attempt_digest)
        write_attempt(args.command, "failed", exit_code=exc.code, error=str(exc), **attempt_base)
        print(f"[mm32] ERROR: {exc}", file=sys.stderr)
        return exc.code
    except KeyboardInterrupt:
        write_attempt(args.command, "failed", exit_code=130, error="interrupted", **attempt_base)
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
            **attempt_base,
        )
        print(f"[mm32] ERROR: unexpected {type(exc).__name__}: {exc}; see {rel(log_path)}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
