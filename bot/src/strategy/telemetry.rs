use crate::core::*;

use super::{
    coordination::HealPlanStats,
    tactics::{BattlePlanTelemetry, ShotPrediction},
};

const HEALTH_EPSILON: f32 = 1e-4;
const HIGH_POPULATION_NUMERATOR: usize = 3;
const HIGH_POPULATION_DENOMINATOR: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct EnemySnapshot {
    class: u8,
    health: f32,
    invulnerable_until_tick: u32,
}

impl EnemySnapshot {
    fn from_bot(bot: &BotState) -> Self {
        let class = match bot.class() {
            BotClass::Battle => 0,
            BotClass::Healer => 1,
            BotClass::Extractor => 2,
        };
        Self {
            class,
            health: bot.health,
            invulnerable_until_tick: bot.invulnerable_until_tick,
        }
    }

    fn same_lifetime(&self, bot: &BotState) -> bool {
        let class = match bot.class() {
            BotClass::Battle => 0,
            BotClass::Healer => 1,
            BotClass::Extractor => 2,
        };
        self.class == class
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingShot {
    prediction: Option<ShotPrediction>,
}

#[derive(Debug)]
struct PendingFrame {
    tick: u32,
    shots: [Option<PendingShot>; BOTS_MAX],
    heals: [Option<BotId>; BOTS_MAX],
    enemies: [Option<EnemySnapshot>; BOTS_MAX],
    predicted_new_damage_mask: u32,
}

impl PendingFrame {
    fn fire_requests(&self) -> u64 {
        self.shots.iter().filter(|shot| shot.is_some()).count() as u64
    }

    fn shot_predictions(&self) -> u64 {
        self.shots
            .iter()
            .flatten()
            .filter(|shot| shot.prediction.is_some())
            .count() as u64
    }

    fn heal_requests(&self) -> u64 {
        self.heals.iter().filter(|target| target.is_some()).count() as u64
    }

    fn enemy_snapshots(&self) -> u64 {
        self.enemies.iter().filter(|bot| bot.is_some()).count() as u64
    }
}

#[derive(Debug)]
pub struct Telemetry {
    team_slot: u8,
    first_tick: Option<u32>,
    last_tick: Option<u32>,
    calls: u64,
    observed_charges: Vec<u64>,
    min_remaining: i64,
    skipped_ticks: u64,
    low_budget_ticks: u64,
    was_low_budget: bool,
    first_payload_arrival: Option<u32>,
    first_mining: Option<u32>,
    first_contact: Option<u32>,

    // Fire intent and next-frame feedback. A request is observable only when the same
    // battle slot is present on the immediately following callback.
    fire_requests: u64,
    fire_feedback_observable: u64,
    fire_observed: u64,
    fire_not_observed: u64,
    fire_feedback_censored: u64,
    shot_prediction_samples: u64,
    shot_prediction_missing: u64,
    blast_error_samples: u64,
    blast_error_sum: f64,
    blast_error_max: f32,
    blast_error_censored: u64,
    fire_candidates_evaluated: u64,
    zero_marginal_candidates_skipped: u64,
    geometry_uncertain_candidates: u64,
    zero_marginal_fire_suppressions: u64,

    // Team-level enemy transitions after at least one fire request. A surviving slot is
    // positively identified as newly damaged by lower health or a newly advanced
    // invulnerability deadline. Deaths are intentionally censored rather than inferred.
    predicted_new_damage_targets: u64,
    damage_feedback_frames: u64,
    damage_feedback_complete_frames: u64,
    damage_feedback_censored_frames: u64,
    damage_transition_observable: u64,
    damage_transition_censored: u64,
    actual_new_damaged_targets: u64,

    // Healing intent, realised channels, support travel and explicit denominators.
    heal_requests: u64,
    heal_request_ticks: u64,
    heal_feedback_observable: u64,
    heal_observed_channels: u64,
    heal_not_observed: u64,
    heal_target_matches: u64,
    heal_target_mismatches: u64,
    heal_feedback_censored: u64,
    heal_observed_ticks: u64,
    planned_effective_heal_channels: u64,
    healer_channel_tick_slots: u64,
    demand_limited_heal_capacity_slots: u64,
    planned_support_travelers: u64,
    support_travel_ticks: u64,
    support_anchor_samples: u64,
    first_healer_support_radius: Option<u32>,
    full_health_follow_samples: u64,
    full_health_follow_distance_sum: f64,
    full_health_follow_distance_max: f32,
    far_support_travelers: u64,
    far_support_travel_ticks: u64,

    // State-only spatial diagnostics. These count callbacks, never interpolate skipped
    // ticks, and expose their own denominators in the summary.
    friendly_control_presence_ticks: u64,
    enemy_control_presence_ticks: u64,
    contested_control_ticks: u64,
    high_population_ticks: u64,
    max_friendly_population: u32,
    friendly_pair_samples: u64,
    near_neighbor_pairs: u64,
    near_neighbor_ticks: u64,
    near_neighbor_threshold: Option<f32>,

    pending: Option<PendingFrame>,
}

impl Telemetry {
    pub fn new(team_slot: u8) -> Self {
        Self {
            team_slot,
            first_tick: None,
            last_tick: None,
            calls: 0,
            observed_charges: Vec::with_capacity(9_000),
            min_remaining: Budget::BANK,
            skipped_ticks: 0,
            low_budget_ticks: 0,
            was_low_budget: false,
            first_payload_arrival: None,
            first_mining: None,
            first_contact: None,
            fire_requests: 0,
            fire_feedback_observable: 0,
            fire_observed: 0,
            fire_not_observed: 0,
            fire_feedback_censored: 0,
            shot_prediction_samples: 0,
            shot_prediction_missing: 0,
            blast_error_samples: 0,
            blast_error_sum: 0.0,
            blast_error_max: 0.0,
            blast_error_censored: 0,
            fire_candidates_evaluated: 0,
            zero_marginal_candidates_skipped: 0,
            geometry_uncertain_candidates: 0,
            zero_marginal_fire_suppressions: 0,
            predicted_new_damage_targets: 0,
            damage_feedback_frames: 0,
            damage_feedback_complete_frames: 0,
            damage_feedback_censored_frames: 0,
            damage_transition_observable: 0,
            damage_transition_censored: 0,
            actual_new_damaged_targets: 0,
            heal_requests: 0,
            heal_request_ticks: 0,
            heal_feedback_observable: 0,
            heal_observed_channels: 0,
            heal_not_observed: 0,
            heal_target_matches: 0,
            heal_target_mismatches: 0,
            heal_feedback_censored: 0,
            heal_observed_ticks: 0,
            planned_effective_heal_channels: 0,
            healer_channel_tick_slots: 0,
            demand_limited_heal_capacity_slots: 0,
            planned_support_travelers: 0,
            support_travel_ticks: 0,
            support_anchor_samples: 0,
            first_healer_support_radius: None,
            full_health_follow_samples: 0,
            full_health_follow_distance_sum: 0.0,
            full_health_follow_distance_max: 0.0,
            far_support_travelers: 0,
            far_support_travel_ticks: 0,
            friendly_control_presence_ticks: 0,
            enemy_control_presence_ticks: 0,
            contested_control_ticks: 0,
            high_population_ticks: 0,
            max_friendly_population: 0,
            friendly_pair_samples: 0,
            near_neighbor_pairs: 0,
            near_neighbor_ticks: 0,
            near_neighbor_threshold: None,
            pending: None,
        }
    }

    pub fn observe(&mut self, state: &GameState, conf: &GameConfig, budget: Budget, low: bool) {
        self.reconcile_pending(state);

        self.first_tick.get_or_insert(state.tick);
        let mut jumped = false;
        if let Some(previous) = self.last_tick {
            if state.tick > previous + 1 {
                self.skipped_ticks += (state.tick - previous - 1) as u64;
                jumped = true;
            }
        }
        self.last_tick = Some(state.tick);
        self.calls += 1;
        self.min_remaining = self.min_remaining.min(budget.remaining);
        if state.tick > 0 {
            self.observed_charges.push(budget.last_charge);
        }
        if low {
            self.low_budget_ticks += 1;
        }

        let payload = state.payload_pos();
        if self.first_payload_arrival.is_none()
            && state
                .fleet_me
                .iter()
                .any(|bot| bot.pos.dist(&payload) <= conf.payload.capture_radius)
        {
            self.first_payload_arrival = Some(state.tick);
        }
        if self.first_mining.is_none()
            && state
                .fleet_me
                .iter()
                .any(|bot| bot.extracting().option().is_some())
        {
            self.first_mining = Some(state.tick);
        }
        if self.first_contact.is_none()
            && (state
                .fleet_me
                .iter()
                .any(|bot| bot.shot().option().is_some() || bot.health < conf.bot.health)
                || state
                    .fleet_other
                    .iter()
                    .any(|bot| bot.health < conf.bot.health))
        {
            self.first_contact = Some(state.tick);
        }

        self.observe_spatial_state(state, conf);

        let low_budget_started = low && !self.was_low_budget;
        self.was_low_budget = low;
        if (state.tick > 0 && state.tick % 250 == 0) || jumped || low_budget_started {
            println!(
                "MM32_TELEMETRY {}",
                serde_json::json!({
                    "team_slot": self.team_slot,
                    "tick": state.tick,
                    "calls": self.calls,
                    "last_charge": budget.last_charge,
                    "remaining": budget.remaining,
                    "skipped_ticks": self.skipped_ticks,
                    "low_budget_ticks": self.low_budget_ticks,
                    "first_payload_arrival": self.first_payload_arrival,
                    "first_mining": self.first_mining,
                    "first_contact": self.first_contact,
                    "fire_requests": self.fire_requests,
                    "fire_observed": self.fire_observed,
                    "fire_feedback_censored": self.fire_feedback_censored,
                    "actual_new_damaged_targets": self.actual_new_damaged_targets,
                    "heal_requests": self.heal_requests,
                    "heal_observed_channels": self.heal_observed_channels,
                    "contested_control_ticks": self.contested_control_ticks,
                    "high_population_ticks": self.high_population_ticks,
                }),
            );
        }
    }

    fn reconcile_pending(&mut self, state: &GameState) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if state.tick != pending.tick.saturating_add(1) {
            self.censor_pending(&pending);
            return;
        }

        let mut observed_heal_this_tick = false;
        for (id, pending_shot) in pending.shots.iter().enumerate() {
            let Some(pending_shot) = pending_shot else {
                continue;
            };
            let current = state.fleet_me.get(id as BotId);
            if !current.is_some_and(|bot| bot.class() == BotClass::Battle) {
                self.fire_feedback_censored += 1;
                if pending_shot.prediction.is_some() {
                    self.blast_error_censored += 1;
                }
                continue;
            }

            self.fire_feedback_observable += 1;
            let actual = current.and_then(|bot| bot.shot().option());
            if let Some(actual) = actual {
                self.fire_observed += 1;
                if let Some(prediction) = pending_shot.prediction {
                    let error = actual.dist(&prediction.blast_point);
                    self.blast_error_samples += 1;
                    self.blast_error_sum += error as f64;
                    self.blast_error_max = self.blast_error_max.max(error);
                }
            } else {
                self.fire_not_observed += 1;
                if pending_shot.prediction.is_some() {
                    self.blast_error_censored += 1;
                }
            }
        }

        for (id, expected_target) in pending.heals.iter().enumerate() {
            let Some(expected_target) = expected_target else {
                continue;
            };
            let current = state.fleet_me.get(id as BotId);
            if !current.is_some_and(|bot| bot.class() == BotClass::Healer) {
                self.heal_feedback_censored += 1;
                continue;
            }

            self.heal_feedback_observable += 1;
            match current.and_then(|bot| bot.healing().option()) {
                Some(actual_target) => {
                    self.heal_observed_channels += 1;
                    observed_heal_this_tick = true;
                    if actual_target == *expected_target {
                        self.heal_target_matches += 1;
                    } else {
                        self.heal_target_mismatches += 1;
                    }
                }
                None => self.heal_not_observed += 1,
            }
        }
        if observed_heal_this_tick {
            self.heal_observed_ticks += 1;
        }

        if pending.fire_requests() > 0 {
            self.damage_feedback_frames += 1;
            let mut complete = true;
            for (id, old) in pending.enemies.iter().enumerate() {
                let Some(old) = old else {
                    continue;
                };
                let current = state.fleet_other.get(id as BotId);
                let Some(current) = current.filter(|bot| old.same_lifetime(bot)) else {
                    self.damage_transition_censored += 1;
                    complete = false;
                    continue;
                };
                self.damage_transition_observable += 1;
                let new_invulnerability = current.invulnerable_until_tick
                    > old.invulnerable_until_tick
                    && current.invulnerable_until_tick > pending.tick;
                let lower_health = current.health + HEALTH_EPSILON < old.health;
                if new_invulnerability || lower_health {
                    self.actual_new_damaged_targets += 1;
                }
            }
            if complete {
                self.damage_feedback_complete_frames += 1;
            } else {
                self.damage_feedback_censored_frames += 1;
            }
        }
    }

    fn censor_pending(&mut self, pending: &PendingFrame) {
        self.fire_feedback_censored += pending.fire_requests();
        self.blast_error_censored += pending.shot_predictions();
        self.heal_feedback_censored += pending.heal_requests();
        if pending.fire_requests() > 0 {
            self.damage_feedback_censored_frames += 1;
            self.damage_transition_censored += pending.enemy_snapshots();
        }
    }

    fn observe_spatial_state(&mut self, state: &GameState, conf: &GameConfig) {
        self.near_neighbor_threshold = Some(Self::near_neighbor_distance(conf));
        let payload = state.payload_pos();
        let capture_radius_sq = conf.payload.capture_radius.powi(2);
        let friendly_present = state
            .fleet_me
            .iter()
            .any(|bot| bot.pos.dist_sq(&payload) <= capture_radius_sq);
        let enemy_present = state
            .fleet_other
            .iter()
            .any(|bot| bot.pos.dist_sq(&payload) <= capture_radius_sq);
        self.friendly_control_presence_ticks += friendly_present as u64;
        self.enemy_control_presence_ticks += enemy_present as u64;
        self.contested_control_ticks += (friendly_present && enemy_present) as u64;

        let population = state.fleet_me.iter().count();
        self.max_friendly_population = self.max_friendly_population.max(population as u32);
        self.high_population_ticks += (population >= Self::high_population_threshold()) as u64;

        let near_distance_sq = Self::near_neighbor_distance(conf).powi(2);
        let mut near_this_tick = 0u64;
        for left in state.fleet_me.iter() {
            for right in state.fleet_me.iter().filter(|right| right.id > left.id) {
                self.friendly_pair_samples += 1;
                if left.pos.dist_sq(&right.pos) <= near_distance_sq {
                    self.near_neighbor_pairs += 1;
                    near_this_tick += 1;
                }
            }
        }
        self.near_neighbor_ticks += (near_this_tick > 0) as u64;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn observe_action(
        &mut self,
        state: &GameState,
        conf: &GameConfig,
        action: &FleetAction,
        healing: HealPlanStats,
        battle: &[BattlePlanTelemetry; BOTS_MAX],
        support_targets: &[Option<BotId>; BOTS_MAX],
    ) {
        // This should only be non-empty for a repeated decide call on the same state. Do not
        // silently overwrite evidence if a host violates the one-callback-per-state contract.
        if let Some(stale) = self.pending.take() {
            self.censor_pending(&stale);
        }

        let mut shots = [None; BOTS_MAX];
        let mut heals = [None; BOTS_MAX];
        let mut enemies = [None; BOTS_MAX];
        let mut predicted_new_damage_mask = 0u32;
        let mut requested_heals = 0u64;
        let mut far_support_this_tick = false;

        for bot in state.fleet_other.iter() {
            enemies[bot.id as usize] = Some(EnemySnapshot::from_bot(bot));
        }

        for bot in state.fleet_me.iter() {
            let order = &action.bots[bot.id as usize];
            match order.special_action {
                SpecialAction::Battle { fire: true } => {
                    let prediction = battle[bot.id as usize].shot;
                    shots[bot.id as usize] = Some(PendingShot { prediction });
                    self.fire_requests += 1;
                    if let Some(prediction) = prediction {
                        self.shot_prediction_samples += 1;
                        predicted_new_damage_mask |= prediction.predicted_new_damage_mask;
                    } else {
                        self.shot_prediction_missing += 1;
                    }
                }
                SpecialAction::Healer { fire: true, target } => {
                    heals[bot.id as usize] = Some(target);
                    requested_heals += 1;
                    self.heal_requests += 1;
                }
                _ => {}
            }

            let plan = battle[bot.id as usize];
            self.fire_candidates_evaluated += plan.candidates_evaluated as u64;
            self.zero_marginal_candidates_skipped += plan.zero_marginal_candidates as u64;
            self.geometry_uncertain_candidates += plan.geometry_uncertain_candidates as u64;
            self.zero_marginal_fire_suppressions += plan.zero_marginal_suppression as u64;
        }
        self.predicted_new_damage_targets += predicted_new_damage_mask.count_ones() as u64;

        if requested_heals > 0 {
            self.heal_request_ticks += 1;
        }
        self.planned_effective_heal_channels += healing.effective_channels as u64;
        self.planned_support_travelers += healing.support_travelers as u64;
        if healing.support_travelers > 0 {
            self.support_travel_ticks += 1;
        }

        let healer_count = state
            .fleet_me
            .iter()
            .filter(|bot| bot.class() == BotClass::Healer)
            .count() as u64;
        self.healer_channel_tick_slots += healer_count;
        let demand_capacity: u64 = state
            .fleet_me
            .iter()
            .map(|bot| {
                let missing = (conf.bot.health - bot.health).max(0.0);
                if missing <= HEALTH_EPSILON || conf.bot.heal_per_tick <= 0.0 {
                    0
                } else {
                    let channels = ((missing - HEALTH_EPSILON).max(0.0) / conf.bot.heal_per_tick)
                        .ceil() as u64;
                    channels.min(conf.bot.heal_stack_cap.max(0.0).floor() as u64)
                }
            })
            .sum();
        self.demand_limited_heal_capacity_slots += demand_capacity.min(healer_count);

        let all_allies_full = state.fleet_me.iter().next().is_some()
            && state
                .fleet_me
                .iter()
                .all(|bot| bot.health + HEALTH_EPSILON >= conf.bot.health);
        for healer in state
            .fleet_me
            .iter()
            .filter(|bot| bot.class() == BotClass::Healer)
        {
            let Some(anchor) =
                support_targets[healer.id as usize].and_then(|id| state.fleet_me.get(id))
            else {
                continue;
            };
            let distance = healer.pos.dist(&anchor.pos);
            self.support_anchor_samples += 1;
            if distance <= conf.bot.base_heal_range {
                self.first_healer_support_radius.get_or_insert(state.tick);
            }
            if all_allies_full {
                self.full_health_follow_samples += 1;
                self.full_health_follow_distance_sum += distance as f64;
                self.full_health_follow_distance_max =
                    self.full_health_follow_distance_max.max(distance);
            }
            if distance > conf.bot.base_heal_range && order_moves(&action.bots[healer.id as usize])
            {
                self.far_support_travelers += 1;
                far_support_this_tick = true;
            }
        }
        self.far_support_travel_ticks += far_support_this_tick as u64;

        self.pending = Some(PendingFrame {
            tick: state.tick,
            shots,
            heals,
            enemies,
            predicted_new_damage_mask,
        });
    }

    fn percentile(sorted: &[u64], numerator: usize) -> Option<u64> {
        (!sorted.is_empty()).then(|| sorted[(sorted.len() - 1) * numerator / 100])
    }

    fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
        (denominator > 0).then(|| numerator as f64 / denominator as f64)
    }

    fn mean(sum: f64, samples: u64) -> Option<f64> {
        (samples > 0).then(|| sum / samples as f64)
    }

    const fn high_population_threshold() -> usize {
        (BOTS_MAX * HIGH_POPULATION_NUMERATOR).div_ceil(HIGH_POPULATION_DENOMINATOR)
    }

    fn near_neighbor_distance(conf: &GameConfig) -> f32 {
        // A compact, splash-relevant threshold: two hull radii plus one splash radius.
        conf.bot.radius * 2.0 + conf.bot.base_blaster_splash_radius
    }

    fn summary(&self) -> serde_json::Value {
        let mut sorted = self.observed_charges.clone();
        sorted.sort_unstable();
        let sum: u128 = sorted.iter().map(|&v| v as u128).sum();
        let mean = (!sorted.is_empty()).then(|| sum as f64 / sorted.len() as f64);
        let pending_fire = self.pending.as_ref().map_or(0, PendingFrame::fire_requests);
        let pending_predictions = self
            .pending
            .as_ref()
            .map_or(0, PendingFrame::shot_predictions);
        let pending_heals = self.pending.as_ref().map_or(0, PendingFrame::heal_requests);
        let pending_damage_targets = self
            .pending
            .as_ref()
            .filter(|pending| pending.fire_requests() > 0)
            .map_or(0, PendingFrame::enemy_snapshots);
        let pending_predicted_damage_targets = self.pending.as_ref().map_or(0, |pending| {
            pending.predicted_new_damage_mask.count_ones() as u64
        });

        // Split the compile-time JSON construction into shallow pieces. The final object is
        // still flat (for the runner), while avoiding a large recursive macro expansion in
        // the official client build.
        let sections = [
            serde_json::json!({
                "telemetry_schema": "m02_behavior_v1",
                "team_slot": self.team_slot,
                "first_tick": self.first_tick,
                "last_tick": self.last_tick,
                "calls": self.calls,
                "state_observation_ticks": self.calls,
                "observed_charges": sorted.len(),
                "charge_mean": mean,
                "charge_p95": Self::percentile(&sorted, 95),
                "charge_p99": Self::percentile(&sorted, 99),
                "charge_peak": sorted.last().copied(),
                "min_remaining": self.min_remaining,
                "skipped_ticks": self.skipped_ticks,
                "low_budget_ticks": self.low_budget_ticks,
                "first_payload_arrival": self.first_payload_arrival,
                "first_mining": self.first_mining,
                "first_contact": self.first_contact,
            }),
            serde_json::json!({
                "fire_requests": self.fire_requests,
                "fire_feedback_observable": self.fire_feedback_observable,
                "fire_observed": self.fire_observed,
                "fire_not_observed": self.fire_not_observed,
                "fire_feedback_censored": self.fire_feedback_censored,
                "fire_feedback_pending": pending_fire,
                "fire_observed_rate": Self::ratio(self.fire_observed, self.fire_feedback_observable),
                "shot_prediction_samples": self.shot_prediction_samples,
                "shot_prediction_missing": self.shot_prediction_missing,
                "blast_error_samples": self.blast_error_samples,
                "blast_error_mean": Self::mean(self.blast_error_sum, self.blast_error_samples),
                "blast_error_max": (self.blast_error_samples > 0).then_some(self.blast_error_max),
                "blast_error_censored": self.blast_error_censored,
                "blast_error_pending": pending_predictions,
            "fire_candidates_evaluated": self.fire_candidates_evaluated,
            "zero_marginal_candidates_skipped": self.zero_marginal_candidates_skipped,
            "geometry_uncertain_candidates": self.geometry_uncertain_candidates,
            "zero_marginal_fire_suppressions": self.zero_marginal_fire_suppressions,
            }),
            serde_json::json!({
                "predicted_new_damage_targets": self.predicted_new_damage_targets,
                "predicted_new_damage_targets_pending": pending_predicted_damage_targets,
                "damage_feedback_frames": self.damage_feedback_frames,
                "damage_feedback_complete_frames": self.damage_feedback_complete_frames,
                "damage_feedback_censored_frames": self.damage_feedback_censored_frames,
                "damage_transition_observable": self.damage_transition_observable,
                "damage_transition_censored": self.damage_transition_censored,
                "damage_transition_pending": pending_damage_targets,
                "actual_new_damaged_targets": self.actual_new_damaged_targets,
            }),
            serde_json::json!({
                "heal_requests": self.heal_requests,
                "heal_request_ticks": self.heal_request_ticks,
                "heal_feedback_observable": self.heal_feedback_observable,
                "heal_observed_channels": self.heal_observed_channels,
                "heal_not_observed": self.heal_not_observed,
                "heal_target_matches": self.heal_target_matches,
                "heal_target_mismatches": self.heal_target_mismatches,
                "heal_feedback_censored": self.heal_feedback_censored,
                "heal_feedback_pending": pending_heals,
                "heal_observed_ticks": self.heal_observed_ticks,
                "planned_effective_heal_channels": self.planned_effective_heal_channels,
                "healer_channel_tick_slots": self.healer_channel_tick_slots,
                "demand_limited_heal_capacity_slots": self.demand_limited_heal_capacity_slots,
                "effective_heal_capacity_occupancy": Self::ratio(
                    self.planned_effective_heal_channels,
                    self.demand_limited_heal_capacity_slots,
                ),
                "planned_support_travelers": self.planned_support_travelers,
                "support_travel_ticks": self.support_travel_ticks,
            }),
            serde_json::json!({
                "support_anchor_samples": self.support_anchor_samples,
                "first_healer_support_radius": self.first_healer_support_radius,
                "full_health_follow_samples": self.full_health_follow_samples,
                "full_health_follow_distance_mean": Self::mean(
                    self.full_health_follow_distance_sum,
                    self.full_health_follow_samples,
                ),
                "full_health_follow_distance_max": (self.full_health_follow_samples > 0)
                    .then_some(self.full_health_follow_distance_max),
                "far_support_travelers": self.far_support_travelers,
                "far_support_travel_ticks": self.far_support_travel_ticks,
                "control_observation_ticks": self.calls,
                "friendly_control_presence_ticks": self.friendly_control_presence_ticks,
                "enemy_control_presence_ticks": self.enemy_control_presence_ticks,
                "contested_control_ticks": self.contested_control_ticks,
                "high_population_threshold": Self::high_population_threshold(),
                "high_population_ticks": self.high_population_ticks,
                "max_friendly_population": self.max_friendly_population,
                "near_neighbor_distance": self.near_neighbor_threshold,
                "friendly_pair_samples": self.friendly_pair_samples,
                "near_neighbor_pairs": self.near_neighbor_pairs,
                "near_neighbor_pair_rate": Self::ratio(
                    self.near_neighbor_pairs,
                    self.friendly_pair_samples,
                ),
                "near_neighbor_ticks": self.near_neighbor_ticks,
            }),
        ];
        let mut summary = serde_json::Map::new();
        for section in sections {
            let serde_json::Value::Object(fields) = section else {
                unreachable!("telemetry section must be an object");
            };
            summary.extend(fields);
        }
        serde_json::Value::Object(summary)
    }
}

fn order_moves(action: &BotAction) -> bool {
    action.move_action.direction.norm_sq() > 0.0
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        println!("MM32_SUMMARY {}", self.summary());
    }
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

    fn spawn_battle(fleet: &mut BotArray, conf: &GameConfig, pos: Vec2) -> BotId {
        let id = fleet.add();
        let bot = &mut fleet[id];
        bot.id = id;
        bot.health = conf.bot.health;
        bot.pos = pos;
        bot.special = SpecialState::new(BotClass::Battle);
        id
    }

    #[test]
    fn next_frame_feedback_counts_observed_shot_error_and_damage() {
        let conf = config();
        let mut state = GameState::new(&conf);
        let shooter = spawn_battle(&mut state.fleet_me, &conf, Vec2::new(4.0, 4.0));
        let enemy = spawn_battle(&mut state.fleet_other, &conf, Vec2::new(8.0, 4.0));
        let budget = Budget {
            remaining: Budget::BANK,
            last_charge: 0,
        };
        let mut telemetry = Telemetry::new(0);
        telemetry.observe(&state, &conf, budget, false);

        let predicted = Vec2::new(7.5, 4.0);
        let mut action = FleetAction::new();
        action.bots[shooter as usize].special_action = SpecialAction::Battle { fire: true };
        let mut battle = [BattlePlanTelemetry::default(); BOTS_MAX];
        battle[shooter as usize].shot = Some(ShotPrediction {
            blast_point: predicted,
            predicted_new_damage_mask: 1u32 << enemy,
        });
        telemetry.observe_action(
            &state,
            &conf,
            &action,
            HealPlanStats::default(),
            &battle,
            &[None; BOTS_MAX],
        );

        state.tick = 1;
        state.fleet_me[shooter].special = SpecialState::Battle {
            next_fire_tick: 60,
            shot: StateOption::Some(Vec2::new(7.75, 4.0)),
        };
        state.fleet_other[enemy].health -= conf.bot.blaster_damage;
        state.fleet_other[enemy].invulnerable_until_tick = conf.bot.base_invulnerability_ticks;
        telemetry.observe(&state, &conf, budget, false);

        assert_eq!(telemetry.fire_requests, 1);
        assert_eq!(telemetry.fire_feedback_observable, 1);
        assert_eq!(telemetry.fire_observed, 1);
        assert_eq!(telemetry.blast_error_samples, 1);
        assert!((telemetry.blast_error_sum - 0.25).abs() < 1e-6);
        assert_eq!(telemetry.actual_new_damaged_targets, 1);
        assert_eq!(telemetry.damage_transition_observable, 1);
        assert_eq!(telemetry.damage_transition_censored, 0);
    }

    #[test]
    fn skipped_callback_censors_feedback_and_terminal_action_stays_pending() {
        let conf = config();
        let mut state = GameState::new(&conf);
        let shooter = spawn_battle(&mut state.fleet_me, &conf, Vec2::new(4.0, 4.0));
        let _enemy = spawn_battle(&mut state.fleet_other, &conf, Vec2::new(8.0, 4.0));
        let budget = Budget {
            remaining: Budget::BANK,
            last_charge: 0,
        };
        let mut telemetry = Telemetry::new(0);
        telemetry.observe(&state, &conf, budget, false);

        let mut action = FleetAction::new();
        action.bots[shooter as usize].special_action = SpecialAction::Battle { fire: true };
        let mut battle = [BattlePlanTelemetry::default(); BOTS_MAX];
        battle[shooter as usize].shot = Some(ShotPrediction {
            blast_point: Vec2::new(7.5, 4.0),
            predicted_new_damage_mask: 1,
        });
        telemetry.observe_action(
            &state,
            &conf,
            &action,
            HealPlanStats::default(),
            &battle,
            &[None; BOTS_MAX],
        );

        state.tick = 2;
        telemetry.observe(&state, &conf, budget, false);
        assert_eq!(telemetry.skipped_ticks, 1);
        assert_eq!(telemetry.fire_feedback_censored, 1);
        assert_eq!(telemetry.blast_error_censored, 1);
        assert_eq!(telemetry.damage_transition_censored, 1);

        telemetry.observe_action(
            &state,
            &conf,
            &action,
            HealPlanStats::default(),
            &battle,
            &[None; BOTS_MAX],
        );
        let summary = telemetry.summary();
        assert_eq!(summary["fire_feedback_pending"], 1);
        assert_eq!(summary["blast_error_pending"], 1);
        assert_eq!(summary["damage_transition_pending"], 1);
    }
}
