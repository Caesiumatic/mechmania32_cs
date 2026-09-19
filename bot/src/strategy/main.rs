use std::cell::RefCell;

use crate::core::*;

use super::controller::Controller;

/// Official entry point. Both engine slots deliberately construct the same controller;
/// the engine mirrors team B's state and actions into the team's local frame.
pub fn get_strategy(team: u8) -> Strategy {
    println!(
        "MM32_START {{\"strategy\":\"{}\",\"parameter_set\":\"{}\",\"learned_parameters\":{},\"team_slot\":{team}}}",
        super::policy::STRATEGY_ID,
        super::policy::PARAMETER_SET_ID,
        super::policy::LEARNED_PARAMETER_COUNT,
    );
    let controller = RefCell::new(Controller::new(team));
    Box::new(move |state| {
        controller
            .borrow_mut()
            .decide(state, get_config(), get_budget())
    })
}
