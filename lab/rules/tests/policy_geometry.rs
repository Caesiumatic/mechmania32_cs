//! Production ballistics parity tests against pinned `mm-engine` 4cbd3467.
//!
//! The production file is imported directly.  This test-only `core` bridge
//! supplies the same lightweight types under the engine feature, without ever
//! linking engine-only APIs into the submitted bot crate.

mod core {
    pub use mm_engine::game::{config::*, state::BotId, util::*};
}

#[path = "../../../bot/src/strategy/ballistics.rs"]
mod ballistics;

use ballistics::{
    marginal_damage_mask, trace_shot, Circle, EnemyGeometry, FirstHit, ShotResult, ShotScene,
};
use mm_engine::game::{
    action::eval_tick,
    config::{GameConfig, MapTile, MAP_SIZE},
    geom::{scan, ScanMask, ScanTarget},
    state::{BotClass, BotId, FleetAction, GameState, SpecialAction, SpecialState},
    team::Team,
    topology::has_line_of_sight,
    util::{normalize_degrees, Vec2},
};

const ENGINE_REV: &str = "4cbd3467e6617e330d42cb78ae3327ab2c88926a";

fn official_config() -> GameConfig {
    let _provenance = ENGINE_REV;
    serde_json::from_str(include_str!("../../fixtures/default_config.json"))
        .expect("pinned official GameConfig fixture must match the engine schema")
}

fn open_config() -> GameConfig {
    let mut conf = official_config();
    conf.map = [[MapTile::Empty; MAP_SIZE]; MAP_SIZE];
    conf.deposit.pos = Vec2::new(-100.0, -100.0);
    conf.deposit.radius = 0.0;
    conf
}

fn empty_scene<'a>(enemies: &'a [EnemyGeometry]) -> ShotScene<'a> {
    ShotScene {
        enemies,
        payload: Circle {
            centre: Vec2::new(-100.0, -100.0),
            radius: 0.0,
        },
        deposits: [
            Circle {
                centre: Vec2::new(-100.0, -100.0),
                radius: 0.0,
            },
            Circle {
                centre: Vec2::new(-101.0, -101.0),
                radius: 0.0,
            },
        ],
    }
}

fn approx_point(actual: Vec2, expected: Vec2) {
    assert!(
        actual.dist(&expected) <= 1.0e-5,
        "point mismatch: actual={actual:?}, expected={expected:?}"
    );
}

/// Exact extraction of the d13c236 v0 distance/perpendicular gate.  Its LOS
/// predicates are intentionally outside this adapter: D01/D02 fail before an
/// obstacle is involved, while D03-D05 demonstrate information it never had.
fn legacy_v0_geometric_gate(conf: &GameConfig, to_enemy: Vec2, actual_direction: Vec2) -> bool {
    let distance = to_enemy.norm();
    if distance <= 1.0e-5 || distance > conf.bot.blaster_range + conf.bot.radius {
        return false;
    }
    let direction = actual_direction.normalize_or_zero();
    let along = to_enemy.dot(direction);
    let perpendicular_sq = (to_enemy.norm_sq() - along * along).max(0.0);
    let hit_radius = conf.bot.radius + conf.bot.base_blaster_splash_radius;
    along >= 0.0 && along <= conf.bot.blaster_range && perpendicular_sq <= hit_radius * hit_radius
}

#[test]
fn d01_old_splash_tube_false_positive_is_rejected_by_real_blast_point() {
    let conf = open_config();
    let origin = Vec2::new(2.0, 2.0);
    let enemy = EnemyGeometry {
        id: 0,
        centre: origin + Vec2::new(5.0, 0.4),
        damageable: true,
    };
    assert!(legacy_v0_geometric_gate(
        &conf,
        enemy.centre - origin,
        Vec2::new(1.0, 0.0)
    ));

    let result = trace_shot(&conf, &empty_scene(&[enemy]), origin, Vec2::new(1.0, 0.0));
    approx_point(result.blast_point, Vec2::new(12.0, 2.0));
    assert_eq!(result.first_hit, None);
    assert_eq!(result.physical_mask, 0);
    assert_eq!(result.damageable_mask, 0);
}

#[test]
fn d02_direct_hull_and_endpoint_splash_boundaries() {
    let conf = open_config();
    let origin = Vec2::new(2.0, 2.0);

    let direct = EnemyGeometry {
        id: 0,
        centre: origin + Vec2::new(10.2, 0.0),
        damageable: true,
    };
    let direct_result = trace_shot(&conf, &empty_scene(&[direct]), origin, Vec2::new(1.0, 0.0));
    assert_eq!(direct_result.first_hit, Some(FirstHit::Enemy(0)));
    approx_point(direct_result.blast_point, origin + Vec2::new(9.95, 0.0));
    assert_eq!(direct_result.damageable_mask, 1);
    assert!(!legacy_v0_geometric_gate(
        &conf,
        direct.centre - origin,
        Vec2::new(1.0, 0.0)
    ));

    let endpoint_hit = EnemyGeometry {
        id: 1,
        centre: origin + Vec2::new(10.5, 0.0),
        damageable: true,
    };
    let endpoint_result = trace_shot(
        &conf,
        &empty_scene(&[endpoint_hit]),
        origin,
        Vec2::new(1.0, 0.0),
    );
    assert_eq!(endpoint_result.first_hit, None);
    approx_point(endpoint_result.blast_point, origin + Vec2::new(10.0, 0.0));
    assert_eq!(endpoint_result.damageable_mask, 1u32 << 1);
    assert!(!legacy_v0_geometric_gate(
        &conf,
        endpoint_hit.centre - origin,
        Vec2::new(1.0, 0.0)
    ));

    let endpoint_miss = EnemyGeometry {
        id: 2,
        centre: origin + Vec2::new(10.56, 0.0),
        damageable: true,
    };
    let miss_result = trace_shot(
        &conf,
        &empty_scene(&[endpoint_miss]),
        origin,
        Vec2::new(1.0, 0.0),
    );
    assert_eq!(miss_result.damageable_mask, 0);
}

#[test]
fn d03_nearest_protected_enemy_still_sets_the_blast() {
    let conf = open_config();
    let origin = Vec2::new(2.0, 2.0);
    let enemies = [
        EnemyGeometry {
            id: 0,
            centre: origin + Vec2::new(5.0, 0.0),
            damageable: false,
        },
        EnemyGeometry {
            id: 1,
            centre: origin + Vec2::new(8.0, 0.0),
            damageable: true,
        },
    ];
    let result = trace_shot(&conf, &empty_scene(&enemies), origin, Vec2::new(1.0, 0.0));
    assert_eq!(result.first_hit, Some(FirstHit::Enemy(0)));
    approx_point(result.blast_point, origin + Vec2::new(4.75, 0.0));
    assert_eq!(result.physical_mask, 1);
    assert_eq!(result.damageable_mask, 0);
}

#[test]
fn d04_turn_limited_actual_ray_hits_wall_not_target_centre_ray() {
    let mut conf = open_config();
    conf.map[5][8] = MapTile::Wall;
    let origin = Vec2::new(2.0, 8.9);
    let target = EnemyGeometry {
        id: 0,
        centre: Vec2::new(7.0, 9.3),
        damageable: true,
    };
    let desired = (target.centre - origin).angle_deg();
    let actual = normalize_degrees(
        357.0 + mm_engine::game::util::diff_degrees(desired, 357.0).clamp(-3.0, 3.0),
    );
    assert!((actual - 0.0).abs() <= 1.0e-5);
    assert!(legacy_v0_geometric_gate(
        &conf,
        target.centre - origin,
        Vec2::from_angle_deg(actual)
    ));
    assert!(
        has_line_of_sight(&conf, origin, target.centre),
        "v0 checked the target-centre LOS rather than the actual post-turn ray"
    );
    let legacy_state = GameState::new(&conf);
    assert!(
        scan(
            &legacy_state,
            &conf,
            origin,
            Vec2::from_angle_deg(actual),
            ScanMask::PAYLOAD | ScanMask::DEPOSITS,
            None,
            (target.centre - origin).dot(Vec2::from_angle_deg(actual)),
        )
        .is_none(),
        "synthetic D04 keeps v0's separately checked dynamic blockers clear"
    );

    let result = trace_shot(
        &conf,
        &empty_scene(&[target]),
        origin,
        Vec2::from_angle_deg(actual),
    );
    assert_eq!(result.first_hit, Some(FirstHit::Wall { x: 5, y: 8 }));
    approx_point(result.blast_point, Vec2::new(5.0, 8.9));
    assert_eq!(result.damageable_mask, 0);
}

#[test]
fn d05_overlapping_blast_sets_charge_only_marginal_targets() {
    let conf = open_config();
    let enemies = [
        EnemyGeometry {
            id: 0,
            centre: Vec2::new(10.45, -0.3),
            damageable: true,
        },
        EnemyGeometry {
            id: 1,
            centre: Vec2::new(10.45, 0.3),
            damageable: true,
        },
        EnemyGeometry {
            id: 2,
            centre: Vec2::new(10.45, 0.9),
            damageable: true,
        },
    ];
    let scene = empty_scene(&enemies);
    let first = trace_shot(&conf, &scene, Vec2::ZERO, Vec2::new(1.0, 0.0));
    let second = trace_shot(&conf, &scene, Vec2::new(0.0, 0.6), Vec2::new(1.0, 0.0));
    assert_eq!(first.damageable_mask, 0b011);
    assert_eq!(second.damageable_mask, 0b110);
    assert_eq!(marginal_damage_mask(&first, 0), 0b011);
    assert_eq!(marginal_damage_mask(&second, first.damageable_mask), 0b100);
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
    let bot = &mut state.fleets_mut()[team][id];
    bot.health = conf.bot.health;
    bot.pos = pos;
    bot.vel = Vec2::ZERO;
    bot.angle = angle;
    bot.turn_vel = 0.0;
    bot.invulnerable_until_tick = 0;
    bot.special = SpecialState::new(class);
    id
}

fn scene_trace(state: &GameState, conf: &GameConfig, origin: Vec2, direction: Vec2) -> ShotResult {
    let enemies: Vec<_> = state.fleets()[Team::B]
        .iter()
        .map(|enemy| EnemyGeometry {
            id: enemy.id,
            centre: enemy.pos,
            damageable: enemy.invulnerable_until_tick <= state.tick,
        })
        .collect();
    let scene = ShotScene {
        enemies: &enemies,
        payload: Circle {
            centre: state.payload_pos(),
            radius: conf.payload.radius,
        },
        deposits: [
            Circle {
                centre: state.deposits()[Team::A].pos,
                radius: conf.deposit.radius,
            },
            Circle {
                centre: state.deposits()[Team::B].pos,
                radius: conf.deposit.radius,
            },
        ],
    };
    trace_shot(conf, &scene, origin, direction)
}

fn oracle_first_hit(target: ScanTarget) -> FirstHit {
    match target {
        ScanTarget::Bot { team: Team::B, id } => FirstHit::Enemy(id),
        ScanTarget::Boundary => FirstHit::Boundary,
        ScanTarget::Wall { x, y } => FirstHit::Wall { x, y },
        ScanTarget::Payload => FirstHit::Payload,
        ScanTarget::Deposit { team: Team::A } => FirstHit::DepositMe,
        ScanTarget::Deposit { team: Team::B } => FirstHit::DepositOther,
        unexpected => panic!("unexpected oracle scan target: {unexpected:?}"),
    }
}

fn oracle_scan(
    state: &GameState,
    conf: &GameConfig,
    origin: Vec2,
    direction: Vec2,
) -> (Vec2, Option<FirstHit>) {
    let mask = ScanMask::bots(Team::B)
        | ScanMask::BOUNDARY
        | ScanMask::WALLS
        | ScanMask::PAYLOAD
        | ScanMask::DEPOSITS;
    match scan(
        state,
        conf,
        origin,
        direction,
        mask,
        None,
        conf.bot.blaster_range,
    ) {
        Some(hit) => (hit.point, Some(oracle_first_hit(hit.target))),
        None => (
            origin + direction.normalize_or_zero() * conf.bot.blaster_range,
            None,
        ),
    }
}

fn hull_clear(conf: &GameConfig, point: Vec2) -> bool {
    let radius = conf.bot.radius;
    if point.x < radius
        || point.x > MAP_SIZE as f32 - radius
        || point.y < radius
        || point.y > MAP_SIZE as f32 - radius
    {
        return false;
    }

    let low_x = (point.x - radius).floor() as isize;
    let high_x = (point.x + radius).floor() as isize;
    let low_y = (point.y - radius).floor() as isize;
    let high_y = (point.y + radius).floor() as isize;
    for x in low_x..=high_x {
        for y in low_y..=high_y {
            if !conf.is_wall(x, y) {
                continue;
            }
            let nearest = Vec2::new(
                point.x.clamp(x as f32, x as f32 + 1.0),
                point.y.clamp(y as f32, y as f32 + 1.0),
            );
            if point.dist_sq(&nearest) < radius * radius {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Copy)]
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 32) as u32
    }

    fn unit(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }
}

fn random_legal_point(
    rng: &mut Lcg,
    state: &GameState,
    conf: &GameConfig,
    occupied: &[Vec2],
) -> Vec2 {
    for _ in 0..20_000 {
        let candidate = Vec2::new(rng.between(0.4, 31.6), rng.between(0.4, 31.6));
        if !hull_clear(conf, candidate)
            || candidate.dist(&state.payload_pos())
                <= conf.payload.capture_radius + conf.bot.radius + 0.05
            || state.deposits().into_iter().any(|deposit| {
                candidate.dist(&deposit.pos) <= conf.deposit.radius + conf.bot.radius + 0.05
            })
            || occupied
                .iter()
                .any(|point| candidate.dist(point) <= conf.bot.radius * 2.0 + 0.01)
        {
            continue;
        }
        return candidate;
    }
    panic!("fixed-seed generator could not find a legal point")
}

#[test]
fn b06_256_fixed_seed_queries_match_official_scan_and_eval_tick() {
    let mut maximum_scan_point_error = 0.0f32;
    let mut maximum_eval_point_error = 0.0f32;
    let mut hit_type_differences = 0u32;
    let mut physical_mask_differences = 0u32;
    let mut damage_mask_differences = 0u32;

    for seed in 0u64..256 {
        let conf = official_config();
        let mut rng = Lcg::new(seed);
        let mut state = GameState::new(&conf);
        state.tick = 100 + seed as u32;
        state.capture = rng.between(-0.85, 0.85);
        state.fabricators_mut()[Team::A].next_bot_creation = u32::MAX;
        state.fabricators_mut()[Team::B].next_bot_creation = u32::MAX;

        let mut occupied = Vec::new();
        let shooter_pos = random_legal_point(&mut rng, &state, &conf, &occupied);
        occupied.push(shooter_pos);
        let angle = rng.between(0.0, 360.0);
        let shooter = spawn(
            &mut state,
            &conf,
            Team::A,
            shooter_pos,
            angle,
            BotClass::Battle,
        );

        let enemy_count = 1 + (rng.next_u32() as usize % 6);
        for ordinal in 0..enemy_count {
            let point = random_legal_point(&mut rng, &state, &conf, &occupied);
            occupied.push(point);
            let id = spawn(
                &mut state,
                &conf,
                Team::B,
                point,
                rng.between(0.0, 360.0),
                if ordinal % 3 == 0 {
                    BotClass::Healer
                } else {
                    BotClass::Battle
                },
            );
            if (seed + ordinal as u64) % 4 == 0 {
                state.fleets_mut()[Team::B][id].invulnerable_until_tick = state.tick + 1;
            }
        }

        let direction = Vec2::from_angle_deg(angle);
        let production = scene_trace(&state, &conf, shooter_pos, direction);
        let (oracle_point, oracle_hit) = oracle_scan(&state, &conf, shooter_pos, direction);
        maximum_scan_point_error =
            maximum_scan_point_error.max(production.blast_point.dist(&oracle_point));
        if production.first_hit != oracle_hit {
            hit_type_differences += 1;
        }

        let splash = conf.bot.radius + conf.bot.base_blaster_splash_radius;
        let mut oracle_physical = 0u32;
        for enemy in state.fleets()[Team::B].iter() {
            if enemy.pos.dist_sq(&oracle_point) <= splash * splash {
                oracle_physical |= 1u32 << enemy.id;
            }
        }
        if production.physical_mask != oracle_physical {
            physical_mask_differences += 1;
        }

        let health_before: Vec<_> = state.fleets()[Team::B]
            .iter()
            .map(|enemy| (enemy.id, enemy.health))
            .collect();
        let mut fire = FleetAction::new();
        fire.bots[shooter as usize].special_action = SpecialAction::Battle { fire: true };
        eval_tick(&mut state, &conf, fire, FleetAction::new());
        let official_shot = state.fleets()[Team::A][shooter]
            .shot()
            .option()
            .expect("ready shooter must record an official blast point");
        maximum_eval_point_error =
            maximum_eval_point_error.max(production.blast_point.dist(&official_shot));

        let mut official_damage = 0u32;
        for (id, health) in health_before {
            if state.fleets()[Team::B][id].health < health {
                official_damage |= 1u32 << id;
            }
        }
        if production.damageable_mask != official_damage {
            damage_mask_differences += 1;
        }
    }

    println!(
        "ballistics_oracle seeds=256 max_scan_error={maximum_scan_point_error} \
         max_eval_error={maximum_eval_point_error} hit_type_diff={hit_type_differences} \
         physical_mask_diff={physical_mask_differences} damage_mask_diff={damage_mask_differences}"
    );
    assert!(
        maximum_scan_point_error <= 1.0e-6,
        "pure scan max blast error={maximum_scan_point_error}"
    );
    // `eval_tick` additionally runs collision projection before firing.  Generated
    // placements are legal, but f32 contact cleanup may still move a centre by a few
    // ten-thousandths; require an error well below the engine's own EPSILON (0.001).
    assert!(
        maximum_eval_point_error <= mm_engine::game::config::EPSILON,
        "post-collision max blast error={maximum_eval_point_error}"
    );
    assert_eq!(hit_type_differences, 0);
    assert_eq!(physical_mask_differences, 0);
    assert_eq!(damage_mask_differences, 0);
}

#[test]
fn exact_range_tie_keeps_endpoint_and_mirror_direction_is_symmetric() {
    let conf = open_config();
    let origin = Vec2::new(8.0, 8.0);
    let enemy = EnemyGeometry {
        id: 4,
        // Near hull is exactly the ten-unit endpoint.  Official strict `<`
        // leaves `first_hit` empty, while the endpoint splash still covers it.
        centre: Vec2::new(18.25, 8.0),
        damageable: true,
    };
    let forward = trace_shot(&conf, &empty_scene(&[enemy]), origin, Vec2::new(1.0, 0.0));
    assert_eq!(forward.first_hit, None);
    assert_eq!(forward.damageable_mask, 1u32 << 4);

    let mirrored_origin = Vec2::new(24.0, 24.0);
    let mirrored_enemy = EnemyGeometry {
        id: 4,
        centre: Vec2::new(13.75, 24.0),
        damageable: true,
    };
    let mirrored = trace_shot(
        &conf,
        &empty_scene(&[mirrored_enemy]),
        mirrored_origin,
        Vec2::new(-1.0, 0.0),
    );
    assert_eq!(mirrored.first_hit, None);
    assert_eq!(mirrored.damageable_mask, forward.damageable_mask);
    approx_point(
        mirrored.blast_point,
        Vec2::new(32.0 - forward.blast_point.x, 32.0 - forward.blast_point.y),
    );
}
