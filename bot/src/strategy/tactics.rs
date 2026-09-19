use crate::core::*;

use super::ballistics::{
    marginal_damage_mask, trace_shot, Circle, EnemyGeometry, FirstHit, ShotResult, ShotScene,
};
use super::geometry::{formation_spot, next_facing, payload_control_spot};
use super::policy::{
    DIRECT_HIT_WEIGHT, ENGAGE_RANGE_FRACTION, HEALER_DAMAGE_WEIGHT, LETHAL_DAMAGE_WEIGHT,
    NEW_DAMAGE_WEIGHT,
};

/// The part of a fire plan that can be checked against the next callback.  The point is a
/// behaviour prediction (it includes the constant-velocity enemy model), not another
/// geometry oracle result; pure ray correctness is tested separately in `lab/rules`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotPrediction {
    pub blast_point: Vec2,
    pub predicted_new_damage_mask: u32,
}

/// Small fixed-size counters carried from the planner into production telemetry.  Keeping
/// this out of the logger avoids re-running every ray query just to measure the policy.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BattlePlanTelemetry {
    pub candidates_evaluated: u32,
    pub zero_marginal_candidates: u32,
    pub geometry_uncertain_candidates: u32,
    pub zero_marginal_suppression: bool,
    pub shot: Option<ShotPrediction>,
}

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

fn possible_payload_positions(state: &GameState, conf: &GameConfig) -> [Vec2; 3] {
    let path_length = payload_path_len();
    let capture_step = if path_length > EPSILON {
        conf.payload.speed / path_length
    } else {
        0.0
    };
    [
        state.payload_pos(),
        payload_position((state.capture + capture_step).clamp(-1.0, 1.0)),
        payload_position((state.capture - capture_step).clamp(-1.0, 1.0)),
    ]
}

fn collision_free_position_is_confident(
    state: &GameState,
    conf: &GameConfig,
    origin: Vec2,
    payload_positions: &[Vec2; 3],
) -> bool {
    let radius = conf.bot.radius;
    let width = MAP_SIZE as f32;
    if origin.x < radius
        || origin.x > width - radius
        || origin.y < radius
        || origin.y > width - radius
        || !point_free(conf, origin)
    {
        return false;
    }

    let separated = |centre: Vec2, obstacle_radius: f32| {
        origin.dist(&centre) + EPSILON >= radius + obstacle_radius
    };
    if payload_positions
        .iter()
        .any(|&payload| !separated(payload, conf.payload.radius))
        || !separated(state.deposit_me.pos, conf.deposit.radius)
        || !separated(state.deposit_other.pos, conf.deposit.radius)
    {
        return false;
    }

    // Bots deliberately overlap in the official engine and never participate in collision
    // projection. Only walls, the payload and deposits can invalidate this firing origin.
    true
}

fn uncertain_candidate_count(state: &GameState, low_budget: bool) -> u32 {
    1 + if low_budget {
        u32::from(state.fleet_other.iter().next().is_some())
    } else {
        state.fleet_other.iter().count() as u32
    }
}

fn same_shot_across_payload_motion(reference: &ShotResult, alternative: &ShotResult) -> bool {
    reference.first_hit == alternative.first_hit
        && reference.physical_mask == alternative.physical_mask
        && reference.damageable_mask == alternative.damageable_mask
        && reference.blast_point.dist(&alternative.blast_point) <= EPSILON
}

fn candidate_score(
    result: &ShotResult,
    marginal_mask: u32,
    state: &GameState,
    conf: &GameConfig,
) -> i32 {
    let mut score = marginal_mask.count_ones() as i32 * NEW_DAMAGE_WEIGHT;
    for enemy in state.fleet_other.iter() {
        if marginal_mask & (1u32 << enemy.id) == 0 {
            continue;
        }
        if enemy.health <= conf.bot.blaster_damage + EPSILON {
            score += LETHAL_DAMAGE_WEIGHT;
        }
        if enemy.class() == BotClass::Healer {
            score += HEALER_DAMAGE_WEIGHT;
        }
    }
    if matches!(result.first_hit, Some(FirstHit::Enemy(_))) {
        score += DIRECT_HIT_WEIGHT;
    }
    score
}

pub fn battle_action(
    bot: &BotState,
    target: Option<&BotState>,
    state: &GameState,
    conf: &GameConfig,
    controls_payload: bool,
    already_covered: u32,
    low_budget: bool,
) -> (BotAction, u32, BattlePlanTelemetry) {
    let mut telemetry = BattlePlanTelemetry::default();
    let payload = state.payload_pos();
    let home = state.deposit_me.pos;
    let predicted_target = target.map(|enemy| enemy.pos + enemy.vel);

    let destination = if controls_payload {
        payload_control_spot(conf, payload, home, bot.id)
    } else if let Some(enemy) = target {
        if bot.pos.dist(&enemy.pos) > conf.bot.blaster_range * ENGAGE_RANGE_FRACTION {
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

    let Some(target_enemy) = target else {
        return (action, 0, telemetry);
    };
    if bot.next_fire_tick() > state.tick {
        return (action, 0, telemetry);
    }

    // Engine order is move, turn, payload, collision, then fire.  This predicts the
    // ordinary unobstructed move exactly.  If that position appears to require collision
    // projection, retain the legal move/turn action but conservatively skip the shot.
    let origin = bot.pos + actual_move_delta(move_dir, conf.bot.speed);
    let payload_positions = possible_payload_positions(state, conf);
    if !collision_free_position_is_confident(state, conf, origin, &payload_positions) {
        telemetry.geometry_uncertain_candidates = uncertain_candidate_count(state, low_budget);
        return (action, 0, telemetry);
    }
    if state.fleet_other.iter().any(|enemy| {
        !collision_free_position_is_confident(
            state,
            conf,
            enemy.pos + enemy.vel,
            &payload_positions,
        )
    }) {
        // The constant-velocity prediction is undefined through a wall, payload or deposit
        // collision. Preserve movement/turning and wait for a later callback rather than
        // presenting a behaviour-model error as exact ballistics.
        telemetry.geometry_uncertain_candidates = uncertain_candidate_count(state, low_budget);
        return (action, 0, telemetry);
    }

    // Constant velocity is explicitly a behaviour model.  The ballistics query itself
    // receives a fully defined firing-time geometry and exactly mirrors engine scan rules.
    let enemies: Vec<EnemyGeometry> = state
        .fleet_other
        .iter()
        .map(|candidate| EnemyGeometry {
            id: candidate.id,
            centre: candidate.pos + candidate.vel,
            damageable: candidate.invulnerable_until_tick <= state.tick,
        })
        .collect();
    let deposits = [
        Circle {
            centre: state.deposit_me.pos,
            radius: conf.deposit.radius,
        },
        Circle {
            centre: state.deposit_other.pos,
            radius: conf.deposit.radius,
        },
    ];

    // Candidate zero preserves the current barrel direction.  Remaining candidates aim
    // at each predicted enemy centre.  Turn limiting happens before every trace.
    let mut best: Option<(i32, f32, Option<Vec2>, ShotResult)> = None;
    let aim_targets = enemies
        .iter()
        .filter(|candidate| !low_budget || candidate.id == target_enemy.id)
        .map(|candidate| Some(candidate.centre));
    for aim_point in std::iter::once(None).chain(aim_targets) {
        telemetry.candidates_evaluated += 1;
        let desired = aim_point
            .map(|point| (point - origin).angle_deg())
            .unwrap_or(bot.angle);
        let facing = next_facing(bot.angle, desired, conf.bot.turn_speed);
        let direction = Vec2::from_angle_deg(facing);
        let trace_for_payload = |payload: Vec2| {
            trace_shot(
                conf,
                &ShotScene {
                    enemies: &enemies,
                    payload: Circle {
                        centre: payload,
                        radius: conf.payload.radius,
                    },
                    deposits,
                },
                origin,
                direction,
            )
        };
        let result = trace_for_payload(payload_positions[0]);
        if payload_positions[1..]
            .iter()
            .map(|&payload| trace_for_payload(payload))
            .any(|alternative| !same_shot_across_payload_motion(&result, &alternative))
        {
            telemetry.geometry_uncertain_candidates += 1;
            continue;
        }
        let marginal = marginal_damage_mask(&result, already_covered);
        if marginal == 0 {
            telemetry.zero_marginal_candidates += 1;
            continue;
        }
        let score = candidate_score(&result, marginal, state, conf);
        let turn = diff_degrees(facing, bot.angle).abs();
        let replace = match best.as_ref() {
            None => true,
            Some((best_score, best_turn, _, _)) => {
                score > *best_score || (score == *best_score && turn < *best_turn)
            }
        };
        if replace {
            best = Some((score, turn, aim_point, result));
        }
    }

    if let Some((_, _, aim_point, result)) = best {
        action.turn_action = aim_point
            .map(turn_towards)
            .unwrap_or_else(|| turn_to_angle(bot.angle));
        action.special_action = SpecialAction::Battle { fire: true };
        telemetry.shot = Some(ShotPrediction {
            blast_point: result.blast_point,
            predicted_new_damage_mask: marginal_damage_mask(&result, already_covered),
        });
        (action, result.damageable_mask, telemetry)
    } else {
        // Keep the original target-facing movement action even when every shot would add
        // zero current damage; unlike v0, a miss no longer burns the cooldown.
        telemetry.zero_marginal_suppression = telemetry.candidates_evaluated > 0
            && telemetry.zero_marginal_candidates == telemetry.candidates_evaluated;
        (action, 0, telemetry)
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

    fn config() -> GameConfig {
        serde_json::from_str(include_str!("../../../lab/fixtures/default_config.json"))
            .expect("pinned fixture must match client GameConfig")
    }

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

    #[test]
    fn payload_timing_envelope_contains_hold_and_both_one_tick_directions() {
        let conf = config();
        let mut state = GameState::new(&conf);
        state.capture = 0.2;
        let positions = possible_payload_positions(&state, &conf);
        let step = conf.payload.speed / payload_path_len();
        assert_eq!(positions[0], payload_position(state.capture));
        assert_eq!(positions[1], payload_position(state.capture + step));
        assert_eq!(positions[2], payload_position(state.capture - step));
    }

    #[test]
    fn low_budget_limits_aim_candidates_but_keeps_full_physical_scene() {
        let conf = config();
        init_topology(&conf.map, conf.bot.radius);
        let mut state = GameState::new(&conf);

        let shooter_id = state.fleet_me.add();
        let shooter = &mut state.fleet_me[shooter_id];
        shooter.id = shooter_id;
        shooter.health = conf.bot.health;
        shooter.pos = Vec2::new(10.0, 10.0);
        shooter.special = SpecialState::new(BotClass::Battle);

        for position in [Vec2::new(12.0, 10.0), Vec2::new(10.0, 12.0)] {
            let id = state.fleet_other.add();
            let enemy = &mut state.fleet_other[id];
            enemy.id = id;
            enemy.health = conf.bot.health;
            enemy.pos = position;
            enemy.special = SpecialState::new(BotClass::Battle);
        }

        let shooter = &state.fleet_me[shooter_id];
        let target = state.fleet_other.get(0);
        let (_, _, normal) = battle_action(shooter, target, &state, &conf, false, 0, false);
        let (_, _, degraded) = battle_action(shooter, target, &state, &conf, false, 0, true);

        assert_eq!(normal.candidates_evaluated, 3);
        assert_eq!(degraded.candidates_evaluated, 2);
    }
}
