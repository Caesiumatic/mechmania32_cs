//! Independent contract-test crate for the MechMania 32 engine.
//!
//! The executable behavior lives in the pinned `mm-engine` dependency.  This crate only
//! supplies integration tests, so production bot code cannot accidentally depend on the
//! engine-only feature surface.
