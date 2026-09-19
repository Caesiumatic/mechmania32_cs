use crate::core::*;

use super::policy::{TARGET_BATTLES, TARGET_EXTRACTORS, TARGET_HEALERS};

pub const OPENING_SIZE: u8 = TARGET_EXTRACTORS + TARGET_BATTLES + TARGET_HEALERS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FleetCounts {
    pub battle: u8,
    pub healer: u8,
    pub extractor: u8,
}

impl FleetCounts {
    pub fn from_state(state: &GameState) -> Self {
        let mut counts = Self::default();
        for bot in state.fleet_me.iter() {
            match bot.class() {
                BotClass::Battle => counts.battle += 1,
                BotClass::Healer => counts.healer += 1,
                BotClass::Extractor => counts.extractor += 1,
            }
        }
        counts
    }

    pub fn total(self) -> u8 {
        self.battle + self.healer + self.extractor
    }

    pub fn opening_incomplete(self) -> bool {
        self.extractor < TARGET_EXTRACTORS
            || self.battle < TARGET_BATTLES
            || self.healer < TARGET_HEALERS
            || self.total() < OPENING_SIZE
    }
}

/// Establish economy first, then the combat core, then healing. Once the 4/9/4 floor is
/// present, free replacements fill a lost floor before defaulting to Battle.
pub fn next_class(counts: FleetCounts) -> BotClass {
    if counts.extractor < TARGET_EXTRACTORS {
        BotClass::Extractor
    } else if counts.battle < TARGET_BATTLES {
        BotClass::Battle
    } else if counts.healer < TARGET_HEALERS {
        BotClass::Healer
    } else {
        BotClass::Battle
    }
}

pub fn production_order(state: &GameState, conf: &GameConfig) -> (BotClass, bool) {
    let counts = FleetCounts::from_state(state);
    let class = next_class(counts);

    // Rush runs before a natural build. Declining it when the timer is due takes the free
    // bot instead of spending tokens and sliding that free build to the next tick.
    let natural_due = state.fabricator_me.next_bot_creation <= state.tick;
    let rush = !state.in_endgame(conf)
        && !state.fleet_me.is_full()
        && counts.opening_incomplete()
        && !natural_due
        && state.fabricator_me.tokens >= conf.fabricator.rush_cost;

    (class, rush)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_floor_is_four_nine_four() {
        assert_eq!(OPENING_SIZE, 17);
        assert_eq!(next_class(FleetCounts::default()), BotClass::Extractor);
        assert_eq!(
            next_class(FleetCounts {
                extractor: 4,
                battle: 0,
                healer: 0
            }),
            BotClass::Battle,
        );
        assert_eq!(
            next_class(FleetCounts {
                extractor: 4,
                battle: 9,
                healer: 0
            }),
            BotClass::Healer,
        );
    }
}
