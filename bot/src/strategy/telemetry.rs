use crate::core::*;

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
    fire_orders: u64,
    heal_orders: u64,
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
            fire_orders: 0,
            heal_orders: 0,
        }
    }

    pub fn observe(&mut self, state: &GameState, conf: &GameConfig, budget: Budget, low: bool) {
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
                }),
            );
        }
    }

    pub fn observe_action(&mut self, action: &FleetAction) {
        for bot in &action.bots {
            match bot.special_action {
                SpecialAction::Battle { fire: true } => self.fire_orders += 1,
                SpecialAction::Healer { fire: true, .. } => self.heal_orders += 1,
                _ => {}
            }
        }
    }

    fn percentile(sorted: &[u64], numerator: usize) -> Option<u64> {
        (!sorted.is_empty()).then(|| sorted[(sorted.len() - 1) * numerator / 100])
    }

    fn summary(&self) -> serde_json::Value {
        let mut sorted = self.observed_charges.clone();
        sorted.sort_unstable();
        let sum: u128 = sorted.iter().map(|&v| v as u128).sum();
        let mean = (!sorted.is_empty()).then(|| sum as f64 / sorted.len() as f64);
        serde_json::json!({
            "team_slot": self.team_slot,
            "first_tick": self.first_tick,
            "last_tick": self.last_tick,
            "calls": self.calls,
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
            "fire_orders": self.fire_orders,
            "heal_orders": self.heal_orders,
        })
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        println!("MM32_SUMMARY {}", self.summary());
    }
}
