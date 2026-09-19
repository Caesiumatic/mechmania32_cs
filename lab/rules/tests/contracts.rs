//! M1 rule contracts for `mm-engine` revision
//! `4cbd3467e6617e330d42cb78ae3327ab2c88926a`.
//!
//! Every `rNN_...` test calls the public engine surface. `official_config` parses the first
//! configuration line emitted by the pinned official binary, preserved at
//! `lab/fixtures/default_config.json`. Tests that need an unobstructed lane clone it through
//! `isolated_config` and explicitly replace the map and/or deposit geometry; those are
//! synthetic fixtures, not claims about the shipped map.

use mm_engine::game::{
    action::{eval_tick, match_result, EndReason, MatchResult},
    config::{GameConfig, MapTile, MAP_SIZE},
    geom::{scan, ScanMask, ScanTarget},
    state::{
        BotClass, BotId, FleetAction, GameState, Mirror, MoveAction, SpecialAction, SpecialState,
        StateOption, TurnAction,
    },
    team::{Team, TEAMS},
    util::Vec2,
};

const ENGINE_REV: &str = "4cbd3467e6617e330d42cb78ae3327ab2c88926a";

fn official_config() -> GameConfig {
    // The dependency revision above is the code provenance lock. Deserialization also makes
    // fixture/schema drift fail loudly instead of silently changing a boundary expectation.
    let _provenance = ENGINE_REV;
    serde_json::from_str(include_str!("../../fixtures/default_config.json"))
        .expect("pinned official GameConfig fixture must match the engine schema")
}

fn isolated_config() -> GameConfig {
    let mut conf = official_config();
    conf.map = [[MapTile::Empty; MAP_SIZE]; MAP_SIZE];
    // Keep non-mining tests free of solid deposits.  Their positions are outside the arena,
    // while the payload is parked away from each test lane by `state_without_builds`.
    conf.deposit.pos = Vec2::new(-100.0, -100.0);
    conf.deposit.radius = 0.0;
    conf
}

fn state_without_builds(conf: &GameConfig) -> GameState {
    let mut state = GameState::new(conf);
    state.capture = -1.0;
    for team in TEAMS {
        state.fabricators_mut()[team].next_bot_creation = u32::MAX;
        state.fabricators_mut()[team].tokens = 0.0;
    }
    state
}

fn spawn(
    state: &mut GameState,
    conf: &GameConfig,
    team: Team,
    pos: Vec2,
    angle: f32,
    class: BotClass,
) -> BotId {
    let id = state.fleets_mut()[team].add();
    let mut fleets = state.fleets_mut();
    let bot = fleets[team]
        .get_mut(id)
        .expect("newly allocated bot slot disappeared");
    bot.id = id;
    bot.health = conf.bot.health;
    bot.pos = pos;
    bot.vel = Vec2::ZERO;
    bot.angle = angle;
    bot.turn_vel = 0.0;
    bot.invulnerable_until_tick = 0;
    bot.special = SpecialState::new(class);
    id
}

fn firing(ids: &[BotId]) -> FleetAction {
    let mut action = FleetAction::new();
    for &id in ids {
        action.bots[id as usize].special_action = SpecialAction::Battle { fire: true };
    }
    action
}

fn healing(assignments: &[(BotId, BotId)]) -> FleetAction {
    let mut action = FleetAction::new();
    for &(healer, target) in assignments {
        action.bots[healer as usize].special_action = SpecialAction::Healer { fire: true, target };
    }
    action
}

fn mining(ids: &[BotId]) -> FleetAction {
    let mut action = FleetAction::new();
    for &id in ids {
        action.bots[id as usize].special_action = SpecialAction::Extractor { mine: true };
    }
    action
}

fn approx(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.0e-4,
        "expected {expected}, got {actual}"
    );
}

fn assert_result(actual: Option<MatchResult>, winner: Option<Team>, reason: EndReason) {
    assert_eq!(actual, Some(MatchResult { winner, reason }));
}

#[test]
fn r01_initial_state_and_tick_zero_free_production() {
    let conf = official_config();
    for class in [BotClass::Battle, BotClass::Healer, BotClass::Extractor] {
        let mut state = GameState::new(&conf);
        assert_eq!(state.tick, 0);
        assert_eq!(state.fleets()[Team::A].len, 0);
        assert_eq!(state.fleets()[Team::B].len, 0);

        let mut action = FleetAction::new();
        action.fabricator_next = class.clone();
        eval_tick(&mut state, &conf, action.clone(), action);

        assert_eq!(state.tick, 1);
        for team in TEAMS {
            assert_eq!(state.fleets()[team].len, 1);
            assert_eq!(state.fleets()[team].iter().next().unwrap().class(), class);
            assert_eq!(
                state.fabricators()[team].tokens,
                conf.fabricator.starting_tokens
            );
            assert_eq!(
                state.fabricators()[team].next_bot_creation,
                conf.fabricator.interval
            );
        }
    }
}

#[test]
fn r02_rush_and_natural_build_share_one_tick_and_defer_natural() {
    let conf = isolated_config();
    let mut state = GameState::new(&conf);
    state.capture = -1.0;
    eval_tick(&mut state, &conf, FleetAction::new(), FleetAction::new());
    assert_eq!(state.tick, 1);

    let collision_tick = state.tick;
    state.fabricators_mut()[Team::A].tokens = conf.fabricator.rush_cost;
    state.fabricators_mut()[Team::A].next_bot_creation = collision_tick;
    state.fabricators_mut()[Team::B].next_bot_creation = u32::MAX;

    let mut rush = FleetAction::new();
    rush.rush_order = true;
    rush.fabricator_next = BotClass::Healer;
    eval_tick(&mut state, &conf, rush, FleetAction::new());

    assert_eq!(
        state.fleets()[Team::A].len,
        2,
        "at most one bot enters per tick"
    );
    assert_eq!(state.fleets()[Team::A][1].class(), BotClass::Healer);
    approx(state.fabricators()[Team::A].tokens, 0.0);
    assert_eq!(
        state.fabricators()[Team::A].next_bot_creation,
        collision_tick + 1
    );

    let mut natural = FleetAction::new();
    natural.fabricator_next = BotClass::Extractor;
    eval_tick(&mut state, &conf, natural, FleetAction::new());
    assert_eq!(state.fleets()[Team::A].len, 3);
    assert_eq!(state.fleets()[Team::A][2].class(), BotClass::Extractor);
    assert_eq!(
        state.fabricators()[Team::A].next_bot_creation,
        collision_tick + 1 + conf.fabricator.interval
    );
}

#[test]
fn r03_full_population_holds_due_build_until_a_slot_opens() {
    let conf = isolated_config();
    let mut state = state_without_builds(&conf);
    while !state.fleets()[Team::A].is_full() {
        spawn(
            &mut state,
            &conf,
            Team::A,
            Vec2::new(5.0, 5.0),
            0.0,
            BotClass::Battle,
        );
    }
    state.fabricators_mut()[Team::A].next_bot_creation = state.tick;

    eval_tick(&mut state, &conf, FleetAction::new(), FleetAction::new());
    assert!(state.fleets()[Team::A].is_full());
    assert_eq!(state.fabricators()[Team::A].next_bot_creation, 0);
    assert!(state.fabricators()[Team::A].next_bot_creation < state.tick);

    state.fleets_mut()[Team::A].remove(0);
    eval_tick(&mut state, &conf, FleetAction::new(), FleetAction::new());
    assert!(state.fleets()[Team::A].is_full());
    assert!(state.fleets()[Team::A].get(0).is_some());
    assert_eq!(
        state.fabricators()[Team::A].next_bot_creation,
        1 + conf.fabricator.interval
    );
}

#[test]
fn r04_new_bot_ignores_stale_action_in_its_slot() {
    let conf = official_config();
    let mut state = GameState::new(&conf);
    let mut action = FleetAction::new();
    action.fabricator_next = BotClass::Battle;
    action.bots[0].move_action = MoveAction {
        direction: Vec2::new(1.0, 0.0),
    };
    action.bots[0].special_action = SpecialAction::Battle { fire: true };
    action.bots[0].self_destruct = true;

    eval_tick(&mut state, &conf, action, FleetAction::new());

    let bot = state.fleets()[Team::A]
        .get(0)
        .expect("new bot consumed a stale self-destruct");
    assert_eq!(bot.vel, Vec2::ZERO);
    assert_eq!(bot.shot(), StateOption::None);
    approx(bot.pos.x, conf.bot.radius + 0.001);
    approx(bot.pos.y, MAP_SIZE as f32 - conf.bot.radius - 0.001);
}

#[test]
fn r05_healing_rejects_self_and_respects_range_and_facing() {
    let conf = isolated_config();

    let mut self_state = state_without_builds(&conf);
    let self_healer = spawn(
        &mut self_state,
        &conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Healer,
    );
    self_state.fleets_mut()[Team::A][self_healer].health = 5.0;
    eval_tick(
        &mut self_state,
        &conf,
        healing(&[(self_healer, self_healer)]),
        FleetAction::new(),
    );
    approx(self_state.fleets()[Team::A][self_healer].health, 5.0);
    assert_eq!(
        self_state.fleets()[Team::A][self_healer].healing(),
        StateOption::None
    );

    let mut valid = state_without_builds(&conf);
    let healer = spawn(
        &mut valid,
        &conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Healer,
    );
    let target = spawn(
        &mut valid,
        &conf,
        Team::A,
        Vec2::new(7.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    valid.fleets_mut()[Team::A][target].health = 5.0;
    eval_tick(
        &mut valid,
        &conf,
        healing(&[(healer, target)]),
        FleetAction::new(),
    );
    approx(
        valid.fleets()[Team::A][target].health,
        5.0 + conf.bot.heal_per_tick,
    );
    assert_eq!(
        valid.fleets()[Team::A][healer].healing(),
        StateOption::Some(target)
    );

    let mut outside_arc = state_without_builds(&conf);
    let healer = spawn(
        &mut outside_arc,
        &conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Healer,
    );
    let target_pos = Vec2::new(5.0, 16.0) + Vec2::from_angle_deg(60.0) * 2.0;
    let target = spawn(
        &mut outside_arc,
        &conf,
        Team::A,
        target_pos,
        0.0,
        BotClass::Battle,
    );
    outside_arc.fleets_mut()[Team::A][target].health = 5.0;
    eval_tick(
        &mut outside_arc,
        &conf,
        healing(&[(healer, target)]),
        FleetAction::new(),
    );
    approx(outside_arc.fleets()[Team::A][target].health, 5.0);
}

fn healing_result_at(
    conf: &GameConfig,
    distance: f32,
    angle_deg: f32,
) -> (f32, StateOption<BotId>) {
    let mut state = state_without_builds(conf);
    let origin = Vec2::new(16.0, 16.0);
    let healer = spawn(&mut state, conf, Team::A, origin, 0.0, BotClass::Healer);
    let target = spawn(
        &mut state,
        conf,
        Team::A,
        origin + Vec2::from_angle_deg(angle_deg) * distance,
        0.0,
        BotClass::Battle,
    );
    state.fleets_mut()[Team::A][target].health = 5.0;
    eval_tick(
        &mut state,
        conf,
        healing(&[(healer, target)]),
        FleetAction::new(),
    );
    (
        state.fleets()[Team::A][target].health,
        state.fleets()[Team::A][healer].healing(),
    )
}

#[test]
fn r05_range_boundary_includes_exactly_three_and_rejects_just_outside() {
    let conf = isolated_config();
    let (at_boundary, channel) = healing_result_at(&conf, 3.0, 0.0);
    approx(at_boundary, 5.0 + conf.bot.heal_per_tick);
    assert!(matches!(channel, StateOption::Some(_)));

    let (outside, channel) = healing_result_at(&conf, 3.0 + 1.0e-3, 0.0);
    approx(outside, 5.0);
    assert_eq!(channel, StateOption::None);
}

#[test]
fn r05_arc_boundary_includes_exactly_45_degrees_and_rejects_just_outside() {
    let conf = isolated_config();
    let (at_boundary, channel) = healing_result_at(&conf, 2.0, 45.0);
    approx(at_boundary, 5.0 + conf.bot.heal_per_tick);
    assert!(matches!(channel, StateOption::Some(_)));

    let (outside, channel) = healing_result_at(&conf, 2.0, 45.0 + 1.0e-3);
    approx(outside, 5.0);
    assert_eq!(channel, StateOption::None);
}

fn health_after_stacked_heal(conf: &GameConfig, count: usize) -> f32 {
    let mut state = state_without_builds(conf);
    let target = spawn(
        &mut state,
        conf,
        Team::A,
        Vec2::new(16.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    state.fleets_mut()[Team::A][target].health = 1.0;

    let mut assignments = Vec::new();
    for i in 0..count {
        let angle = i as f32 * 360.0 / count as f32;
        let id = spawn(
            &mut state,
            conf,
            Team::A,
            Vec2::new(16.0, 16.0) + Vec2::from_angle_deg(angle) * 2.0,
            angle + 180.0,
            BotClass::Healer,
        );
        assignments.push((id, target));
    }
    eval_tick(&mut state, conf, healing(&assignments), FleetAction::new());
    state.fleets()[Team::A][target].health
}

#[test]
fn r06_healing_caps_at_three_and_resolves_before_damage() {
    let conf = isolated_config();
    let three = health_after_stacked_heal(&conf, 3);
    let four = health_after_stacked_heal(&conf, 4);
    approx(three, 1.0 + 3.0 * conf.bot.heal_per_tick);
    approx(four, three);

    let mut state = state_without_builds(&conf);
    let healer = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Healer,
    );
    let target = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(7.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let enemy = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(10.0, 16.0),
        180.0,
        BotClass::Battle,
    );
    state.fleets_mut()[Team::A][target].health =
        conf.bot.blaster_damage - conf.bot.heal_per_tick / 2.0;

    eval_tick(
        &mut state,
        &conf,
        healing(&[(healer, target)]),
        firing(&[enemy]),
    );

    let survivor = state.fleets()[Team::A]
        .get(target)
        .expect("healing must land before lethal damage");
    approx(survivor.health, conf.bot.heal_per_tick / 2.0);
}

#[test]
fn r07_damage_protection_and_weapon_cooldown_boundaries() {
    let conf = isolated_config();
    let mut state = state_without_builds(&conf);
    let shooters = [2.0, 3.0, 4.0, 5.0].map(|x| {
        spawn(
            &mut state,
            &conf,
            Team::A,
            Vec2::new(x, 16.0),
            0.0,
            BotClass::Battle,
        )
    });
    let target = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(10.0, 16.0),
        180.0,
        BotClass::Battle,
    );

    eval_tick(
        &mut state,
        &conf,
        firing(&shooters[..2]),
        FleetAction::new(),
    );
    approx(
        state.fleets()[Team::B][target].health,
        conf.bot.health - conf.bot.blaster_damage,
    );
    assert_eq!(
        state.fleets()[Team::B][target].invulnerable_until_tick,
        conf.bot.base_invulnerability_ticks
    );
    for &id in &shooters[..2] {
        assert_eq!(
            state.fleets()[Team::A][id].next_fire_tick(),
            conf.bot.blaster_cooldown
        );
    }

    state.tick = conf.bot.base_invulnerability_ticks - 1;
    eval_tick(
        &mut state,
        &conf,
        firing(&[shooters[2]]),
        FleetAction::new(),
    );
    approx(
        state.fleets()[Team::B][target].health,
        conf.bot.health - conf.bot.blaster_damage,
    );

    state.tick = conf.bot.base_invulnerability_ticks;
    eval_tick(
        &mut state,
        &conf,
        firing(&[shooters[3]]),
        FleetAction::new(),
    );
    approx(
        state.fleets()[Team::B][target].health,
        conf.bot.health - 2.0 * conf.bot.blaster_damage,
    );

    state.tick = conf.bot.blaster_cooldown - 1;
    eval_tick(
        &mut state,
        &conf,
        firing(&[shooters[0]]),
        FleetAction::new(),
    );
    assert_eq!(
        state.fleets()[Team::A][shooters[0]].shot(),
        StateOption::None
    );

    state.tick = conf.bot.blaster_cooldown;
    eval_tick(
        &mut state,
        &conf,
        firing(&[shooters[0]]),
        FleetAction::new(),
    );
    assert!(state.fleets()[Team::A][shooters[0]]
        .shot()
        .option()
        .is_some());
    assert_eq!(
        state.fleets()[Team::A][shooters[0]].next_fire_tick(),
        2 * conf.bot.blaster_cooldown
    );
    approx(
        state.fleets()[Team::B][target].health,
        conf.bot.health - 3.0 * conf.bot.blaster_damage,
    );
}

#[test]
fn r08_same_tick_mutual_kill_allows_both_shots() {
    let conf = isolated_config();
    let mut state = state_without_builds(&conf);
    let a = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let b = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(10.0, 16.0),
        180.0,
        BotClass::Battle,
    );
    state.fleets_mut()[Team::A][a].health = conf.bot.blaster_damage;
    state.fleets_mut()[Team::B][b].health = conf.bot.blaster_damage;

    eval_tick(&mut state, &conf, firing(&[a]), firing(&[b]));

    assert_eq!(state.fleets()[Team::A].len, 0);
    assert_eq!(state.fleets()[Team::B].len, 0);
}

fn endpoint_target_health(conf: &GameConfig, center_distance: f32) -> f32 {
    let mut state = state_without_builds(conf);
    let shooter = spawn(
        &mut state,
        conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let target = spawn(
        &mut state,
        conf,
        Team::B,
        Vec2::new(5.0 + center_distance, 16.0),
        180.0,
        BotClass::Battle,
    );
    eval_tick(&mut state, conf, firing(&[shooter]), FleetAction::new());
    state.fleets()[Team::B][target].health
}

#[test]
fn r09_max_range_endpoint_creates_splash_with_10_55_center_limit() {
    let conf = isolated_config();
    let effective = conf.bot.blaster_range + conf.bot.radius + conf.bot.base_blaster_splash_radius;

    approx(
        endpoint_target_health(&conf, conf.bot.blaster_range + 0.50),
        conf.bot.health - conf.bot.blaster_damage,
    );
    approx(
        endpoint_target_health(&conf, effective + 0.01),
        conf.bot.health,
    );
    approx(effective, 10.55);
}

#[test]
fn r10_first_payload_deposit_or_enemy_sets_blast_and_only_enemies_take_splash() {
    let mut conf = isolated_config();

    // Payload is the first public geometry hit and the recorded blast point.
    let mut payload_state = state_without_builds(&conf);
    payload_state.capture = 0.0;
    let payload_shooter = spawn(
        &mut payload_state,
        &conf,
        Team::A,
        Vec2::new(10.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let hit = scan(
        &payload_state,
        &conf,
        Vec2::new(10.0, 16.0),
        Vec2::new(1.0, 0.0),
        ScanMask::PAYLOAD | ScanMask::DEPOSITS | ScanMask::bots(Team::B),
        None,
        conf.bot.blaster_range,
    )
    .expect("payload should stop the ray");
    assert_eq!(hit.target, ScanTarget::Payload);
    eval_tick(
        &mut payload_state,
        &conf,
        firing(&[payload_shooter]),
        FleetAction::new(),
    );
    let shot = payload_state.fleets()[Team::A][payload_shooter]
        .shot()
        .option()
        .unwrap();
    approx(shot.x, 16.0 - conf.payload.radius);

    // Move the real-size deposit into an empty synthetic lane and check it wins the scan.
    conf.deposit.pos = Vec2::new(20.0, 16.0);
    conf.deposit.radius = 0.5;
    let mut deposit_state = state_without_builds(&conf);
    let deposit_shooter = spawn(
        &mut deposit_state,
        &conf,
        Team::A,
        Vec2::new(15.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let hit = scan(
        &deposit_state,
        &conf,
        Vec2::new(15.0, 16.0),
        Vec2::new(1.0, 0.0),
        ScanMask::DEPOSITS,
        None,
        conf.bot.blaster_range,
    )
    .expect("deposit should stop the ray");
    assert_eq!(hit.target, ScanTarget::Deposit { team: Team::A });
    eval_tick(
        &mut deposit_state,
        &conf,
        firing(&[deposit_shooter]),
        FleetAction::new(),
    );
    let shot = deposit_state.fleets()[Team::A][deposit_shooter]
        .shot()
        .option()
        .unwrap();
    approx(shot.x, conf.deposit.pos.x - conf.deposit.radius);

    // An enemy is the first hit in this lane.  Overlapping enemies share the blast while
    // an overlapping ally neither blocks the shot nor takes friendly splash.
    let conf = isolated_config();
    let mut group_state = state_without_builds(&conf);
    let shooter = spawn(
        &mut group_state,
        &conf,
        Team::A,
        Vec2::new(5.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let first = spawn(
        &mut group_state,
        &conf,
        Team::B,
        Vec2::new(10.0, 16.0),
        180.0,
        BotClass::Battle,
    );
    let second = spawn(
        &mut group_state,
        &conf,
        Team::B,
        Vec2::new(10.1, 16.0),
        180.0,
        BotClass::Battle,
    );
    let ally = spawn(
        &mut group_state,
        &conf,
        Team::A,
        Vec2::new(10.1, 16.0),
        0.0,
        BotClass::Battle,
    );
    let hit = scan(
        &group_state,
        &conf,
        Vec2::new(5.0, 16.0),
        Vec2::new(1.0, 0.0),
        ScanMask::bots(Team::B),
        None,
        conf.bot.blaster_range,
    )
    .unwrap();
    assert_eq!(
        hit.target,
        ScanTarget::Bot {
            team: Team::B,
            id: first
        }
    );
    eval_tick(
        &mut group_state,
        &conf,
        firing(&[shooter]),
        FleetAction::new(),
    );
    approx(
        group_state.fleets()[Team::B][first].health,
        conf.bot.health - conf.bot.blaster_damage,
    );
    approx(
        group_state.fleets()[Team::B][second].health,
        conf.bot.health - conf.bot.blaster_damage,
    );
    approx(group_state.fleets()[Team::A][ally].health, conf.bot.health);
}

fn capture_after_push(conf: &GameConfig, a_count: usize, b_count: usize) -> f32 {
    let mut state = GameState::new(conf);
    state.capture = 0.0;
    for team in TEAMS {
        state.fabricators_mut()[team].next_bot_creation = u32::MAX;
    }
    let positions = [
        Vec2::new(18.0, 16.0),
        Vec2::new(16.0, 18.0),
        Vec2::new(14.0, 16.0),
        Vec2::new(16.0, 14.0),
    ];
    for &pos in positions.iter().take(a_count) {
        spawn(&mut state, conf, Team::A, pos, 0.0, BotClass::Battle);
    }
    for &pos in positions.iter().rev().take(b_count) {
        spawn(&mut state, conf, Team::B, pos, 180.0, BotClass::Battle);
    }
    eval_tick(&mut state, conf, FleetAction::new(), FleetAction::new());
    state.capture
}

#[test]
fn r13_payload_one_or_many_push_at_fixed_speed_and_opposition_contests() {
    let mut conf = isolated_config();
    conf.deposit.radius = 0.0;
    let one = capture_after_push(&conf, 1, 0);
    let three = capture_after_push(&conf, 3, 0);
    assert!(one > 0.0);
    approx(three, one);
    approx(capture_after_push(&conf, 1, 1), 0.0);
    approx(capture_after_push(&conf, 3, 1), 0.0);
}

#[test]
fn r16_shared_mining_cap_preserves_incumbents_and_admits_only_free_slots() {
    let mut conf = isolated_config();
    conf.deposit.pos = Vec2::new(20.0, 16.0);
    conf.deposit.radius = 0.5;
    conf.deposit.extractor_cap = 8;
    let mut state = state_without_builds(&conf);

    let incumbents: Vec<BotId> = (0..8)
        .map(|i| {
            spawn(
                &mut state,
                &conf,
                Team::A,
                Vec2::new(17.0, 15.9 + i as f32 * 0.02),
                0.0,
                BotClass::Extractor,
            )
        })
        .collect();
    let incumbent_mask = incumbents.iter().fold(0u32, |mask, id| mask | (1u32 << id));

    eval_tick(&mut state, &conf, mining(&incumbents), FleetAction::new());
    assert_eq!(
        state.deposits()[Team::A].extractors[Team::A],
        incumbent_mask
    );
    approx(
        state.fabricators()[Team::A].tokens,
        conf.bot.extract_rate * conf.deposit.extractor_cap as f32,
    );

    let newcomer_a = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(17.0, 16.2),
        0.0,
        BotClass::Extractor,
    );
    let newcomer_b = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(23.0, 16.0),
        180.0,
        BotClass::Extractor,
    );
    let all_a: Vec<_> = incumbents.iter().copied().chain([newcomer_a]).collect();
    eval_tick(&mut state, &conf, mining(&all_a), mining(&[newcomer_b]));
    let slots = state.deposits()[Team::A].extractors;
    assert_eq!(slots[Team::A], incumbent_mask);
    assert_eq!(slots[Team::B], 0);
    assert_eq!(slots[Team::A].count_ones() + slots[Team::B].count_ones(), 8);

    let before_income = state.fabricators()[Team::A].tokens + state.fabricators()[Team::B].tokens;
    let remaining: Vec<_> = incumbents[1..]
        .iter()
        .copied()
        .chain([newcomer_a])
        .collect();
    eval_tick(&mut state, &conf, mining(&remaining), mining(&[newcomer_b]));
    let slots = state.deposits()[Team::A].extractors;
    let remaining_mask = incumbent_mask & !(1u32 << incumbents[0]);
    assert_eq!(slots[Team::A] & remaining_mask, remaining_mask);
    assert_eq!(slots[Team::A].count_ones() + slots[Team::B].count_ones(), 8);
    let newcomer_count =
        ((slots[Team::A] >> newcomer_a) & 1) + ((slots[Team::B] >> newcomer_b) & 1);
    assert_eq!(
        newcomer_count, 1,
        "exact winner may vary, but only one slot was free"
    );
    let after_income = state.fabricators()[Team::A].tokens + state.fabricators()[Team::B].tokens;
    approx(
        after_income - before_income,
        conf.bot.extract_rate * conf.deposit.extractor_cap as f32,
    );
}

#[test]
fn r17_blast_death_plus_self_destruct_removes_slot_once() {
    let conf = isolated_config();
    let mut state = state_without_builds(&conf);
    let doomed = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(10.0, 16.0),
        0.0,
        BotClass::Battle,
    );
    let shooter = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(15.0, 16.0),
        180.0,
        BotClass::Battle,
    );
    state.fleets_mut()[Team::A][doomed].health = conf.bot.blaster_damage;
    let mut a = FleetAction::new();
    a.bots[doomed as usize].self_destruct = true;

    // The regression contract is also "does not panic"; reaching the assertions proves it.
    eval_tick(&mut state, &conf, a, firing(&[shooter]));
    assert_eq!(state.fleets()[Team::A].len, 0);
    assert_eq!(state.fleets()[Team::B].len, 1);
}

#[test]
fn r18_tick_5999_builds_then_6000_enters_endgame_and_stops_building() {
    let conf = isolated_config();
    let endgame_start = conf.max_ticks - conf.endgame_ticks;
    assert_eq!(endgame_start, 6_000);

    let mut boundary = GameState::new(&conf);
    boundary.capture = 0.0;
    boundary.tick = endgame_start - 1;
    boundary.fabricators_mut()[Team::A].next_bot_creation = boundary.tick;
    boundary.fabricators_mut()[Team::B].next_bot_creation = u32::MAX;
    eval_tick(&mut boundary, &conf, FleetAction::new(), FleetAction::new());
    assert_eq!(boundary.tick, endgame_start);
    assert_eq!(
        boundary.fleets()[Team::A].len,
        1,
        "tick 5999 still fabricates"
    );
    assert_eq!(boundary.fleets()[Team::B].len, 0);
    assert_result(
        match_result(&boundary, &conf),
        Some(Team::A),
        EndReason::Elimination,
    );

    let mut stopped = GameState::new(&conf);
    stopped.capture = 0.0;
    stopped.tick = endgame_start;
    stopped.fabricators_mut()[Team::A].tokens = conf.fabricator.rush_cost;
    stopped.fabricators_mut()[Team::A].next_bot_creation = stopped.tick;
    let mut rush = FleetAction::new();
    rush.rush_order = true;
    eval_tick(&mut stopped, &conf, rush, FleetAction::new());
    assert_eq!(stopped.fleets()[Team::A].len, 0);
    approx(
        stopped.fabricators()[Team::A].tokens,
        conf.fabricator.rush_cost,
    );
}

#[test]
fn r19_tiebreak_order_is_progress_then_health_then_tokens_with_near_zero_sign() {
    let conf = isolated_config();
    let mut state = GameState::new(&conf);
    state.tick = conf.max_ticks;
    let a = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(5.0, 5.0),
        0.0,
        BotClass::Battle,
    );
    let b = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(27.0, 27.0),
        180.0,
        BotClass::Battle,
    );

    state.capture = f32::EPSILON;
    state.fleets_mut()[Team::A][a].health = 1.0;
    state.fabricators_mut()[Team::B].tokens = 10_000.0;
    assert_result(
        match_result(&state, &conf),
        Some(Team::A),
        EndReason::PayloadProgress,
    );
    state.capture = -f32::EPSILON;
    assert_result(
        match_result(&state, &conf),
        Some(Team::B),
        EndReason::PayloadProgress,
    );

    state.capture = 0.0;
    state.fleets_mut()[Team::A][a].health = conf.bot.health - 1.0;
    state.fleets_mut()[Team::B][b].health = conf.bot.health;
    state.fabricators_mut()[Team::A].tokens = 20_000.0;
    assert_result(
        match_result(&state, &conf),
        Some(Team::B),
        EndReason::HealthPool,
    );

    state.fleets_mut()[Team::A][a].health = conf.bot.health;
    state.fabricators_mut()[Team::A].tokens = 20_000.0;
    state.fabricators_mut()[Team::B].tokens = 10_000.0;
    assert_result(
        match_result(&state, &conf),
        Some(Team::A),
        EndReason::Tokens,
    );

    state.fabricators_mut()[Team::A].tokens = 10_000.0;
    assert_result(match_result(&state, &conf), None, EndReason::Tie);
}

#[test]
fn r20_state_and_action_mirror_round_trip_flips_progress_positions_and_labels() {
    let conf = official_config();
    let mut state = GameState::new(&conf);
    state.capture = 0.25;
    let extractor = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(5.0, 6.0),
        30.0,
        BotClass::Extractor,
    );
    let healer = spawn(
        &mut state,
        &conf,
        Team::B,
        Vec2::new(20.0, 21.0),
        210.0,
        BotClass::Healer,
    );
    state.fleets_mut()[Team::A][extractor].special = SpecialState::Extractor {
        extracting: StateOption::Some(Team::A),
    };
    state.deposits_mut()[Team::A].extractors[Team::A] = 1u32 << extractor;
    state.fabricators_mut()[Team::A].tokens = 111.0;
    state.fabricators_mut()[Team::B].tokens = 222.0;

    let original = state.clone();
    state.mirror(&conf);
    approx(state.capture, -0.25);
    let mirrored_extractor = &state.fleets()[Team::B][extractor];
    approx(mirrored_extractor.pos.x, MAP_SIZE as f32 - 5.0);
    approx(mirrored_extractor.pos.y, MAP_SIZE as f32 - 6.0);
    assert_eq!(mirrored_extractor.extracting(), StateOption::Some(Team::B));
    assert_eq!(
        state.deposits()[Team::B].extractors[Team::B],
        1u32 << extractor
    );
    assert_eq!(state.fleets()[Team::A][healer].class(), BotClass::Healer);
    approx(state.fabricators()[Team::A].tokens, 222.0);
    approx(state.fabricators()[Team::B].tokens, 111.0);
    state.mirror(&conf);
    assert!(
        state == original,
        "mirroring state twice must be the identity"
    );

    let mut action = FleetAction::new();
    action.fabricator_next = BotClass::Extractor;
    action.rush_order = true;
    action.bots[0].move_action.direction = Vec2::new(0.25, -0.75);
    action.bots[0].turn_action = TurnAction::TargetPosition {
        pos: Vec2::new(7.0, 9.0),
    };
    action.bots[0].special_action = SpecialAction::Healer {
        fire: true,
        target: 3,
    };
    let original_action = action.clone();
    action.mirror(&conf);
    assert_eq!(action.bots[0].move_action.direction, Vec2::new(-0.25, 0.75));
    match action.bots[0].turn_action {
        TurnAction::TargetPosition { pos } => {
            approx(pos.x, MAP_SIZE as f32 - 7.0);
            approx(pos.y, MAP_SIZE as f32 - 9.0);
        }
        ref other => panic!("unexpected mirrored turn action: {other:?}"),
    }
    assert_eq!(
        action.bots[0].special_action,
        SpecialAction::Healer {
            fire: true,
            target: 3
        }
    );
    action.mirror(&conf);
    assert_eq!(action, original_action);
}

#[test]
fn r24_endgame_still_credits_mining_code_doc_open_contract() {
    // CODE at 4cbd3467: fabrication stops in endgame, but extraction still runs later in
    // `eval_tick`.  DOC reportedly says endgame income stops; that discrepancy remains OPEN
    // for organizer confirmation, so this contract deliberately pins executable behavior.
    let mut conf = isolated_config();
    conf.deposit.pos = Vec2::new(20.0, 16.0);
    conf.deposit.radius = 0.5;
    let mut state = state_without_builds(&conf);
    state.tick = conf.max_ticks - conf.endgame_ticks;
    assert!(state.in_endgame(&conf));
    let miner = spawn(
        &mut state,
        &conf,
        Team::A,
        Vec2::new(17.0, 16.0),
        0.0,
        BotClass::Extractor,
    );
    state.fabricators_mut()[Team::A].tokens = 0.0;
    state.fabricators_mut()[Team::A].next_bot_creation = state.tick;

    eval_tick(&mut state, &conf, mining(&[miner]), FleetAction::new());

    approx(state.fabricators()[Team::A].tokens, conf.bot.extract_rate);
    assert_eq!(
        state.fleets()[Team::A][miner].extracting(),
        StateOption::Some(Team::A)
    );
    assert_eq!(state.fleets()[Team::A].len, 1, "endgame did not fabricate");
}
