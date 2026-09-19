//! Authoritative, inspectable tuning values for the M02 v1 policy.
//!
//! Rules such as weapon range, healing limits and movement speed continue to come from the
//! handshaked `GameConfig`.  This file contains only strategy choices; keeping them here avoids
//! a second JSON source that the submitted strategy could not read at runtime.

pub const STRATEGY_ID: &str = "m02-v1";
pub const PARAMETER_SET_ID: &str = "m02-v1-static-2026-09-19";
pub const LEARNED_PARAMETER_COUNT: usize = 0;

// Economy and budget degradation.
pub const TARGET_EXTRACTORS: u8 = 4;
pub const TARGET_BATTLES: u8 = 9;
pub const TARGET_HEALERS: u8 = 4;
pub const LOW_BUDGET_THRESHOLD: i64 = 100_000;

// Navigation and formation choices.
pub const EXTRA_CLEARANCE: f32 = 0.08;
pub const MINING_ANGLE_OFFSETS: [f32; 8] = [0.0, 35.0, -35.0, 70.0, -70.0, 110.0, -110.0, 180.0];
pub const PAYLOAD_SPREAD_DEGREES: f32 = 18.0;
pub const FORMATION_START_DEGREES: f32 = -75.0;
pub const FORMATION_STEP_DEGREES: f32 = 25.0;
pub const FORMATION_SLOTS: u8 = 7;
pub const FORMATION_EXTRA_RING: f32 = 0.65;
pub const FORMATION_MIN_RING: f32 = 0.5;
pub const ENGAGE_RANGE_FRACTION: f32 = 0.72;

// Greedy same-tick fire allocation. New effective coverage dominates every tie-break term.
pub const NEW_DAMAGE_WEIGHT: i32 = 10_000;
pub const LETHAL_DAMAGE_WEIGHT: i32 = 500;
pub const HEALER_DAMAGE_WEIGHT: i32 = 50;
pub const DIRECT_HIT_WEIGHT: i32 = 5;

// Healer task stability and support-position search.
pub const POSITION_EPSILON: f32 = 1e-4;
pub const ENTITY_CLEARANCE: f32 = 0.06;
pub const SUPPORT_SWITCH_PENALTY: f32 = 0.35;
pub const HEAL_SWITCH_PENALTY: f32 = 0.20;
pub const BATTLE_PRIORITY_BONUS: f32 = 0.15;
pub const SUPPORT_RANGE_FRACTION: f32 = 0.55;
pub const SUPPORT_MIN_BOT_RADII: f32 = 3.0;
pub const SUPPORT_SEARCH_STEPS: usize = 16;
pub const SUPPORT_SEARCH_STEP_DEGREES: f32 = 22.5;
