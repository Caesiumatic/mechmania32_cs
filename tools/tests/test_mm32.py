from __future__ import annotations

import importlib.util
import json
import os
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).resolve().parents[1] / "mm32.py"
SPEC = importlib.util.spec_from_file_location("mm32", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
mm32 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mm32)


def minimal_suite(fixture: Path) -> dict:
    return {
        "schema_version": 2,
        "round": "m02",
        "suite_id": "unit-suite",
        "fixture_path": str(fixture),
        "engine": {
            "kind": "fixture_build",
            "base_source_commit": "4cbd3467e6617e330d42cb78ae3327ab2c88926a",
        },
        "time_enforcement": True,
        "no_time_limit": False,
        "match_timeout_seconds": 10,
        "expected_matches": 1,
        "candidates": [
            {
                "id": "candidate",
                "source": {"kind": "working_strategy_tree", "path": "bot/src/strategy"},
            }
        ],
        "opponents": [
            {
                "id": "opponent",
                "source": {
                    "kind": "single_file",
                    "path": "lab/opponents/starter_symmetric/main.rs",
                },
            }
        ],
        "matches": [
            {
                "id": "match-1",
                "candidate": "candidate",
                "opponent": "opponent",
                "candidate_side": "a",
                "repetition": 1,
            }
        ],
    }


class EnvironmentSelectionTests(unittest.TestCase):
    def test_explicit_environment_has_priority(self) -> None:
        env = {
            "PATH": "/caller/bin:/usr/bin",
            "MM32_CARGO": "/explicit/cargo",
            "MM32_RUSTC": "/explicit/rustc",
            "MM32_RUSTUP": "/explicit/rustup",
            "MM32_RUSTFMT": "/explicit/rustfmt",
            "MM32_RUST_TOOLCHAIN": "stable",
            "RUSTUP_HOME": "/shared/rustup",
            "CARGO_HOME": "/shared/cargo",
        }
        selected = mm32.select_toolchain(env)
        self.assertEqual(selected["executables"]["cargo"], "/explicit/cargo")
        self.assertEqual(selected["executable_sources"]["cargo"], "environment:MM32_CARGO")
        self.assertEqual(selected["rustup_home_scope"], "shared")
        self.assertEqual(selected["cargo_home_scope"], "shared")

    def test_tool_env_preserves_callers_path(self) -> None:
        env = {
            "PATH": "/caller/one:/caller/two",
            "MM32_CARGO": "/explicit/cargo",
            "MM32_RUSTC": "/explicit/rustc",
            "MM32_RUSTUP": "/explicit/rustup",
            "MM32_RUSTFMT": "/explicit/rustfmt",
            "RUSTUP_HOME": "/shared/rustup",
            "CARGO_HOME": "/shared/cargo",
        }
        selection = mm32.select_toolchain(env)
        with mock.patch.dict(os.environ, env, clear=True):
            result = mm32.tool_env("unit-target", selection)
        self.assertIn("/caller/one", result["PATH"].split(os.pathsep))
        self.assertIn("/caller/two", result["PATH"].split(os.pathsep))
        self.assertTrue(result["CARGO_TARGET_DIR"].endswith(".local/target/unit-target"))

    def test_complete_caller_path_infers_caller_homes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary_dir = root / "bin"
            binary_dir.mkdir()
            for name in ("cargo", "rustc", "rustup", "rustfmt"):
                executable = binary_dir / name
                executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                executable.chmod(0o755)
            env = {"PATH": str(binary_dir), "HOME": str(root / "home")}
            selected = mm32.select_toolchain(env)
        self.assertTrue(selected["caller_toolchain_detected"])
        self.assertEqual(selected["cargo_home"], str(root / "home" / ".cargo"))
        self.assertEqual(selected["rustup_home"], str(root / "home" / ".rustup"))
        self.assertEqual(selected["cargo_home_source"], "inferred_caller_default")


class SuiteValidationTests(unittest.TestCase):
    def write_suite(self, directory: Path, value: dict) -> Path:
        path = directory / "suite.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_valid_suite_accepts_explicit_round(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            fixture = directory / "fixture.json"
            fixture.write_text("{}\n", encoding="utf-8")
            suite = minimal_suite(fixture)
            loaded = mm32.load_and_validate_suite(self.write_suite(directory, suite), "2")
            self.assertEqual(loaded["suite_id"], "unit-suite")

    def test_duplicate_matrix_cell_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            fixture = directory / "fixture.json"
            fixture.write_text("{}\n", encoding="utf-8")
            suite = minimal_suite(fixture)
            duplicate = dict(suite["matches"][0])
            duplicate["id"] = "match-2"
            suite["matches"].append(duplicate)
            suite["expected_matches"] = 2
            with self.assertRaises(mm32.ToolFailure):
                mm32.load_and_validate_suite(self.write_suite(directory, suite), "m02")

    def test_time_enforcement_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            fixture = directory / "fixture.json"
            fixture.write_text("{}\n", encoding="utf-8")
            suite = minimal_suite(fixture)
            suite["time_enforcement"] = False
            with self.assertRaises(mm32.ToolFailure):
                mm32.load_and_validate_suite(self.write_suite(directory, suite), "m02")


class EvidenceTests(unittest.TestCase):
    def test_python_source_fingerprint_ignores_bytecode_cache(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "test_example.py"
            source.write_text("VALUE = 1\n", encoding="utf-8")
            initial = mm32.sha256_python_sources(root)
            cache = root / "__pycache__"
            cache.mkdir()
            (cache / "test_example.cpython.pyc").write_bytes(b"generated")
            self.assertEqual(mm32.sha256_python_sources(root), initial)
            source.write_text("VALUE = 2\n", encoding="utf-8")
            self.assertNotEqual(mm32.sha256_python_sources(root), initial)

    def test_evaluated_commit_allows_only_versioned_evidence_paths(self) -> None:
        repository = {
            "head": "abc123",
            "status_porcelain": [
                "?? artifacts/summaries/m02/metrics.json",
                "?? docs/reports/mechmania32_report2.md",
                "?? docs/rules/mechmania32_rules2.md",
                "?? docs/mechmania32_index2.md",
            ],
        }
        self.assertEqual(mm32.evaluated_code_commit(repository), "abc123")
        repository["status_porcelain"].append(" M bot/src/strategy/main.rs")
        self.assertIsNone(mm32.evaluated_code_commit(repository))

    def test_current_evidence_requires_matching_successful_attempt(self) -> None:
        inputs = {"digest": "abc", "suite": {"sha256": "123"}}
        record = {"status": "ok", "inputs": inputs}
        attempt = {"status": "ok", "inputs_digest": "abc"}
        self.assertTrue(mm32.evidence_is_current(record, attempt, inputs))
        self.assertFalse(mm32.evidence_is_current(record, {**attempt, "status": "failed"}, inputs))
        self.assertFalse(mm32.evidence_is_current(record, {**attempt, "inputs_digest": "old"}, inputs))

    def test_game_log_parser_extracts_config_result_and_elapsed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "game.mmgl"
            path.write_text(
                '{"max_ticks":9000}\n# result: {"winner":"A","tick":12,"reason":"x"}\n'
                "# time elapsed: 0.25s\n",
                encoding="utf-8",
            )
            config, result, elapsed = mm32.parse_game_log(path)
        self.assertEqual(config, {"max_ticks": 9000})
        self.assertEqual(result["winner"], "A")
        self.assertEqual(elapsed, "0.25s")

    def test_callback_coverage_rejects_skipped_or_incomplete_lifecycle(self) -> None:
        result = {"tick": 10}
        complete = [{"first_tick": 0, "last_tick": 9, "calls": 10, "skipped_ticks": 0}]
        self.assertTrue(mm32.callback_coverage_complete(result, complete))
        self.assertFalse(
            mm32.callback_coverage_complete(result, [{**complete[0], "skipped_ticks": 1}])
        )
        self.assertFalse(mm32.callback_coverage_complete(result, [{**complete[0], "calls": 9}]))

    def test_matrix_and_soak_lifecycle_contracts_are_exact(self) -> None:
        suite = {
            "classification": "soak_scenario",
            "matches": [
                {
                    "id": "a",
                    "candidate": "v1",
                    "opponent": "frozen",
                    "candidate_side": "a",
                    "repetition": 1,
                },
                {
                    "id": "b",
                    "candidate": "v1",
                    "opponent": "frozen",
                    "candidate_side": "b",
                    "repetition": 1,
                },
            ],
        }
        matches = [
            {**suite["matches"][0], "candidate_side": "A", "final_tick": 9_000},
            {**suite["matches"][1], "candidate_side": "B", "final_tick": 6_000},
        ]
        self.assertTrue(mm32.suite_matrix_matches(suite, matches))
        self.assertTrue(mm32.long_game_coverage(suite, matches)["complete"])
        self.assertFalse(
            mm32.suite_matrix_matches(suite, [matches[0], {**matches[1], "repetition": 2}])
        )
        self.assertFalse(
            mm32.long_game_coverage(
                suite, [{**matches[0], "final_tick": 8_999}, matches[1]]
            )["complete"]
        )

    def test_round_normalization_and_run_id_validation(self) -> None:
        self.assertEqual(mm32.normalize_round("2"), "m02")
        self.assertEqual(mm32.normalize_round("M02"), "m02")
        self.assertEqual(mm32.safe_id("run-01.alpha", "run"), "run-01.alpha")
        with self.assertRaises(mm32.ToolFailure):
            mm32.safe_id("../escape", "run")


class AuditValidationTests(unittest.TestCase):
    def test_upstream_checkout_must_match_pinned_archive_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "checkout"
            source.mkdir()
            (source / "Cargo.toml").write_text("[package]\nname='x'\n", encoding="utf-8")
            archive = root / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                bundle.add(source, arcname="source-root")
            upstream = {
                "sources": {
                    "engine": {
                        "local_archive": "source.tar.gz",
                        "archive_sha256": mm32.sha256_file(archive),
                        "local_source": "checkout",
                        "file_sha256": {
                            "Cargo.toml": mm32.sha256_file(source / "Cargo.toml")
                        },
                    }
                }
            }
            with mock.patch.object(mm32, "ROOT", root), mock.patch.object(
                mm32, "LOCAL", root / ".local"
            ):
                self.assertTrue(all(mm32.validate_upstream_source_snapshots(upstream).values()))
                (source / "src.rs").write_text("mutated\n", encoding="utf-8")
                checks = mm32.validate_upstream_source_snapshots(upstream)
        self.assertFalse(checks["engine_checkout_matches_archive_tree"])

    def test_build_record_requires_successful_component_and_command(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            (source / "Cargo.toml").write_text("manifest\n", encoding="utf-8")
            (source / "Cargo.lock").write_text("lock\n", encoding="utf-8")
            binary = root / "bin" / "engine"
            binary.parent.mkdir()
            binary.write_bytes(b"binary")
            records = root / "records"
            (records / "builds").mkdir(parents=True)
            toolchain = {
                "build_profile": "release",
                "fingerprint": "toolchain",
                "target_triple": "test-target",
            }
            source_record = {
                "source_commit": "commit",
                "source_tree_sha256": mm32.sha256_tree(source),
                "manifest_sha256": mm32.sha256_file(source / "Cargo.toml"),
                "cargo_lock_sha256": mm32.sha256_file(source / "Cargo.lock"),
                "binary_name": "engine",
                "profile": "release",
                "toolchain_fingerprint": "toolchain",
                "target_triple": "test-target",
            }
            record = {
                "status": "ok",
                "component": "engine",
                "source": source_record,
                "build_fingerprint": mm32.canonical_sha256(source_record),
                "target_triple": "test-target",
                "binary": {
                    "path": "bin/engine",
                    "sha256": mm32.sha256_file(binary),
                    "bytes": binary.stat().st_size,
                },
                "command": {"exit_code": 0, "timed_out": False},
            }
            record_path = records / "builds" / "engine.json"
            record_path.write_text(json.dumps(record), encoding="utf-8")
            upstream = {
                "builds": {
                    "engine": {
                        "source_commit": "commit",
                        "source_path": "source",
                        "output_path": "bin/engine",
                        "binary_name": "engine",
                    }
                }
            }
            with mock.patch.object(mm32, "ROOT", root), mock.patch.object(
                mm32, "RECORDS", records
            ):
                self.assertTrue(
                    all(mm32.validate_local_build_records(upstream, toolchain).values())
                )
                record["status"] = "failed"
                record_path.write_text(json.dumps(record), encoding="utf-8")
                checks = mm32.validate_local_build_records(upstream, toolchain)
        self.assertFalse(checks["engine_status"])

    def test_cleancheck_rejects_empty_absence_and_wrong_stage_list(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            records = root / ".local" / "records"
            records.mkdir(parents=True)
            suite = root / "configs" / "suites" / "m02_cleancheck.json"
            suite.parent.mkdir(parents=True)
            suite.write_text("{}\n", encoding="utf-8")
            run_dir = root / "artifacts" / "summaries" / "m02" / "cleancheck" / "run"
            run_dir.mkdir(parents=True)
            manifest = {
                "status": "ok",
                "round": "m02",
                "suite_id": "m02-cleancheck",
                "run_id": "run",
                "source_commit": "commit",
                "suite": {
                    "path": "configs/suites/m02_cleancheck.json",
                    "sha256": mm32.sha256_file(suite),
                },
                "initial_absence": {},
                "second_bootstrap_idempotent": True,
                "shared_cache_conditions": {"network_disabled_for_cargo": True},
                "toolchain_compiler_fingerprint": "compiler",
                "commands": [
                    {"label": label, "exit_code": 0, "timed_out": False}
                    for label in (
                        "bootstrap_first",
                        "test",
                        "timed_match",
                        "packcheck",
                        "bootstrap_second",
                    )
                ],
            }
            manifest_path = run_dir / "manifest.json"
            latest_path = records / "cleancheck-latest.json"
            for path in (manifest_path, latest_path):
                path.write_text(json.dumps(manifest), encoding="utf-8")
            with mock.patch.object(mm32, "ROOT", root), mock.patch.object(
                mm32, "RECORDS", records
            ):
                empty_absence = mm32.validate_cleancheck_artifact(
                    "m02", "commit", {"compiler_fingerprint": "compiler"}
                )
                manifest["initial_absence"] = {
                    ".local": True,
                    "bot/Cargo.toml": True,
                    "bot/Cargo.lock": True,
                    "bot/src/core.rs": True,
                    "engine_binary": True,
                }
                manifest["commands"][2]["label"] = "wrong_stage"
                for path in (manifest_path, latest_path):
                    path.write_text(json.dumps(manifest), encoding="utf-8")
                wrong_stage = mm32.validate_cleancheck_artifact(
                    "m02", "commit", {"compiler_fingerprint": "compiler"}
                )
                manifest["commands"][2]["label"] = "timed_match"
                for path in (manifest_path, latest_path):
                    path.write_text(json.dumps(manifest), encoding="utf-8")
                complete = mm32.validate_cleancheck_artifact(
                    "m02", "commit", {"compiler_fingerprint": "compiler"}
                )
        self.assertFalse(empty_absence["current"])
        self.assertFalse(wrong_stage["current"])
        self.assertTrue(complete["current"])


class ParsingTests(unittest.TestCase):
    def test_rust_and_unittest_totals(self) -> None:
        rust = "test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured"
        self.assertEqual(mm32.parse_test_totals([rust]), {"passed": 5, "failed": 0, "skipped": 1})
        python = "Ran 7 tests in 0.01s\n\nOK (skipped=2)\n"
        self.assertEqual(
            mm32.parse_unittest_totals(python, 0),
            {"passed": 5, "failed": 0, "skipped": 2},
        )

    def test_behavior_aggregation_preserves_denominators_and_weighted_means(self) -> None:
        summaries = [
            {
                "fire_requests": 4,
                "fire_feedback_observable": 3,
                "fire_observed": 2,
                "fire_feedback_censored": 1,
                "blast_error_samples": 2,
                "blast_error_mean": 1.0,
                "blast_error_max": 1.5,
                "actual_new_damaged_targets": 2,
                "damage_transition_observable": 5,
                "damage_transition_censored": 1,
                "heal_requests": 3,
                "heal_feedback_observable": 2,
                "heal_observed_channels": 2,
                "heal_feedback_censored": 1,
                "planned_effective_heal_channels": 2,
                "demand_limited_heal_capacity_slots": 4,
                "full_health_follow_samples": 2,
                "full_health_follow_distance_mean": 3.0,
                "full_health_follow_distance_max": 4.0,
                "control_observation_ticks": 10,
                "contested_control_ticks": 4,
                "high_population_ticks": 3,
                "friendly_pair_samples": 8,
                "near_neighbor_pairs": 2,
            },
            {
                "fire_requests": 2,
                "fire_feedback_observable": 1,
                "fire_observed": 1,
                "fire_feedback_censored": 1,
                "blast_error_samples": 1,
                "blast_error_mean": 4.0,
                "blast_error_max": 4.0,
                "actual_new_damaged_targets": 1,
                "damage_transition_observable": 2,
                "damage_transition_censored": 2,
                "heal_requests": 1,
                "heal_feedback_observable": 1,
                "heal_observed_channels": 1,
                "heal_feedback_censored": 0,
                "planned_effective_heal_channels": 1,
                "demand_limited_heal_capacity_slots": 2,
                "full_health_follow_samples": 1,
                "full_health_follow_distance_mean": 6.0,
                "full_health_follow_distance_max": 6.0,
                "control_observation_ticks": 5,
                "contested_control_ticks": 1,
                "high_population_ticks": 2,
                "friendly_pair_samples": 2,
                "near_neighbor_pairs": 1,
            },
        ]
        result = mm32.aggregate_behavior_summaries(summaries)
        self.assertEqual(result["fire"]["requested"], 6)
        self.assertEqual(result["fire"]["censored"], 2)
        self.assertAlmostEqual(result["blast_error"]["mean"], 2.0)
        self.assertEqual(result["blast_error"]["samples"], 3)
        self.assertEqual(result["damage"]["transition_censored"], 3)
        self.assertEqual(result["healing"]["demand_limited_capacity_slots"], 6)
        self.assertAlmostEqual(result["healing"]["effective_capacity_occupancy"], 0.5)
        self.assertAlmostEqual(result["support"]["full_health_follow_distance_mean"], 4.0)
        self.assertEqual(result["control_and_density"]["observation_ticks"], 15)
        self.assertAlmostEqual(result["control_and_density"]["near_neighbor_pair_rate"], 0.3)


if __name__ == "__main__":
    unittest.main()
