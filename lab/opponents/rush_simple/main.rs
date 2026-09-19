use crate::core::*;

/// A deliberately simple pressure opponent: it builds Battle bots, earns no mining income,
/// and sends every available hull to a legal ring around the payload. The mirrored client
/// view makes the same policy valid for both engine slots.
pub fn get_strategy(team: u8) -> Strategy {
    println!("MM32_OPPONENT {{\"strategy\":\"rush_simple\",\"team_slot\":{team}}}");
    Box::new(rush_strategy)
}

fn rush_strategy(state: &GameState) -> FleetAction {
    let conf = get_config();
    let mut action = FleetAction::new();

    action.fabricator_next = BotClass::Battle;

    // This bot has zero recurring economy: Extractors never mine. It spends the starting
    // bank on Battle bots, while preserving any natural build that is already due.
    let natural_build_due = state.fabricator_me.next_bot_creation <= state.tick;
    action.rush_order = !state.in_endgame(conf)
        && !state.fleet_me.is_full()
        && !natural_build_due
        && state.fabricator_me.tokens >= conf.fabricator.rush_cost;

    let payload = state.payload_pos();
    for bot in state.fleet_me.iter() {
        let station = payload_station(conf, payload, bot.id);
        let bot_action = &mut action.bots[bot.id as usize];

        match bot.class() {
            BotClass::Battle => configure_battle(bot, bot_action, state, conf, station),
            BotClass::Healer => configure_healer(bot, bot_action, state, conf, station),
            BotClass::Extractor => {
                bot_action.move_action = move_bot(navigate_to(conf, bot.pos, station));
                bot_action.turn_action = nearest_enemy(state, bot.pos)
                    .map(|enemy| turn_towards(enemy.pos))
                    .unwrap_or_else(|| turn_towards(payload));
                bot_action.special_action = SpecialAction::Extractor { mine: false };
            }
        }
    }

    action
}

fn payload_station(conf: &GameConfig, payload: Vec2, id: BotId) -> Vec2 {
    let radius = conf.payload.radius + conf.bot.radius + 0.05;
    let slot = id as usize % 8;

    // Try the bot's stable slot first, then rotate around the ring if that point is inside a
    // wall. The ring is outside the solid payload while remaining inside the shipped
    // capture radius.
    for offset in 0..8usize {
        let angle = (slot + offset) as f32 * std::f32::consts::TAU / 8.0;
        let candidate = payload + Vec2::new(angle.cos(), angle.sin()) * radius;
        if point_free(conf, candidate) {
            return candidate;
        }
    }

    // Collision handling will stop the bot at the payload hull if every ring point is
    // blocked in a non-standard map.
    payload
}

fn configure_battle(
    bot: &BotState,
    bot_action: &mut BotAction,
    state: &GameState,
    conf: &GameConfig,
    station: Vec2,
) {
    bot_action.move_action = move_bot(navigate_to(conf, bot.pos, station));

    let Some(enemy) = nearest_enemy(state, bot.pos) else {
        bot_action.turn_action = turn_towards(station);
        bot_action.special_action = SpecialAction::Battle { fire: false };
        return;
    };

    bot_action.turn_action = turn_towards(enemy.pos);
    let can_fire = bot.next_fire_tick() <= state.tick
        && enemy.invulnerable_until_tick <= state.tick
        && bot.pos.dist(&enemy.pos) <= conf.bot.blaster_range
        && has_line_of_sight(conf, bot.pos, enemy.pos);
    bot_action.special_action = SpecialAction::Battle { fire: can_fire };
}

fn configure_healer(
    bot: &BotState,
    bot_action: &mut BotAction,
    state: &GameState,
    conf: &GameConfig,
    station: Vec2,
) {
    if let Some((target, target_pos)) = most_injured_ally(state, conf, bot.id, bot.pos) {
        bot_action.move_action = move_bot(navigate_to(conf, bot.pos, target_pos));
        bot_action.turn_action = turn_towards(target_pos);
        let can_heal = bot.pos.dist(&target_pos) <= conf.bot.base_heal_range
            && has_line_of_sight(conf, bot.pos, target_pos);
        bot_action.special_action = SpecialAction::Healer {
            fire: can_heal,
            target,
        };
        return;
    }

    bot_action.move_action = move_bot(navigate_to(conf, bot.pos, station));
    bot_action.turn_action = turn_towards(station);
    bot_action.special_action = SpecialAction::Healer {
        fire: false,
        target: bot.id,
    };
}

fn nearest_enemy<'a>(state: &'a GameState, from: Vec2) -> Option<&'a BotState> {
    state
        .fleet_other
        .iter()
        .min_by(|a, b| from.dist_sq(&a.pos).total_cmp(&from.dist_sq(&b.pos)))
}

fn most_injured_ally(
    state: &GameState,
    conf: &GameConfig,
    healer_id: BotId,
    healer_pos: Vec2,
) -> Option<(BotId, Vec2)> {
    state
        .fleet_me
        .iter()
        .filter(|ally| ally.id != healer_id && ally.health < conf.bot.health - 0.0001)
        .min_by(|a, b| {
            a.health.total_cmp(&b.health).then_with(|| {
                healer_pos
                    .dist_sq(&a.pos)
                    .total_cmp(&healer_pos.dist_sq(&b.pos))
            })
        })
        .map(|ally| (ally.id, ally.pos))
}
