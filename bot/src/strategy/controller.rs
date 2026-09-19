use crate::core::*;

use super::{
    economy::production_order,
    geometry::{finite_vec, mining_spot},
    tactics::{battle_action, extractor_action, healer_action},
    telemetry::Telemetry,
};

const LOW_BUDGET_THRESHOLD: i64 = 100_000;

pub struct Controller {
    last_tick: Option<u32>,
    payload_controller: Option<BotId>,
    cached_targets: [Option<BotId>; BOTS_MAX],
    telemetry: Telemetry,
}

impl Controller {
    pub fn new(team_slot: u8) -> Self {
        Self {
            last_tick: None,
            payload_controller: None,
            cached_targets: [None; BOTS_MAX],
            telemetry: Telemetry::new(team_slot),
        }
    }

    pub fn decide(&mut self, state: &GameState, conf: &GameConfig, budget: Budget) -> FleetAction {
        if self.last_tick.is_some_and(|tick| state.tick > tick + 1) {
            self.cached_targets.fill(None);
        }
        self.last_tick = Some(state.tick);

        let low_budget = budget.remaining < LOW_BUDGET_THRESHOLD;
        self.telemetry.observe(state, conf, budget, low_budget);

        let (fabricator_next, rush_order) = production_order(state, conf);
        let mut fleet_action = FleetAction::new();
        fleet_action.fabricator_next = fabricator_next;
        fleet_action.rush_order = rush_order;

        self.refresh_payload_controller(state);
        let mut reserved_enemies = [false; BOTS_MAX];
        let mut heal_assignments = [0u8; BOTS_MAX];
        let mut extractor_ordinal = 0usize;

        for bot in state.fleet_me.iter() {
            let action = match bot.class() {
                BotClass::Battle => {
                    let target = self.select_enemy(bot, state, low_budget, &reserved_enemies);
                    let controls_payload = self.payload_controller == Some(bot.id);
                    let (action, fires) = battle_action(bot, target, state, conf, controls_payload);
                    if let Some(enemy) = target {
                        self.cached_targets[bot.id as usize] = Some(enemy.id);
                        if fires {
                            reserved_enemies[enemy.id as usize] = true;
                        }
                    } else {
                        self.cached_targets[bot.id as usize] = None;
                    }
                    action
                }
                BotClass::Healer => {
                    let target = select_heal_target(bot, state, conf, &heal_assignments);
                    if let Some(ally) = target {
                        heal_assignments[ally.id as usize] += 1;
                    }
                    healer_action(bot, target, conf)
                }
                BotClass::Extractor => {
                    let (deposit, pos) = select_deposit(state, conf, bot.id, extractor_ordinal);
                    extractor_ordinal += 1;
                    extractor_action(bot, deposit, pos, conf)
                }
            };

            debug_assert!(finite_vec(action.move_action.direction));
            fleet_action.bots[bot.id as usize] = action;
        }

        self.telemetry.observe_action(&fleet_action);
        fleet_action
    }

    fn refresh_payload_controller(&mut self, state: &GameState) {
        if self
            .payload_controller
            .and_then(|id| state.fleet_me.get(id))
            .is_some_and(|bot| bot.class() == BotClass::Battle)
        {
            return;
        }
        self.payload_controller = state
            .fleet_me
            .iter()
            .filter(|bot| bot.class() == BotClass::Battle)
            .map(|bot| bot.id)
            .min();
    }

    fn select_enemy<'a>(
        &self,
        bot: &BotState,
        state: &'a GameState,
        low_budget: bool,
        reserved: &[bool; BOTS_MAX],
    ) -> Option<&'a BotState> {
        if low_budget {
            if let Some(cached) = self.cached_targets[bot.id as usize]
                .and_then(|id| state.fleet_other.get(id))
                .filter(|enemy| !reserved[enemy.id as usize])
            {
                return Some(cached);
            }
        }

        state
            .fleet_other
            .iter()
            .filter(|enemy| !reserved[enemy.id as usize])
            .min_by(|a, b| bot.pos.dist_sq(&a.pos).total_cmp(&bot.pos.dist_sq(&b.pos)))
            .or_else(|| {
                state
                    .fleet_other
                    .iter()
                    .min_by(|a, b| bot.pos.dist_sq(&a.pos).total_cmp(&bot.pos.dist_sq(&b.pos)))
            })
    }
}

fn select_heal_target<'a>(
    healer: &BotState,
    state: &'a GameState,
    conf: &GameConfig,
    assignments: &[u8; BOTS_MAX],
) -> Option<&'a BotState> {
    let cap = conf.bot.heal_stack_cap.max(1.0).floor() as u8;
    state
        .fleet_me
        .iter()
        .filter(|ally| {
            ally.id != healer.id
                && ally.health + 1e-6 < conf.bot.health
                && assignments[ally.id as usize] < cap
        })
        .min_by(|a, b| {
            let score =
                |ally: &BotState| ally.health / conf.bot.health + healer.pos.dist(&ally.pos) * 0.01;
            score(a).total_cmp(&score(b))
        })
}

fn select_deposit(state: &GameState, conf: &GameConfig, id: BotId, ordinal: usize) -> (Vec2, Vec2) {
    let cap = conf.deposit.extractor_cap as u32;
    let own_taken = state.deposit_me.extractors[Team::Me].count_ones()
        + state.deposit_me.extractors[Team::Other].count_ones();
    let incumbent_own = state.deposit_me.extractors[Team::Me] & (1u32 << id) != 0;
    let other_taken = state.deposit_other.extractors[Team::Me].count_ones()
        + state.deposit_other.extractors[Team::Other].count_ones();

    let deposit = if own_taken < cap || incumbent_own || other_taken >= cap {
        state.deposit_me.pos
    } else {
        state.deposit_other.pos
    };
    (deposit, mining_spot(conf, deposit, ordinal))
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

    #[test]
    fn r26_empty_fleet_and_active_classes_produce_legal_finite_actions() {
        let conf = config();
        init_topology(&conf.map, conf.bot.radius);
        let mut state = GameState::new(&conf);
        let mut controller = Controller::new(0);

        let empty = controller.decide(
            &state,
            &conf,
            Budget {
                remaining: Budget::BANK,
                last_charge: 0,
            },
        );
        assert_eq!(empty.fabricator_next, BotClass::Extractor);
        assert!(!empty.rush_order, "tick 0 must take the free natural build");

        for (class, pos) in [
            (BotClass::Battle, Vec2::new(2.0, 29.0)),
            (BotClass::Healer, Vec2::new(2.5, 29.0)),
            (BotClass::Extractor, Vec2::new(22.0, 29.0)),
        ] {
            let id = state.fleet_me.add();
            let bot = &mut state.fleet_me[id];
            bot.health = if class == BotClass::Battle { 9.0 } else { 10.0 };
            bot.pos = pos;
            bot.special = SpecialState::new(class);
        }

        let action = controller.decide(
            &state,
            &conf,
            Budget {
                remaining: Budget::BANK,
                last_charge: 1,
            },
        );
        for bot in state.fleet_me.iter() {
            let order = &action.bots[bot.id as usize];
            assert!(finite_vec(order.move_action.direction));
            assert!(!order.self_destruct);
            assert_eq!(order.special_action.class(), bot.class());
            if let SpecialAction::Healer { fire: true, target } = order.special_action {
                assert_ne!(target, bot.id);
                assert!(state.fleet_me.get(target).is_some());
            }
        }
    }

    #[test]
    fn both_team_slots_construct_the_same_controller_state_machine() {
        let a = Controller::new(0);
        let b = Controller::new(1);
        assert_eq!(a.last_tick, b.last_tick);
        assert_eq!(a.payload_controller, b.payload_controller);
        assert_eq!(a.cached_targets, b.cached_targets);
    }
}
