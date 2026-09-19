use crate::core::*;

/// Independent challenge opponent: a distributed battle screen with persistent healer
/// support. It intentionally does not import the candidate strategy modules.
pub fn get_strategy(team: u8) -> Strategy {
    println!("MM32_OPPONENT {{\"strategy\":\"support_screen_v1\",\"team_slot\":{team}}}");
    Box::new(support_screen)
}

fn support_screen(state: &GameState) -> FleetAction {
    let conf = get_config();
    support_screen_with_config(state, conf)
}

fn support_screen_with_config(state: &GameState, conf: &GameConfig) -> FleetAction {
    let mut action = FleetAction::new();
    action.fabricator_next = next_class(state);
    let natural_due = state.fabricator_me.next_bot_creation <= state.tick;
    action.rush_order = !state.in_endgame(conf)
        && !state.fleet_me.is_full()
        && !natural_due
        && state.fabricator_me.tokens >= conf.fabricator.rush_cost;

    let payload = state.payload_pos();
    let mut heal_load = [0u8; BOTS_MAX];
    for bot in state.fleet_me.iter() {
        let order = &mut action.bots[bot.id as usize];
        match bot.class() {
            BotClass::Battle => battle_order(bot, order, state, conf, payload),
            BotClass::Healer => healer_order(bot, order, state, conf, payload, &mut heal_load),
            BotClass::Extractor => extractor_order(bot, order, state, conf),
        }
    }
    action
}

fn next_class(state: &GameState) -> BotClass {
    let (mut extractors, mut battles, mut healers) = (0usize, 0usize, 0usize);
    for bot in state.fleet_me.iter() {
        match bot.class() {
            BotClass::Extractor => extractors += 1,
            BotClass::Battle => battles += 1,
            BotClass::Healer => healers += 1,
        }
    }
    if extractors < 2 {
        BotClass::Extractor
    } else if healers * 2 < battles.max(2) {
        BotClass::Healer
    } else {
        BotClass::Battle
    }
}

fn screen_spot(state: &GameState, conf: &GameConfig, id: BotId) -> Vec2 {
    let payload = state.payload_pos();
    let home = state.deposit_me.pos;
    let base = (home - payload).normalize_or_else(|| Vec2::new(-1.0, 0.0));
    let radius = (conf.payload.radius + conf.bot.radius + 0.20)
        .min(conf.payload.capture_radius - conf.bot.radius - 0.05);
    for shift in 0..12 {
        let angle = -82.5 + ((id as usize + shift) % 12) as f32 * 15.0;
        let candidate = payload + base.rotate_deg(angle) * radius;
        if point_free(conf, candidate) {
            return candidate;
        }
    }
    payload + base * radius
}

fn battle_order(
    bot: &BotState,
    order: &mut BotAction,
    state: &GameState,
    conf: &GameConfig,
    payload: Vec2,
) {
    let station = screen_spot(state, conf, bot.id);
    let target = nearest_enemy(state, bot.pos);
    let destination = target
        .filter(|enemy| bot.pos.dist(&enemy.pos) > conf.bot.blaster_range * 0.78)
        .map(|enemy| enemy.pos + enemy.vel)
        .unwrap_or(station);
    let move_direction = navigate_to(conf, bot.pos, destination);
    order.move_action = move_bot(move_direction);
    let aim = target.map(|enemy| enemy.pos + enemy.vel).unwrap_or(payload);
    order.turn_action = turn_towards(aim);
    order.special_action = SpecialAction::Battle {
        fire: target.is_some_and(|enemy| {
            let origin = bot.pos + actual_move_delta(move_direction, conf.bot.speed);
            let heading = post_turn_angle(bot.angle, origin, aim, conf.bot.turn_speed);
            bot.next_fire_tick() <= state.tick
                && enemy.invulnerable_until_tick <= state.tick
                && has_line_of_sight(conf, origin, aim)
                && simple_shot_covers(conf, origin, heading, aim)
        }),
    };
}

fn healer_order(
    bot: &BotState,
    order: &mut BotAction,
    state: &GameState,
    conf: &GameConfig,
    payload: Vec2,
    heal_load: &mut [u8; BOTS_MAX],
) {
    let cap = conf.bot.heal_stack_cap.max(1.0).floor() as u8;
    let anchor = nearest_battle(state, bot.pos);
    let follow = anchor.map(|ally| ally.pos).unwrap_or(payload);
    let offset =
        Vec2::from_angle_deg((bot.id as f32 * 137.5) % 360.0) * (conf.bot.base_heal_range * 0.52);
    let desired = follow + offset;
    let destination = if point_free(conf, desired) {
        desired
    } else {
        follow
    };
    let move_direction = navigate_to(conf, bot.pos, destination);
    let predicted_pos = bot.pos + actual_move_delta(move_direction, conf.bot.speed);
    order.move_action = move_bot(move_direction);

    // Allocate only channels that can be in range after this tick's move. A healer that
    // still needs to turn keeps aiming at the injured ally but does not consume stack
    // capacity until the post-turn heading is inside the official half arc.
    let immediate = state
        .fleet_me
        .iter()
        .filter(|ally| {
            ally.id != bot.id
                && ally.health + 1e-5 < conf.bot.health
                && heal_load[ally.id as usize] < cap
                && predicted_pos.dist_sq(&ally.pos) <= conf.bot.base_heal_range.powi(2)
                && has_line_of_sight(conf, predicted_pos, ally.pos)
        })
        .min_by(|a, b| {
            a.health.total_cmp(&b.health).then_with(|| {
                predicted_pos
                    .dist_sq(&a.pos)
                    .total_cmp(&predicted_pos.dist_sq(&b.pos))
            })
        });

    if let Some(ally) = immediate {
        let heading = post_turn_angle(bot.angle, predicted_pos, ally.pos, conf.bot.turn_speed);
        let within_arc = diff_degrees((ally.pos - predicted_pos).angle_deg(), heading).abs()
            <= conf.bot.base_heal_arc_deg / 2.0;
        if within_arc {
            heal_load[ally.id as usize] += 1;
        }
        order.turn_action = turn_towards(ally.pos);
        order.special_action = SpecialAction::Healer {
            fire: within_arc,
            target: ally.id,
        };
    } else {
        order.turn_action = turn_towards(follow);
        order.special_action = SpecialAction::Healer {
            fire: false,
            target: bot.id,
        };
    }
}

fn extractor_order(bot: &BotState, order: &mut BotAction, state: &GameState, conf: &GameConfig) {
    let toward_centre =
        (Vec2::new(16.0, 16.0) - state.deposit_me.pos).normalize_or_else(|| Vec2::new(0.0, -1.0));
    let ring = conf.deposit.radius + conf.bot.radius + 0.06;
    let desired =
        state.deposit_me.pos + toward_centre.rotate_deg((bot.id as f32 - 1.0) * 28.0) * ring;
    order.move_action = move_bot(navigate_to(conf, bot.pos, desired));
    order.turn_action = turn_towards(state.deposit_me.pos);
    order.special_action = SpecialAction::Extractor { mine: true };
}

fn nearest_enemy(state: &GameState, from: Vec2) -> Option<&BotState> {
    state
        .fleet_other
        .iter()
        .min_by(|a, b| from.dist_sq(&a.pos).total_cmp(&from.dist_sq(&b.pos)))
}

fn nearest_battle(state: &GameState, from: Vec2) -> Option<&BotState> {
    state
        .fleet_me
        .iter()
        .filter(|ally| ally.class() == BotClass::Battle)
        .min_by(|a, b| from.dist_sq(&a.pos).total_cmp(&from.dist_sq(&b.pos)))
}

fn actual_move_delta(direction: Vec2, speed: f32) -> Vec2 {
    let sanitized = if direction.norm_sq() > 1.0 {
        direction.normalize_or_zero()
    } else {
        direction
    };
    sanitized * speed
}

fn post_turn_angle(current: f32, origin: Vec2, target: Vec2, turn_speed: f32) -> f32 {
    let desired = (target - origin).angle_deg();
    normalize_degrees(current + diff_degrees(desired, current).clamp(-turn_speed, turn_speed))
}

/// Conservative firing gate for this frozen opponent. It accepts a direct hull
/// intersection or endpoint splash, and deliberately leaves exact obstruction/first-hit
/// semantics to the candidate strategy under evaluation.
fn simple_shot_covers(conf: &GameConfig, origin: Vec2, heading: f32, target: Vec2) -> bool {
    let direction = Vec2::from_angle_deg(heading);
    let relative = target - origin;
    let along = relative.dot(direction).clamp(0.0, conf.bot.blaster_range);
    let closest = origin + direction * along;
    let direct = closest.dist_sq(&target) <= conf.bot.radius.powi(2);
    let endpoint = origin + direction * conf.bot.blaster_range;
    let splash = conf.bot.radius + conf.bot.base_blaster_splash_radius;
    direct || endpoint.dist_sq(&target) <= splash.powi(2)
}

#[cfg(test)]
pub(crate) fn action_for_test(state: &GameState, conf: &GameConfig) -> FleetAction {
    support_screen_with_config(state, conf)
}
