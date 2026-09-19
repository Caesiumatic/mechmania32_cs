use crate::core::*;

/// The official starter strategy with the side-dependent entry removed and with every bot
/// class given a matching action. The engine mirrors team B into the same local frame, so
/// both team slots deliberately use this exact function.
pub fn get_strategy(team: u8) -> Strategy {
    println!("MM32_OPPONENT {{\"strategy\":\"starter_symmetric\",\"team_slot\":{team}}}");
    Box::new(basic_strategy)
}

fn basic_strategy(state: &GameState) -> FleetAction {
    let conf = get_config();
    let mut action = FleetAction::new();

    action.fabricator_next = next_class(state);

    // Let a due natural build happen for free. On all other ticks, retain the starter's
    // simple behaviour of spending available tokens on another body.
    let natural_build_due = state.fabricator_me.next_bot_creation <= state.tick;
    action.rush_order = !state.in_endgame(conf)
        && !state.fleet_me.is_full()
        && !natural_build_due
        && state.fabricator_me.tokens >= conf.fabricator.rush_cost;

    let payload = state.payload_pos();
    let mining_spot = legal_mining_spot(state, conf);
    let mut assigned_contester = false;

    for bot in state.fleet_me.iter() {
        let bot_action = &mut action.bots[bot.id as usize];

        match bot.class() {
            BotClass::Extractor => {
                bot_action.move_action = move_bot(navigate_to(conf, bot.pos, mining_spot));
                bot_action.turn_action = turn_towards(state.deposit_me.pos);
                bot_action.special_action = SpecialAction::Extractor { mine: true };
            }
            BotClass::Healer => configure_healer(bot, bot_action, state, conf, payload),
            BotClass::Battle => {
                let move_target = if assigned_contester {
                    nearest_enemy(state, bot.pos)
                        .map(|enemy| enemy.pos)
                        .unwrap_or(payload)
                } else {
                    assigned_contester = true;
                    payload
                };
                configure_battle(bot, bot_action, state, conf, move_target);
            }
        }
    }

    action
}

/// Keep the starter's mostly-Battle composition, while ensuring its two support classes
/// can actually occur and can be replaced after a loss.
fn next_class(state: &GameState) -> BotClass {
    let mut extractors = 0usize;
    let mut healers = 0usize;
    let mut battles = 0usize;

    for bot in state.fleet_me.iter() {
        match bot.class() {
            BotClass::Extractor => extractors += 1,
            BotClass::Healer => healers += 1,
            BotClass::Battle => battles += 1,
        }
    }

    if extractors == 0 {
        BotClass::Extractor
    } else if healers == 0 && battles >= 2 {
        BotClass::Healer
    } else {
        BotClass::Battle
    }
}

fn legal_mining_spot(state: &GameState, conf: &GameConfig) -> Vec2 {
    let distance = conf.deposit.radius + conf.bot.radius + 0.05;
    let offsets = [
        Vec2::new(0.0, 1.0),
        Vec2::new(1.0, 0.0),
        Vec2::new(-1.0, 0.0),
        Vec2::new(0.0, -1.0),
    ];

    for offset in offsets {
        let candidate = state.deposit_me.pos + offset * distance;
        if point_free(conf, candidate) && has_line_of_sight(conf, candidate, state.deposit_me.pos) {
            return candidate;
        }
    }

    // The shipped map has a legal +y spot; retaining it as a deterministic fallback also
    // keeps this opponent close to the official starter strategy.
    state.deposit_me.pos + Vec2::new(0.0, distance)
}

fn configure_battle(
    bot: &BotState,
    bot_action: &mut BotAction,
    state: &GameState,
    conf: &GameConfig,
    move_target: Vec2,
) {
    bot_action.move_action = move_bot(navigate_to(conf, bot.pos, move_target));

    let Some(enemy) = nearest_enemy(state, bot.pos) else {
        bot_action.turn_action = turn_towards(move_target);
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
    fallback: Vec2,
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

    let follow = nearest_battle(state, bot.pos).unwrap_or(fallback);
    bot_action.move_action = move_bot(navigate_to(conf, bot.pos, follow));
    bot_action.turn_action = turn_towards(follow);
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

fn nearest_battle(state: &GameState, from: Vec2) -> Option<Vec2> {
    state
        .fleet_me
        .iter()
        .filter(|ally| ally.class() == BotClass::Battle)
        .min_by(|a, b| from.dist_sq(&a.pos).total_cmp(&from.dist_sq(&b.pos)))
        .map(|ally| ally.pos)
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
