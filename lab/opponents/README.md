# M02 challenge opponents

This directory contains two frozen, candidate-independent challenge policies. Both are
standalone replacements for the official starterpack's `src/strategy/main.rs` and import
only `crate::core::*`; neither imports `bot/src/strategy/`.

## Versioned behaviour

- `support_screen_v1` (`support_screen/main.rs`): builds two extractors, maintains roughly
  one healer per two battles, assigns battle bots stable and distinct payload-screen
  positions, keeps full-health healers moving with the nearest battle anchor, caps effective
  current healing at the official three-channel limit, and withholds shots until the
  post-move/post-turn ray can cover the selected enemy.
- `strafe_pressure_v1` (`strafe_pressure/main.rs`): alternates clockwise/counter-clockwise
  tangents by bot id, corrects radially toward 72% of blaster range, follows the nearest
  battle with support bots, and withholds pressure shots until the post-move/post-turn ray
  can cover the selected enemy.

The strategy ids are emitted once at process startup as `MM32_OPPONENT` records. Formal run
manifests additionally record the exact source SHA-256 and compiled binary SHA-256; those
run-bound hashes, rather than this prose, are authoritative for a comparison batch.

## Behaviour contracts

`mm32-opponent-contracts` compiles the real two single-file implementations against the
pinned bot-client commit `879578d448251d0acc7f10eef3d7fe5450758096`. Its five tests cover:

1. full-health healer movement toward a battle anchor;
2. the three-channel effective-healing cap;
3. distinct payload-screen approaches;
4. opposite strafe tangents plus near-range separation; and
5. aligned-shot acceptance and turning-shot rejection.

Run with the project-selected Cargo/Rust toolchain:

```text
cargo test --offline --manifest-path lab/opponents/Cargo.toml
```

The 2026-09-19 local acceptance run completed 5/5 tests. Each standalone opponent also
completed an official starterpack release build. The only emitted build diagnostic was the
already tracked local `rust-objcopy`/`libLLVM.dylib` stripping warning; compilation exited 0.

## Controlled lifecycle soak

`lab/fixtures/m02_soak_static_payload.json` is equivalent in parsed JSON to
`lab/fixtures/default_config.json` except for `payload.speed: 0.02 -> 0.0` and
`bot.blaster_damage: 3.0 -> 0.0`. The default fixture already starts endgame at tick 6000
and ends at tick 9000. Freezing payload progress and damage prevents the two normal early
termination paths, so the two-direction v1 self-play plan in `configs/suites/m02_soak.json`
forces both games to exercise the 6000-9000 lifecycle window unless there is an engine/bot
failure.

This suite is classified `soak_scenario`: it is not part of the default-initial-state formal
32-game comparison, and no result is claimed until the timed suite has actually run and its
manifest validates `final_tick` and callback/budget coverage.
