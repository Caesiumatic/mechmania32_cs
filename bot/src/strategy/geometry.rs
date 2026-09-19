use crate::core::*;

const EXTRA_CLEARANCE: f32 = 0.08;

pub fn mining_spot(conf: &GameConfig, deposit: Vec2, ordinal: usize) -> Vec2 {
    let centre = Vec2::new(MAP_SIZE as f32 * 0.5, MAP_SIZE as f32 * 0.5);
    let base = (centre - deposit).normalize_or_else(|| Vec2::new(0.0, -1.0));
    let ring = conf.deposit.radius + conf.bot.radius + EXTRA_CLEARANCE;
    const OFFSETS: [f32; 8] = [0.0, 35.0, -35.0, 70.0, -70.0, 110.0, -110.0, 180.0];

    for shift in 0..OFFSETS.len() {
        let angle = OFFSETS[(ordinal + shift) % OFFSETS.len()];
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
        1 => 18.0,
        _ => -18.0,
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
    let angle = -75.0 + (id as f32 % 7.0) * 25.0;
    let max_ring = (conf.payload.capture_radius - conf.bot.radius - EXTRA_CLEARANCE).max(0.5);
    let ring = (conf.payload.radius + conf.bot.radius + 0.65).min(max_ring);
    let candidate = payload + base.rotate_deg(angle) * ring;
    if point_free(conf, candidate) {
        candidate
    } else {
        payload_control_spot(conf, payload, home, id)
    }
}

pub fn support_spot(conf: &GameConfig, target: Vec2, id: BotId) -> Vec2 {
    let angle = (id as f32 * 137.507_77) % 360.0;
    let radius = (conf.bot.base_heal_range * 0.55).max(conf.bot.radius * 3.0);
    let candidate = target + Vec2::from_angle_deg(angle) * radius;
    if point_free(conf, candidate) {
        candidate
    } else {
        target
    }
}

pub fn next_facing(current: f32, desired: f32, max_turn: f32) -> f32 {
    normalize_degrees(current + diff_degrees(desired, current).clamp(-max_turn, max_turn))
}

pub fn ray_circle_entry(origin: Vec2, direction: Vec2, centre: Vec2, radius: f32) -> Option<f32> {
    let dir = direction.normalize_or_zero();
    if dir.norm_sq() == 0.0 {
        return None;
    }
    let rel = centre - origin;
    let along = rel.dot(dir);
    let perp_sq = rel.norm_sq() - along * along;
    let radius_sq = radius * radius;
    if perp_sq > radius_sq {
        return None;
    }
    let half = (radius_sq - perp_sq).max(0.0).sqrt();
    let near = along - half;
    let far = along + half;
    if far < 0.0 {
        None
    } else {
        Some(near.max(0.0))
    }
}

pub fn finite_vec(v: Vec2) -> bool {
    v.x.is_finite() && v.y.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_circle_entry_handles_hit_and_miss() {
        let origin = Vec2::ZERO;
        let entry =
            ray_circle_entry(origin, Vec2::new(1.0, 0.0), Vec2::new(3.0, 0.0), 1.0).unwrap();
        assert!((entry - 2.0).abs() < 1e-6);
        assert!(ray_circle_entry(origin, Vec2::new(1.0, 0.0), Vec2::new(3.0, 2.0), 1.0).is_none());
    }
}
