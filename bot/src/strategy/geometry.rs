use crate::core::*;

use super::policy::{
    EXTRA_CLEARANCE, FORMATION_EXTRA_RING, FORMATION_MIN_RING, FORMATION_SLOTS,
    FORMATION_START_DEGREES, FORMATION_STEP_DEGREES, MINING_ANGLE_OFFSETS, PAYLOAD_SPREAD_DEGREES,
};

pub fn mining_spot(conf: &GameConfig, deposit: Vec2, ordinal: usize) -> Vec2 {
    let centre = Vec2::new(MAP_SIZE as f32 * 0.5, MAP_SIZE as f32 * 0.5);
    let base = (centre - deposit).normalize_or_else(|| Vec2::new(0.0, -1.0));
    let ring = conf.deposit.radius + conf.bot.radius + EXTRA_CLEARANCE;
    for shift in 0..MINING_ANGLE_OFFSETS.len() {
        let angle = MINING_ANGLE_OFFSETS[(ordinal + shift) % MINING_ANGLE_OFFSETS.len()];
        let candidate = deposit + base.rotate_deg(angle) * ring;
        if point_free(conf, candidate)
            && has_line_of_sight(conf, candidate, deposit)
            && path_length(conf, candidate, deposit).is_some()
        {
            return candidate;
        }
    }
    deposit + base * ring
}

pub fn payload_control_spot(conf: &GameConfig, payload: Vec2, home: Vec2, id: BotId) -> Vec2 {
    let base = (home - payload).normalize_or_else(|| Vec2::new(-1.0, 0.0));
    let spread = match id % 3 {
        0 => 0.0,
        1 => PAYLOAD_SPREAD_DEGREES,
        _ => -PAYLOAD_SPREAD_DEGREES,
    };
    let radius = conf.payload.radius + conf.bot.radius + EXTRA_CLEARANCE;
    let candidate = payload + base.rotate_deg(spread) * radius;
    if point_free(conf, candidate) {
        candidate
    } else {
        payload + base * radius
    }
}

pub fn formation_spot(conf: &GameConfig, payload: Vec2, home: Vec2, id: BotId) -> Vec2 {
    let base = (home - payload).normalize_or_else(|| Vec2::new(-1.0, 0.0));
    let angle =
        FORMATION_START_DEGREES + (id as f32 % FORMATION_SLOTS as f32) * FORMATION_STEP_DEGREES;
    let max_ring =
        (conf.payload.capture_radius - conf.bot.radius - EXTRA_CLEARANCE).max(FORMATION_MIN_RING);
    let ring = (conf.payload.radius + conf.bot.radius + FORMATION_EXTRA_RING).min(max_ring);
    let candidate = payload + base.rotate_deg(angle) * ring;
    if point_free(conf, candidate) {
        candidate
    } else {
        payload_control_spot(conf, payload, home, id)
    }
}

pub fn next_facing(current: f32, desired: f32, max_turn: f32) -> f32 {
    normalize_degrees(current + diff_degrees(desired, current).clamp(-max_turn, max_turn))
}

pub fn finite_vec(v: Vec2) -> bool {
    v.x.is_finite() && v.y.is_finite()
}
