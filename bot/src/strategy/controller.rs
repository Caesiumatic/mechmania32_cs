use crate::core::*;

use super::{
    coordination::plan_healer_actions,
    economy::production_order,
    geometry::{finite_vec, mining_spot},
    policy::LOW_BUDGET_THRESHOLD,
    tactics::{battle_action, extractor_action, BattlePlanTelemetry},
    telemetry::Telemetry,
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct AllySnapshot {
    class: u8,
    health: f32,
    pos: Vec2,
}

impl AllySnapshot {
    fn from_bot(bot: &BotState) -> Self {
        let class = match bot.class() {
            BotClass::Battle => 0,
            BotClass::Healer => 1,
            BotClass::Extractor => 2,
        };
        Self {
            class,
            health: bot.health,
            pos: bot.pos,
        }
    }
}

pub struct Controller {
    last_tick: Option<u32>,
    payload_controller: Option<BotId>,
    cached_targets: [Option<BotId>; BOTS_MAX],
    cached_heal_targets: [Option<BotId>; BOTS_MAX],
    cached_support_targets: [Option<BotId>; BOTS_MAX],
    ally_snapshots: [Option<AllySnapshot>; BOTS_MAX],
    telemetry: Telemetry,
}

impl Controller {
    pub fn new(team_slot: u8) -> Self {
        Self {
            last_tick: None,
            payload_controller: None,
            cached_targets: [None; BOTS_MAX],
            cached_heal_targets: [None; BOTS_MAX],
            cached_support_targets: [None; BOTS_MAX],
            ally_snapshots: [None; BOTS_MAX],
            telemetry: Telemetry::new(team_slot),
        }
    }

    pub fn decide(&mut self, state: &GameState, conf: &GameConfig, budget: Budget) -> FleetAction {
        if self.last_tick.is_some_and(|tick| state.tick > tick + 1) {
            self.clear_assignment_caches();
        }
        self.refresh_ally_lifetimes(state, conf);
        self.last_tick = Some(state.tick);

        let low_budget = budget.remaining < LOW_BUDGET_THRESHOLD;
        self.telemetry.observe(state, conf, budget, low_budget);

        let (fabricator_next, rush_order) = production_order(state, conf);
        let mut fleet_action = FleetAction::new();
        fleet_action.fabricator_next = fabricator_next;
        fleet_action.rush_order = rush_order;

        self.refresh_payload_controller(state);
        let mut covered_enemies = 0u32;
        let mut extractor_ordinal = 0usize;
        let mut battle_telemetry = [BattlePlanTelemetry::default(); BOTS_MAX];

        for bot in state.fleet_me.iter() {
            let action = match bot.class() {
                BotClass::Battle => {
                    let target = self.select_enemy(bot, state, low_budget, covered_enemies);
                    let controls_payload = self.payload_controller == Some(bot.id);
                    let (action, newly_covered, plan_telemetry) = battle_action(
                        bot,
                        target,
                        state,
                        conf,
                        controls_payload,
                        covered_enemies,
                        low_budget,
                    );
                    battle_telemetry[bot.id as usize] = plan_telemetry;
                    if let Some(enemy) = target {
                        self.cached_targets[bot.id as usize] = Some(enemy.id);
                    } else {
                        self.cached_targets[bot.id as usize] = None;
                    }
                    covered_enemies |= newly_covered;
                    action
                }
                // Healer movement and channels are assigned together only after all other
                // classes have proposed movement, so every candidate sees one team snapshot.
                BotClass::Healer => continue,
                BotClass::Extractor => {
                    let (deposit, pos) = select_deposit(state, conf, bot.id, extractor_ordinal);
                    extractor_ordinal += 1;
                    extractor_action(bot, deposit, pos, conf)
                }
            };

            debug_assert!(finite_vec(action.move_action.direction));
            fleet_action.bots[bot.id as usize] = action;
        }

        let heal_stats = plan_healer_actions(
            state,
            conf,
            &mut fleet_action,
            &mut self.cached_heal_targets,
            &mut self.cached_support_targets,
        );
        for bot in state.fleet_me.iter() {
            let action = &fleet_action.bots[bot.id as usize];
            debug_assert!(finite_vec(action.move_action.direction));
            debug_assert_eq!(action.special_action.class(), bot.class());
        }

        self.telemetry.observe_action(
            state,
            conf,
            &fleet_action,
            heal_stats,
            &battle_telemetry,
            &self.cached_support_targets,
        );
        fleet_action
    }

    fn clear_assignment_caches(&mut self) {
        self.cached_targets.fill(None);
        self.cached_heal_targets.fill(None);
        self.cached_support_targets.fill(None);
    }

    fn invalidate_ally_slot(&mut self, id: BotId) {
        for cache in [
            &mut self.cached_heal_targets,
            &mut self.cached_support_targets,
        ] {
            cache[id as usize] = None;
            for target in cache.iter_mut() {
                if *target == Some(id) {
                    *target = None;
                }
            }
        }
    }

    fn refresh_ally_lifetimes(&mut self, state: &GameState, conf: &GameConfig) {
        let elapsed = self
            .last_tick
            .map(|tick| state.tick.saturating_sub(tick).max(1))
            .unwrap_or(1) as f32;
        let max_health_gain = conf.bot.heal_per_tick * conf.bot.heal_stack_cap * elapsed + 1e-4;
        // Collisions can add displacement beyond commanded speed. This bound is deliberately
        // generous, while still detecting the spawn-corner jump of a reused slot.
        let max_displacement = conf.bot.speed * elapsed + conf.bot.radius * 4.0 + 1e-4;

        for index in 0..BOTS_MAX {
            let current = state
                .fleet_me
                .get(index as BotId)
                .map(AllySnapshot::from_bot);
            let changed_lifetime = match (self.ally_snapshots[index], current) {
                (None, None) => false,
                (None, Some(_)) | (Some(_), None) => true,
                (Some(old), Some(new)) => {
                    old.class != new.class
                        || new.health > old.health + max_health_gain
                        || old.pos.dist(&new.pos) > max_displacement
                }
            };
            if changed_lifetime {
                self.invalidate_ally_slot(index as BotId);
            }
            self.ally_snapshots[index] = current;
        }
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
        covered: u32,
    ) -> Option<&'a BotState> {
        if low_budget {
            if let Some(cached) = self.cached_targets[bot.id as usize]
                .and_then(|id| state.fleet_other.get(id))
                .filter(|enemy| covered & (1u32 << enemy.id) == 0)
            {
                return Some(cached);
            }
        }

        state
            .fleet_other
            .iter()
            .filter(|enemy| covered & (1u32 << enemy.id) == 0)
            .min_by(|a, b| bot.pos.dist_sq(&a.pos).total_cmp(&bot.pos.dist_sq(&b.pos)))
            .or_else(|| {
                state
                    .fleet_other
                    .iter()
                    .min_by(|a, b| bot.pos.dist_sq(&a.pos).total_cmp(&bot.pos.dist_sq(&b.pos)))
            })
    }
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
        assert_eq!(a.cached_heal_targets, b.cached_heal_targets);
        assert_eq!(a.cached_support_targets, b.cached_support_targets);
        assert_eq!(a.ally_snapshots, b.ally_snapshots);
    }

    fn spawn(
        state: &mut GameState,
        conf: &GameConfig,
        class: BotClass,
        pos: Vec2,
        health: f32,
    ) -> BotId {
        let id = state.fleet_me.add();
        let bot = &mut state.fleet_me[id];
        bot.id = id;
        bot.health = health.min(conf.bot.health);
        bot.pos = pos;
        bot.vel = Vec2::ZERO;
        bot.angle = 0.0;
        bot.turn_vel = 0.0;
        bot.invulnerable_until_tick = 0;
        bot.special = SpecialState::new(class);
        id
    }

    #[test]
    fn d09_death_slot_reuse_and_tick_jump_drop_stale_coordination_cache() {
        let conf = config();
        init_topology(&conf.map, conf.bot.radius);
        let mut state = GameState::new(&conf);
        let healer = spawn(
            &mut state,
            &conf,
            BotClass::Healer,
            Vec2::new(10.0, 10.0),
            conf.bot.health,
        );
        let wounded = spawn(
            &mut state,
            &conf,
            BotClass::Battle,
            Vec2::new(12.0, 10.0),
            9.0,
        );
        let mut controller = Controller::new(0);
        let budget = Budget {
            remaining: Budget::BANK,
            last_charge: 0,
        };

        let first = controller.decide(&state, &conf, budget);
        assert_eq!(
            controller.cached_heal_targets[healer as usize],
            Some(wounded)
        );
        assert_eq!(
            controller.cached_support_targets[healer as usize],
            Some(wounded)
        );
        assert!(matches!(
            first.bots[healer as usize].special_action,
            SpecialAction::Healer {
                fire: true,
                target
            } if target == wounded
        ));

        state.fleet_me.remove(wounded);
        state.tick += 1;
        controller.decide(&state, &conf, budget);
        assert_eq!(controller.cached_heal_targets[healer as usize], None);
        assert_eq!(controller.cached_support_targets[healer as usize], None);

        let reused = spawn(
            &mut state,
            &conf,
            BotClass::Extractor,
            Vec2::new(22.0, 29.0),
            conf.bot.health,
        );
        assert_eq!(reused, wounded, "BotArray must reuse the first free slot");
        controller.cached_heal_targets[healer as usize] = Some(reused);
        controller.cached_support_targets[healer as usize] = Some(reused);
        state.tick += 2;
        let after_jump = controller.decide(&state, &conf, budget);
        assert_eq!(controller.cached_heal_targets[healer as usize], None);
        // With no battle unit, the valid non-healer fallback may deliberately become the
        // new support anchor; the stale heal binding must not become a channel.
        assert!(matches!(
            after_jump.bots[healer as usize].special_action,
            SpecialAction::Healer { fire: false, .. }
        ));
    }
}
