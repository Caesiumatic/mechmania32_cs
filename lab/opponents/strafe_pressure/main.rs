use crate::core::*;

/// Independent pressure opponent that circles the nearest enemy while preserving firing
/// distance. It is intentionally simple and frozen for M02 evaluation.
pub fn get_strategy(team: u8) -> Strategy {
    println!("MM32_OPPONENT {{\"strategy\":\"strafe_pressure_v1\",\"team_slot\":{team}}}");
    Box::new(strafe_pressure)
}

fn strafe_pressure(state: &GameState) -> FleetAction {
    let conf = get_config();
    strafe_pressure_with_config(state, conf)
}

fn strafe_pressure_with_config(state: &GameState, conf: &GameConfig) -> FleetAction {
    let mut action = FleetAction::new();
    action.fabricator_next = next_class(state);
    let natural_due = state.fabricator_me.next_bot_creation <= state.tick;
    action.rush_order = !state.in_endgame(conf)
        && !state.fleet_me.is_full()
        && !natural_due
        && state.fabricator_me.tokens >= conf.fabricator.rush_cost;

    for bot in state.fleet_me.iter() {
        let order = &mut action.bots[bot.id as usize];
        match bot.class() {
            BotClass::Battle => battle_order(bot, order, state, conf),
            BotClass::Healer => healer_order(bot, order, state, conf),
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
    if extractors < 1 {
        BotClass::Extractor
    } else if battles >= 5 && healers * 4 < battles {
        BotClass::Healer
    } else {
        BotClass::Battle
    }
}

fn battle_order(bot: &BotState, order: &mut BotAction, state: &GameState, conf: &GameConfig) {
    let payload = state.payload_pos();
    let Some(enemy) = nearest_enemy(state, bot.pos) else {
        order.move_action = move_bot(navigate_to(conf, bot.pos, payload));
        order.turn_action = turn_towards(payload);
        order.special_action = SpecialAction::Battle { fire: false };
        return;
    };

    let predicted = enemy.pos + enemy.vel;
    let radial = (predicted - bot.pos).normalize_or_else(|| Vec2::new(1.0, 0.0));
    let tangent = if bot.id % 2 == 0 {
        radial.rotate_deg(90.0)
    } else {
        radial.rotate_deg(-90.0)
    };
    let distance = bot.pos.dist(&predicted);
    let preferred = conf.bot.blaster_range * 0.72;
    let radial_correction = ((distance - preferred) / preferred).clamp(-0.8, 0.8);
    let strafe_direction = (tangent + radial * radial_correction).normalize_or_zero();
    let one_step_target = bot.pos + strafe_direction * conf.bot.speed;
    let move_direction = if point_free(conf, one_step_target) {
        strafe_direction
    } else {
        navigate_to(conf, bot.pos, payload)
    };

    order.move_action = move_bot(move_direction);
    order.turn_action = turn_towards(predicted);
    order.special_action = SpecialAction::Battle {
        fire: {
            let origin = bot.pos + actual_move_delta(move_direction, conf.bot.speed);
            let heading = post_turn_angle(bot.angle, origin, predicted, conf.bot.turn_speed);
            bot.next_fire_tick() <= state.tick
                && enemy.invulnerable_until_tick <= state.tick
                && has_line_of_sight(conf, origin, predicted)
                && simple_shot_covers(conf, origin, heading, predicted)
        },
    };
}

fn healer_order(bot: &BotState, order: &mut BotAction, state: &GameState, conf: &GameConfig) {
    let anchor = nearest_battle(state, bot.pos);
    let target = state
        .fleet_me
        .iter()
        .filter(|ally| {
            ally.id != bot.id
                && ally.health + 1e-5 < conf.bot.health
                && bot.pos.dist_sq(&ally.pos) <= conf.bot.base_heal_range.powi(2)
        })
        .min_by(|a, b| a.health.total_cmp(&b.health));
    let follow = anchor
        .map(|ally| ally.pos)
        .unwrap_or_else(|| state.payload_pos());
    let move_direction = navigate_to(conf, bot.pos, follow);
    let predicted_pos = bot.pos + actual_move_delta(move_direction, conf.bot.speed);
    order.move_action = move_bot(move_direction);
    if let Some(ally) = target {
        let heading = post_turn_angle(bot.angle, predicted_pos, ally.pos, conf.bot.turn_speed);
        let within_arc = diff_degrees((ally.pos - predicted_pos).angle_deg(), heading).abs()
            <= conf.bot.base_heal_arc_deg / 2.0;
        order.turn_action = turn_towards(ally.pos);
        order.special_action = SpecialAction::Healer {
            fire: predicted_pos.dist_sq(&ally.pos) <= conf.bot.base_heal_range.powi(2)
                && within_arc
                && has_line_of_sight(conf, predicted_pos, ally.pos),
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
    let direction =
        (Vec2::new(16.0, 16.0) - state.deposit_me.pos).normalize_or_else(|| Vec2::new(0.0, -1.0));
    let target = state.deposit_me.pos + direction * (conf.deposit.radius + conf.bot.radius + 0.06);
    order.move_action = move_bot(navigate_to(conf, bot.pos, target));
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
    strafe_pressure_with_config(state, conf)
}
