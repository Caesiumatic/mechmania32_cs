use crate::core::*;

use super::geometry::next_facing;
use super::policy::{
    BATTLE_PRIORITY_BONUS, ENTITY_CLEARANCE, HEAL_SWITCH_PENALTY, POSITION_EPSILON,
    SUPPORT_MIN_BOT_RADII, SUPPORT_RANGE_FRACTION, SUPPORT_SEARCH_STEPS,
    SUPPORT_SEARCH_STEP_DEGREES, SUPPORT_SWITCH_PENALTY,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealPlanStats {
    pub effective_channels: u32,
    pub support_travelers: u32,
}

/// Mirror `MoveAction::sanitize` and the engine's per-tick movement scaling.
pub fn actual_move_delta(direction: Vec2, speed: f32) -> Vec2 {
    let sanitized = if direction.norm_sq() > 1.0 {
        direction.normalize_or_zero()
    } else {
        direction
    };
    sanitized * speed
}

fn entity_clear(state: &GameState, conf: &GameConfig, point: Vec2) -> bool {
    let payload_clearance = conf.payload.radius + conf.bot.radius + ENTITY_CLEARANCE;
    let deposit_clearance = conf.deposit.radius + conf.bot.radius + ENTITY_CLEARANCE;
    point.dist_sq(&state.payload_pos()) >= payload_clearance * payload_clearance
        && point.dist_sq(&state.deposit_me.pos) >= deposit_clearance * deposit_clearance
        && point.dist_sq(&state.deposit_other.pos) >= deposit_clearance * deposit_clearance
}

#[cfg(test)]
fn conservative_position(
    state: &GameState,
    conf: &GameConfig,
    bot: &BotState,
    action: &BotAction,
) -> Vec2 {
    predicted_position(state, conf, bot, action).0
}

fn predicted_position(
    state: &GameState,
    conf: &GameConfig,
    bot: &BotState,
    action: &BotAction,
) -> (Vec2, bool) {
    let proposed = bot.pos + actual_move_delta(action.move_action.direction, conf.bot.speed);
    if point_free(conf, proposed) && entity_clear(state, conf, proposed) {
        (proposed, true)
    } else {
        // The exact engine collision projection is deliberately not approximated through a
        // solid entity. The old position is useful for support continuity, while `false`
        // prevents this uncertain geometry from consuming an effective heal channel.
        (bot.pos, false)
    }
}

fn movement(conf: &GameConfig, from: Vec2, to: Vec2) -> Vec2 {
    if from.dist_sq(&to) <= (conf.bot.speed * 0.5).powi(2) {
        Vec2::ZERO
    } else {
        navigate_to(conf, from, to)
    }
}

fn support_position(state: &GameState, conf: &GameConfig, healer: &BotState, anchor: Vec2) -> Vec2 {
    let ring = (conf.bot.base_heal_range * SUPPORT_RANGE_FRACTION)
        .max(conf.bot.radius * SUPPORT_MIN_BOT_RADII);
    let base_angle = (healer.id as f32 * 137.507_77) % 360.0;

    for step in 0..SUPPORT_SEARCH_STEPS {
        let offset = if step == 0 {
            0.0
        } else {
            let magnitude = ((step + 1) / 2) as f32 * SUPPORT_SEARCH_STEP_DEGREES;
            if step % 2 == 0 {
                -magnitude
            } else {
                magnitude
            }
        };
        let candidate = anchor + Vec2::from_angle_deg(base_angle + offset) * ring;
        if point_free(conf, candidate)
            && entity_clear(state, conf, candidate)
            && path_length(conf, healer.pos, candidate).is_some()
        {
            return candidate;
        }
    }

    // A zero move is always safer than deliberately steering through the anchor/entity.
    // The engine will resolve an already-overlapping input state itself.
    healer.pos
}

fn select_support_anchor<'a>(
    healer: &BotState,
    state: &'a GameState,
    cached: Option<BotId>,
) -> Option<&'a BotState> {
    let payload = state.payload_pos();
    let battle = state
        .fleet_me
        .iter()
        .filter(|ally| ally.id != healer.id && ally.class() == BotClass::Battle)
        .min_by(|a, b| {
            let score = |ally: &BotState| {
                let switch = if cached == Some(ally.id) {
                    0.0
                } else {
                    SUPPORT_SWITCH_PENALTY
                };
                ally.pos.dist(&payload) / 10.0 + switch
            };
            score(a).total_cmp(&score(b)).then_with(|| a.id.cmp(&b.id))
        });
    battle.or_else(|| {
        state
            .fleet_me
            .iter()
            .filter(|ally| ally.id != healer.id && ally.class() != BotClass::Healer)
            .min_by(|a, b| {
                let score = |ally: &BotState| {
                    let switch = if cached == Some(ally.id) {
                        0.0
                    } else {
                        SUPPORT_SWITCH_PENALTY
                    };
                    healer.pos.dist(&ally.pos) / 10.0 + switch
                };
                score(a).total_cmp(&score(b)).then_with(|| a.id.cmp(&b.id))
            })
    })
}

fn heal_capacity(ally: &BotState, conf: &GameConfig) -> u8 {
    let missing = (conf.bot.health - ally.health).max(0.0);
    if missing <= POSITION_EPSILON || conf.bot.heal_per_tick <= 0.0 {
        return 0;
    }
    // Subtract the comparison epsilon before ceil so an exactly-two-channel deficit does
    // not become three merely because `health - 0.10` rounded a few ulps downward.
    let missing_channels =
        ((missing - POSITION_EPSILON).max(0.0) / conf.bot.heal_per_tick).ceil() as u8;
    let stack_cap = conf.bot.heal_stack_cap.max(0.0).floor() as u8;
    missing_channels.min(stack_cap)
}

fn effective_heal_geometry(
    healer: &BotState,
    healer_pos: Vec2,
    ally_pos: Vec2,
    conf: &GameConfig,
) -> bool {
    let to_ally = ally_pos - healer_pos;
    if to_ally.norm_sq() > conf.bot.base_heal_range.powi(2) {
        return false;
    }
    let desired = to_ally.angle_deg();
    let facing = next_facing(healer.angle, desired, conf.bot.turn_speed);
    if diff_degrees(desired, facing).abs() > conf.bot.base_heal_arc_deg * 0.5 {
        return false;
    }
    has_line_of_sight(conf, healer_pos, ally_pos)
}

/// Plan all healer movement first, then allocate only channels that can land this tick.
/// Idle healers retain a stable support anchor and continue travelling with the front line.
pub fn plan_healer_actions(
    state: &GameState,
    conf: &GameConfig,
    fleet_action: &mut FleetAction,
    cached_heal_targets: &mut [Option<BotId>; BOTS_MAX],
    cached_support_targets: &mut [Option<BotId>; BOTS_MAX],
) -> HealPlanStats {
    let mut stats = HealPlanStats::default();

    // Predict the movement already proposed by the non-healers. A healer that can deliver a
    // channel against this coherent snapshot holds position instead of sacrificing an exact
    // range-edge opportunity merely to chase its longer-term support anchor.
    let mut pre_healer_positions = [None; BOTS_MAX];
    let mut pre_healer_confident = [false; BOTS_MAX];
    for bot in state.fleet_me.iter() {
        let (position, confident) = if bot.class() == BotClass::Healer {
            (bot.pos, true)
        } else {
            predicted_position(state, conf, bot, &fleet_action.bots[bot.id as usize])
        };
        pre_healer_positions[bot.id as usize] = Some(position);
        pre_healer_confident[bot.id as usize] = confident;
    }

    let mut reserved_current = [0u8; BOTS_MAX];

    // Phase 1: reserve currently deliverable capacity first. Only an idle healer receives a
    // travelling support task, which makes the movement decision match the allocation order.
    for healer in state
        .fleet_me
        .iter()
        .filter(|bot| bot.class() == BotClass::Healer)
    {
        let cached_heal = cached_heal_targets[healer.id as usize];
        let current_target = state
            .fleet_me
            .iter()
            .filter(|ally| ally.id != healer.id)
            .filter(|ally| reserved_current[ally.id as usize] < heal_capacity(ally, conf))
            .filter(|ally| pre_healer_confident[ally.id as usize])
            .filter(|ally| {
                let ally_pos = pre_healer_positions[ally.id as usize].unwrap_or(ally.pos);
                effective_heal_geometry(healer, healer.pos, ally_pos, conf)
            })
            .min_by(|a, b| {
                let score = |ally: &BotState| {
                    let ally_pos = pre_healer_positions[ally.id as usize].unwrap_or(ally.pos);
                    let health_fraction = ally.health / conf.bot.health.max(POSITION_EPSILON);
                    let distance = healer.pos.dist(&ally_pos) / conf.bot.base_heal_range.max(1.0);
                    let switch = if cached_heal == Some(ally.id) {
                        0.0
                    } else {
                        HEAL_SWITCH_PENALTY
                    };
                    let class_bonus = if ally.class() == BotClass::Battle {
                        -BATTLE_PRIORITY_BONUS
                    } else {
                        0.0
                    };
                    health_fraction + 0.05 * distance + switch + class_bonus
                };
                score(a).total_cmp(&score(b)).then_with(|| a.id.cmp(&b.id))
            });

        let (destination, look_at, support_target) = if let Some(ally) = current_target {
            reserved_current[ally.id as usize] += 1;
            (
                healer.pos,
                pre_healer_positions[ally.id as usize].unwrap_or(ally.pos),
                Some(ally.id),
            )
        } else {
            let anchor =
                select_support_anchor(healer, state, cached_support_targets[healer.id as usize]);
            if let Some(anchor) = anchor {
                let anchor_pos = pre_healer_positions[anchor.id as usize].unwrap_or(anchor.pos);
                (
                    support_position(state, conf, healer, anchor_pos),
                    anchor_pos,
                    Some(anchor.id),
                )
            } else {
                (healer.pos, state.payload_pos(), None)
            }
        };
        let move_direction = movement(conf, healer.pos, destination);
        if move_direction.norm_sq() > 0.0 {
            stats.support_travelers += 1;
        }
        fleet_action.bots[healer.id as usize] = BotAction {
            move_action: move_bot(move_direction),
            turn_action: turn_towards(look_at),
            special_action: SpecialAction::Healer {
                fire: false,
                target: healer.id,
            },
            self_destruct: false,
        };
        cached_support_targets[healer.id as usize] = support_target;
    }

    // All classes now have a proposed movement, so every heal query observes one coherent
    // team snapshot instead of depending on BotId iteration order.
    let mut predicted_positions = [None; BOTS_MAX];
    let mut prediction_confident = [false; BOTS_MAX];
    for bot in state.fleet_me.iter() {
        let (position, confident) =
            predicted_position(state, conf, bot, &fleet_action.bots[bot.id as usize]);
        predicted_positions[bot.id as usize] = Some(position);
        prediction_confident[bot.id as usize] = confident;
    }

    // Phase 2: greedily allocate immediately effective capacity. A far, low-health target
    // cannot consume a slot that a nearby healable target can use this tick.
    let mut assigned = [0u8; BOTS_MAX];
    for healer in state
        .fleet_me
        .iter()
        .filter(|bot| bot.class() == BotClass::Healer)
    {
        let healer_pos = predicted_positions[healer.id as usize].unwrap_or(healer.pos);
        let cached = cached_heal_targets[healer.id as usize];
        let target = state
            .fleet_me
            .iter()
            .filter(|ally| ally.id != healer.id)
            .filter(|ally| assigned[ally.id as usize] < heal_capacity(ally, conf))
            .filter(|ally| {
                prediction_confident[healer.id as usize] && prediction_confident[ally.id as usize]
            })
            .filter(|ally| {
                let ally_pos = predicted_positions[ally.id as usize].unwrap_or(ally.pos);
                effective_heal_geometry(healer, healer_pos, ally_pos, conf)
            })
            .min_by(|a, b| {
                let score = |ally: &BotState| {
                    let ally_pos = predicted_positions[ally.id as usize].unwrap_or(ally.pos);
                    let health_fraction = ally.health / conf.bot.health.max(POSITION_EPSILON);
                    let distance = healer_pos.dist(&ally_pos) / conf.bot.base_heal_range.max(1.0);
                    let switch = if cached == Some(ally.id) {
                        0.0
                    } else {
                        HEAL_SWITCH_PENALTY
                    };
                    let class_bonus = if ally.class() == BotClass::Battle {
                        -BATTLE_PRIORITY_BONUS
                    } else {
                        0.0
                    };
                    health_fraction + 0.05 * distance + switch + class_bonus
                };
                score(a).total_cmp(&score(b)).then_with(|| a.id.cmp(&b.id))
            });

        if let Some(ally) = target {
            let ally_pos = predicted_positions[ally.id as usize].unwrap_or(ally.pos);
            let action = &mut fleet_action.bots[healer.id as usize];
            action.turn_action = turn_towards(ally_pos);
            action.special_action = SpecialAction::Healer {
                fire: true,
                target: ally.id,
            };
            assigned[ally.id as usize] += 1;
            cached_heal_targets[healer.id as usize] = Some(ally.id);
            stats.effective_channels += 1;
        } else {
            cached_heal_targets[healer.id as usize] = None;
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GameConfig {
        GameConfig {
            max_ticks: 9_000,
            endgame_ticks: 3_000,
            bot: BotConfig {
                radius: BOT_RADIUS,
                speed: 0.05,
                health: 10.0,
                turn_speed: 3.0,
                blaster_cooldown: 60,
                blaster_range: 10.0,
                blaster_damage: 3.0,
                heal_per_tick: 0.05,
                extract_rate: 0.05,
                base_invulnerability_ticks: 15,
                base_blaster_splash_radius: 0.3,
                base_heal_range: 3.0,
                base_heal_arc_deg: 90.0,
                heal_stack_cap: 3.0,
                base_extract_range: 5.0,
            },
            payload: PayloadConfig {
                radius: 0.75,
                capture_radius: 2.5,
                speed: 0.02,
            },
            payload_path: PAYLOAD_PATH,
            deposit: DepositConfig {
                pos: DEPOSIT_POS,
                radius: 0.5,
                extractor_cap: 8,
            },
            fabricator: FabricatorConfig {
                interval: 200,
                rush_cost: 50.0,
                starting_tokens: 800.0,
            },
            map: MAP,
        }
    }

    fn state() -> (GameConfig, GameState) {
        let conf = config();
        init_topology(&conf.map, conf.bot.radius);
        let state = GameState::new(&conf);
        (conf, state)
    }

    fn spawn(
        state: &mut GameState,
        conf: &GameConfig,
        class: BotClass,
        pos: Vec2,
        angle: f32,
        health: f32,
    ) -> BotId {
        let id = state.fleet_me.add();
        let bot = &mut state.fleet_me[id];
        bot.id = id;
        bot.health = health.min(conf.bot.health);
        bot.pos = pos;
        bot.vel = Vec2::ZERO;
        bot.angle = angle;
        bot.turn_vel = 0.0;
        bot.invulnerable_until_tick = 0;
        bot.special = SpecialState::new(class);
        id
    }

    fn fire_target(action: &BotAction) -> Option<BotId> {
        match action.special_action {
            SpecialAction::Healer { fire: true, target } => Some(target),
            _ => None,
        }
    }

    #[test]
    fn movement_prediction_preserves_short_vectors() {
        let delta = actual_move_delta(Vec2::new(0.4, 0.0), 0.05);
        assert!((delta.x - 0.02).abs() < 1e-6);
        assert!(delta.y.abs() < 1e-6);
    }

    #[test]
    fn effective_capacity_respects_missing_health_and_stack_cap() {
        let mut conf = config();
        let mut state = GameState::new(&conf);
        let id = state.fleet_me.add();
        let ally = &mut state.fleet_me[id];
        ally.health = conf.bot.health - conf.bot.heal_per_tick * 1.5;
        assert_eq!(heal_capacity(ally, &conf), 2);
        conf.bot.heal_stack_cap = 1.0;
        assert_eq!(heal_capacity(ally, &conf), 1);
    }

    #[test]
    fn d06_full_health_healer_keeps_travelling_with_battle_front() {
        let (conf, mut state) = state();
        let healer = spawn(
            &mut state,
            &conf,
            BotClass::Healer,
            Vec2::new(8.0, 10.0),
            0.0,
            conf.bot.health,
        );
        let battle = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(16.0, 16.0),
            0.0,
            conf.bot.health,
        );
        let mut action = FleetAction::new();
        let mut heals = [None; BOTS_MAX];
        let mut supports = [None; BOTS_MAX];
        let stats = plan_healer_actions(&state, &conf, &mut action, &mut heals, &mut supports);

        assert!(action.bots[healer as usize].move_action.direction.norm_sq() > 0.0);
        assert_eq!(fire_target(&action.bots[healer as usize]), None);
        assert_eq!(supports[healer as usize], Some(battle));
        assert_eq!(stats.effective_channels, 0);
        assert_eq!(stats.support_travelers, 1);
    }

    #[test]
    fn d07_near_effective_heal_beats_far_lower_health() {
        let (conf, mut state) = state();
        let healer = spawn(
            &mut state,
            &conf,
            BotClass::Healer,
            Vec2::new(10.0, 10.0),
            0.0,
            conf.bot.health,
        );
        let near = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(12.0, 10.0),
            0.0,
            9.0,
        );
        let _far = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(20.0, 10.0),
            0.0,
            1.0,
        );
        let mut action = FleetAction::new();
        let mut heals = [None; BOTS_MAX];
        let mut supports = [None; BOTS_MAX];
        plan_healer_actions(&state, &conf, &mut action, &mut heals, &mut supports);

        assert_eq!(fire_target(&action.bots[healer as usize]), Some(near));
    }

    #[test]
    fn d07_range_edge_channel_is_reserved_before_support_travel() {
        let (conf, mut state) = state();
        let healer = spawn(
            &mut state,
            &conf,
            BotClass::Healer,
            Vec2::new(10.0, 10.0),
            270.0,
            conf.bot.health,
        );
        let wounded = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(10.0, 7.0),
            0.0,
            conf.bot.health - 1.0,
        );
        // This healthy unit is the preferred long-term payload anchor and lies in the
        // opposite direction. Travelling toward it would move the exact-range wound out of
        // reach, so the effective channel must be reserved first.
        let payload = state.payload_pos();
        let _front = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            payload,
            0.0,
            conf.bot.health,
        );
        let mut action = FleetAction::new();
        let mut heals = [None; BOTS_MAX];
        let mut supports = [None; BOTS_MAX];
        let stats = plan_healer_actions(&state, &conf, &mut action, &mut heals, &mut supports);

        assert_eq!(stats.effective_channels, 1);
        assert_eq!(fire_target(&action.bots[healer as usize]), Some(wounded));
        assert_eq!(
            action.bots[healer as usize].move_action.direction,
            Vec2::ZERO
        );
    }

    #[test]
    fn d08_predicted_movement_and_turn_limit_gate_effective_heals() {
        let conf = config();
        let mut state = GameState::new(&conf);
        let healer = spawn(
            &mut state,
            &conf,
            BotClass::Healer,
            Vec2::new(10.0, 10.0),
            0.0,
            conf.bot.health,
        );
        let ally = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(13.02, 10.0),
            0.0,
            9.0,
        );
        let entering = conservative_position(
            &state,
            &conf,
            &state.fleet_me[ally],
            &BotAction {
                move_action: move_bot(Vec2::new(-1.0, 0.0)),
                ..BotAction::default()
            },
        );
        assert!(effective_heal_geometry(
            &state.fleet_me[healer],
            state.fleet_me[healer].pos,
            entering,
            &conf,
        ));

        let leaving = conservative_position(
            &state,
            &conf,
            &state.fleet_me[ally],
            &BotAction {
                move_action: move_bot(Vec2::new(1.0, 0.0)),
                ..BotAction::default()
            },
        );
        assert!(!effective_heal_geometry(
            &state.fleet_me[healer],
            state.fleet_me[healer].pos,
            leaving,
            &conf,
        ));

        state.fleet_me[healer].angle = 180.0;
        assert!(!effective_heal_geometry(
            &state.fleet_me[healer],
            state.fleet_me[healer].pos,
            Vec2::new(12.0, 10.0),
            &conf,
        ));
    }

    #[test]
    fn d08_three_channel_cap_full_health_and_invalid_targets_are_safe() {
        let (conf, mut state) = state();
        let mut healers = Vec::new();
        for x in [9.0, 9.3, 9.6, 9.9] {
            healers.push(spawn(
                &mut state,
                &conf,
                BotClass::Healer,
                Vec2::new(x, 10.5),
                0.0,
                conf.bot.health,
            ));
        }
        let target = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(12.0, 10.5),
            0.0,
            9.0,
        );
        let mut action = FleetAction::new();
        let mut heals = [Some(BOTS_MAX as BotId - 1); BOTS_MAX];
        let mut supports = [None; BOTS_MAX];
        let stats = plan_healer_actions(&state, &conf, &mut action, &mut heals, &mut supports);
        assert_eq!(stats.effective_channels, 3);
        assert_eq!(
            healers
                .iter()
                .filter(|&&id| fire_target(&action.bots[id as usize]) == Some(target))
                .count(),
            3
        );
        assert!(healers
            .iter()
            .all(|&id| fire_target(&action.bots[id as usize]) != Some(id)));

        state.fleet_me[target].health = conf.bot.health;
        let stats = plan_healer_actions(&state, &conf, &mut action, &mut heals, &mut supports);
        assert_eq!(stats.effective_channels, 0);
        assert!(healers
            .iter()
            .all(|&id| fire_target(&action.bots[id as usize]).is_none()));
    }

    #[test]
    fn d10_support_positions_avoid_payload_and_deposits() {
        let (conf, mut state) = state();
        let healer = spawn(
            &mut state,
            &conf,
            BotClass::Healer,
            Vec2::new(16.0, 12.0),
            0.0,
            conf.bot.health,
        );
        for entity in [state.payload_pos(), state.deposit_me.pos] {
            let anchor = entity
                - Vec2::new(
                    conf.payload.radius + conf.bot.radius + ENTITY_CLEARANCE,
                    0.0,
                );
            let support = support_position(&state, &conf, &state.fleet_me[healer], anchor);
            assert!(point_free(&conf, support));
            assert!(entity_clear(&state, &conf, support));
            assert!(path_length(&conf, state.fleet_me[healer].pos, support).is_some());
        }
    }
}
