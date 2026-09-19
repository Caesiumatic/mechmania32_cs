//! Behaviour contracts for the frozen M02 challenge opponents.
//!
//! The production opponent files remain standalone `strategy/main.rs` replacements. This
//! small client-feature crate supplies only the official starterpack surface so their real
//! implementations can be exercised without importing any candidate strategy code.

pub mod core {
    pub use mm_engine::game::config::*;
    pub use mm_engine::game::state::*;
    pub use mm_engine::game::team::*;
    pub use mm_engine::game::topology::{
        corridor_clear, disc_free, has_line_of_sight, init_topology, navigate_to, path_length,
        point_free, point_seg_dist, route_waypoints, topology, MapTopology,
    };
    pub use mm_engine::game::util::*;
    pub use mm_engine::ipc;
    pub use mm_engine::ipc::get_config;

    pub type Strategy = Box<dyn Fn(&GameState) -> FleetAction>;

    #[inline(always)]
    pub fn move_bot(vel: Vec2) -> MoveAction {
        MoveAction { direction: vel }
    }

    #[inline(always)]
    pub fn turn_towards(target_position: Vec2) -> TurnAction {
        TurnAction::TargetPosition {
            pos: target_position,
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../strafe_pressure/main.rs"]
mod strafe_pressure;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../support_screen/main.rs"]
mod support_screen;

#[cfg(test)]
mod tests {
    use super::core::*;
    use super::{strafe_pressure, support_screen};
    use std::sync::OnceLock;

    fn conf() -> &'static GameConfig {
        static CONF: OnceLock<GameConfig> = OnceLock::new();
        let conf = CONF.get_or_init(|| {
            serde_json::from_str(include_str!("../../fixtures/default_config.json"))
                .expect("pinned GameConfig fixture must match the client schema")
        });
        init_topology(&conf.map, conf.bot.radius);
        conf
    }

    fn state() -> GameState {
        GameState::new(conf())
    }

    fn add_bot(fleet: &mut BotArray, class: BotClass, pos: Vec2, angle: f32, health: f32) -> BotId {
        let id = fleet.add();
        let bot = &mut fleet[id];
        bot.health = health;
        bot.pos = pos;
        bot.vel = Vec2::ZERO;
        bot.angle = angle;
        bot.turn_vel = 0.0;
        bot.invulnerable_until_tick = 0;
        bot.special = SpecialState::new(class);
        id
    }

    fn actual_delta(direction: Vec2, speed: f32) -> Vec2 {
        let direction = if direction.norm_sq() > 1.0 {
            direction.normalize_or_zero()
        } else {
            direction
        };
        direction * speed
    }

    #[test]
    fn support_screen_full_health_healer_keeps_following_a_battle_anchor() {
        let conf = conf();
        let mut state = state();
        let battle = add_bot(
            &mut state.fleet_me,
            BotClass::Battle,
            Vec2::new(20.0, 20.0),
            0.0,
            conf.bot.health,
        );
        let healer = add_bot(
            &mut state.fleet_me,
            BotClass::Healer,
            Vec2::new(10.0, 10.0),
            0.0,
            conf.bot.health,
        );

        let action = support_screen::action_for_test(&state, conf);
        let order = &action.bots[healer as usize];
        assert!(order.move_action.direction.norm_sq() > 0.0);
        assert_eq!(
            order.turn_action,
            TurnAction::TargetPosition {
                pos: state.fleet_me[battle].pos
            }
        );
        assert_eq!(
            order.special_action,
            SpecialAction::Healer {
                fire: false,
                target: healer
            }
        );
    }

    #[test]
    fn support_screen_caps_current_healing_at_three_channels() {
        let conf = conf();
        let mut state = state();
        let target = add_bot(
            &mut state.fleet_me,
            BotClass::Battle,
            Vec2::new(20.0, 20.0),
            0.0,
            conf.bot.health - 5.0,
        );
        let healer_specs = [
            (Vec2::new(17.6, 20.0), 0.0),
            (Vec2::new(18.0, 20.0), 0.0),
            (Vec2::new(22.0, 20.0), 180.0),
            (Vec2::new(22.4, 20.0), 180.0),
        ];
        let healer_ids: Vec<_> = healer_specs
            .into_iter()
            .map(|(pos, angle)| {
                add_bot(
                    &mut state.fleet_me,
                    BotClass::Healer,
                    pos,
                    angle,
                    conf.bot.health,
                )
            })
            .collect();

        let action = support_screen::action_for_test(&state, conf);
        let channels = healer_ids
            .iter()
            .filter(|id| {
                matches!(
                    action.bots[**id as usize].special_action,
                    SpecialAction::Healer { fire: true, target: got } if got == target
                )
            })
            .count();
        assert_eq!(channels, conf.bot.heal_stack_cap as usize);
    }

    #[test]
    fn support_screen_assigns_distinct_payload_approaches() {
        let conf = conf();
        let mut state = state();
        let ids: Vec<_> = (0..3)
            .map(|_| {
                add_bot(
                    &mut state.fleet_me,
                    BotClass::Battle,
                    Vec2::new(16.5, 14.5),
                    0.0,
                    conf.bot.health,
                )
            })
            .collect();

        let action = support_screen::action_for_test(&state, conf);
        let directions: Vec<_> = ids
            .iter()
            .map(|id| action.bots[*id as usize].move_action.direction)
            .collect();
        assert!(directions.iter().all(|direction| direction.norm_sq() > 0.0));
        assert!(directions.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn strafe_pressure_uses_opposite_tangents_and_distance_correction() {
        let conf = conf();
        let mut state = state();
        let even = add_bot(
            &mut state.fleet_me,
            BotClass::Battle,
            Vec2::new(10.5, 10.5),
            0.0,
            conf.bot.health,
        );
        let odd = add_bot(
            &mut state.fleet_me,
            BotClass::Battle,
            Vec2::new(10.5, 10.5),
            0.0,
            conf.bot.health,
        );
        add_bot(
            &mut state.fleet_other,
            BotClass::Battle,
            Vec2::new(14.5, 10.5),
            180.0,
            conf.bot.health,
        );

        let action = strafe_pressure::action_for_test(&state, conf);
        let radial = Vec2::new(1.0, 0.0);
        let even_move = action.bots[even as usize].move_action.direction;
        let odd_move = action.bots[odd as usize].move_action.direction;
        assert!(even_move.dot(radial) < 0.0, "near bots must open distance");
        assert!(odd_move.dot(radial) < 0.0, "near bots must open distance");
        assert!(even_move.y > 0.0 && odd_move.y < 0.0);
    }

    #[test]
    fn strafe_pressure_only_requests_an_aligned_observable_shot() {
        let conf = conf();
        let mut aligned = state();
        let shooter = add_bot(
            &mut aligned.fleet_me,
            BotClass::Battle,
            Vec2::new(10.5, 10.5),
            0.0,
            conf.bot.health,
        );
        add_bot(
            &mut aligned.fleet_other,
            BotClass::Battle,
            Vec2::new(14.5, 10.5),
            180.0,
            conf.bot.health,
        );
        let aligned_action = strafe_pressure::action_for_test(&aligned, conf);
        let movement = aligned_action.bots[shooter as usize].move_action.direction;
        let origin = aligned.fleet_me[shooter].pos + actual_delta(movement, conf.bot.speed);
        let target = aligned.fleet_other.iter().next().unwrap().pos;
        let desired = (target - origin).angle_deg();
        let heading = normalize_degrees(
            aligned.fleet_me[shooter].angle
                + diff_degrees(desired, aligned.fleet_me[shooter].angle)
                    .clamp(-conf.bot.turn_speed, conf.bot.turn_speed),
        );
        let direction = Vec2::from_angle_deg(heading);
        let relative = target - origin;
        let along = relative.dot(direction).clamp(0.0, conf.bot.blaster_range);
        let closest = origin + direction * along;
        assert!(matches!(
            aligned_action.bots[shooter as usize].special_action,
            SpecialAction::Battle { fire: true }
        ), "aligned order was {:?}; origin={origin:?} target={target:?} desired={desired} heading={heading} los={} closest={closest:?} dist={}", aligned_action.bots[shooter as usize], has_line_of_sight(conf, origin, target), closest.dist(&target));

        aligned.fleet_me[shooter].angle = 180.0;
        let turning_action = strafe_pressure::action_for_test(&aligned, conf);
        assert!(matches!(
            turning_action.bots[shooter as usize].special_action,
            SpecialAction::Battle { fire: false }
        ));

        let move_delta = actual_delta(
            aligned_action.bots[shooter as usize].move_action.direction,
            conf.bot.speed,
        );
        assert!((move_delta.norm() - conf.bot.speed).abs() < 1e-5);
    }
}
