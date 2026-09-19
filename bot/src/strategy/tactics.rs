use crate::core::*;

use super::geometry::{
    formation_spot, next_facing, payload_control_spot, ray_circle_entry, support_spot,
};

fn movement(conf: &GameConfig, from: Vec2, to: Vec2) -> Vec2 {
    if from.dist_sq(&to) <= (conf.bot.speed * 0.5).powi(2) {
        Vec2::ZERO
    } else {
        navigate_to(conf, from, to)
    }
}

/// Mirror `MoveAction::sanitize` before predicting the position used by the engine's
/// turn/fire/heal phases. `navigate_to` deliberately returns a delta divided by speed;
/// values longer than one are capped to a unit direction by the protocol, while shorter
/// values remain proportional so a bot can stop without overshooting.
fn actual_move_delta(direction: Vec2, speed: f32) -> Vec2 {
    let sanitized = if direction.norm_sq() > 1.0 {
        direction.normalize_or_zero()
    } else {
        direction
    };
    sanitized * speed
}

fn dynamic_obstruction_before_target(
    state: &GameState,
    conf: &GameConfig,
    origin: Vec2,
    direction: Vec2,
    target_along: f32,
) -> bool {
    let blockers = [
        (state.payload_pos(), conf.payload.radius),
        (state.deposit_me.pos, conf.deposit.radius),
        (state.deposit_other.pos, conf.deposit.radius),
    ];
    blockers.into_iter().any(|(centre, radius)| {
        ray_circle_entry(origin, direction, centre, radius)
            .is_some_and(|entry| entry + 1e-3 < target_along - conf.bot.radius)
    })
}

pub fn battle_action(
    bot: &BotState,
    target: Option<&BotState>,
    state: &GameState,
    conf: &GameConfig,
    controls_payload: bool,
) -> (BotAction, bool) {
    let payload = state.payload_pos();
    let home = state.deposit_me.pos;
    let predicted_target = target.map(|enemy| enemy.pos + enemy.vel);

    let destination = if controls_payload {
        payload_control_spot(conf, payload, home, bot.id)
    } else if let Some(enemy) = target {
        if bot.pos.dist(&enemy.pos) > conf.bot.blaster_range * 0.72 {
            predicted_target.unwrap_or(enemy.pos)
        } else {
            formation_spot(conf, payload, home, bot.id)
        }
    } else {
        formation_spot(conf, payload, home, bot.id)
    };

    let move_dir = movement(conf, bot.pos, destination);
    let aim = predicted_target.unwrap_or(payload);
    let mut action = BotAction {
        move_action: move_bot(move_dir),
        turn_action: turn_towards(aim),
        special_action: SpecialAction::Battle { fire: false },
        self_destruct: false,
    };

    let Some(enemy) = target else {
        return (action, false);
    };
    if bot.next_fire_tick() > state.tick || enemy.invulnerable_until_tick > state.tick {
        return (action, false);
    }

    // Engine order is move, turn, then fire. Enemy constant-velocity prediction is a v0
    // behavioural assumption; it is kept separate from the static rule contracts.
    let origin = bot.pos + actual_move_delta(move_dir, conf.bot.speed);
    let enemy_pos = enemy.pos + enemy.vel;
    let to_enemy = enemy_pos - origin;
    let distance = to_enemy.norm();
    if distance <= 1e-5 || distance > conf.bot.blaster_range + conf.bot.radius {
        return (action, false);
    }

    let desired = to_enemy.angle_deg();
    let facing = next_facing(bot.angle, desired, conf.bot.turn_speed);
    let direction = Vec2::from_angle_deg(facing);
    let along = to_enemy.dot(direction);
    let perpendicular_sq = (to_enemy.norm_sq() - along * along).max(0.0);
    let hit_radius = conf.bot.radius + conf.bot.base_blaster_splash_radius;
    let geometrically_hits = along >= 0.0
        && along <= conf.bot.blaster_range
        && perpendicular_sq <= hit_radius * hit_radius;
    let static_clear = has_line_of_sight(conf, origin, enemy_pos);
    let dynamic_clear = !dynamic_obstruction_before_target(state, conf, origin, direction, along);

    if geometrically_hits && static_clear && dynamic_clear {
        action.special_action = SpecialAction::Battle { fire: true };
        (action, true)
    } else {
        (action, false)
    }
}

pub fn healer_action(bot: &BotState, target: Option<&BotState>, conf: &GameConfig) -> BotAction {
    let Some(ally) = target else {
        return BotAction {
            special_action: SpecialAction::Healer {
                fire: false,
                target: bot.id,
            },
            ..BotAction::default()
        };
    };

    let destination = support_spot(conf, ally.pos, bot.id);
    let move_dir = if bot.pos.dist(&ally.pos) > conf.bot.base_heal_range * 0.8 {
        movement(conf, bot.pos, destination)
    } else {
        Vec2::ZERO
    };
    let origin = bot.pos + actual_move_delta(move_dir, conf.bot.speed);
    let to_ally = ally.pos - origin;
    let desired = to_ally.angle_deg();
    let facing = next_facing(bot.angle, desired, conf.bot.turn_speed);
    let in_arc = diff_degrees(desired, facing).abs() <= conf.bot.base_heal_arc_deg * 0.5;
    let in_range = to_ally.norm_sq() <= conf.bot.base_heal_range.powi(2);
    let clear = has_line_of_sight(conf, origin, ally.pos);

    BotAction {
        move_action: move_bot(move_dir),
        turn_action: turn_towards(ally.pos),
        special_action: SpecialAction::Healer {
            fire: in_arc && in_range && clear && ally.id != bot.id,
            target: ally.id,
        },
        self_destruct: false,
    }
}

pub fn extractor_action(
    bot: &BotState,
    deposit: Vec2,
    mining_position: Vec2,
    conf: &GameConfig,
) -> BotAction {
    BotAction {
        move_action: move_bot(movement(conf, bot.pos, mining_position)),
        turn_action: turn_towards(deposit),
        special_action: SpecialAction::Extractor { mine: true },
        self_destruct: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicted_move_delta_matches_engine_sanitization() {
        let speed = 0.05;
        let far = actual_move_delta(Vec2::new(120.0, 0.0), speed);
        assert!((far.x - speed).abs() < 1e-6);
        assert!(far.y.abs() < 1e-6);

        let short = actual_move_delta(Vec2::new(0.75, 0.0), speed);
        assert!((short.x - 0.75 * speed).abs() < 1e-6);
        assert!(short.y.abs() < 1e-6);
    }
}
