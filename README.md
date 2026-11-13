# MechMania 32 strategy and evaluation

This repository develops a Rust strategy for MechMania 32 and evaluates it
against pinned engine rules and versioned opponents. The workflow connects
strategy code, rule contracts, timed matches, and compact run evidence.

The strategy modules cover economy, movement, targeting, and team coordination.
The lab keeps engine-rule tests and opponent implementations in separate crates,
so comparisons can identify the exact candidate and opponent versions.

## Run the tools

Use the common entry point from the repository root:

```bash
python3 tools/mm32.py --help
```

`doctor` checks the toolchain, `bootstrap` restores pinned dependencies, and
`test` runs the code and contract checks. Match and recovery commands use an
explicit round, suite, and run ID. See the [M2 index](docs/mechmania32_index2.md)
for the versioned suites, rules, reports, and evidence paths.

## Repository layout

| Path | Contents |
| --- | --- |
| [`bot/`](bot/) | Rust strategy source |
| [`lab/`](lab/) | Rule tests and opponent implementations |
| [`configs/`](configs/) | Toolchain and match configuration |
| [`tools/`](tools/) | Build, test, and match utilities |
| [`docs/`](docs/) | Round-specific rules, implementation reports, and evidence indexes |
| [`artifacts/summaries/`](artifacts/summaries/) | Compact metrics and run manifests |

Large replays, logs, dependency checkouts, and build caches live in the ignored
`.local/` directory. Run manifests record source and binary hashes alongside
results; use them to associate a reported outcome with the evaluated version.
