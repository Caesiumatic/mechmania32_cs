//! Pure, production-side blaster geometry.
//!
//! This mirrors the pinned engine's `geom::scan` ordering and `step_blasters`
//! explosion rule without depending on the engine-only feature.  Behavioural
//! prediction (for example, where an enemy will move next tick) belongs to the
//! caller: every coordinate supplied here is interpreted as the coordinate at
//! firing time, after movement and collision resolution.

use crate::core::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub centre: Vec2,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnemyGeometry {
    pub id: BotId,
    pub centre: Vec2,
    /// Whether this enemy may take damage on the queried tick.  Protected
    /// enemies remain in `enemies` because they still stop the ray.
    pub damageable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstHit {
    Enemy(BotId),
    Boundary,
    Wall { x: u8, y: u8 },
    Payload,
    DepositMe,
    DepositOther,
}

pub struct ShotScene<'a> {
    /// Keep this in ascending BotId order to preserve the engine's strict-tie
    /// scan semantics.
    pub enemies: &'a [EnemyGeometry],
    pub payload: Circle,
    /// Engine/client team iteration order: Me, then Other.
    pub deposits: [Circle; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotResult {
    pub blast_point: Vec2,
    pub first_hit: Option<FirstHit>,
    /// All enemy hulls touched by the explosion, including protected enemies.
    pub physical_mask: u32,
    /// The subset that may actually take damage on this tick.
    pub damageable_mask: u32,
}

fn ray_circle(origin: Vec2, dir: Vec2, centre: Vec2, radius: f32) -> Option<f32> {
    let offset = centre - origin;
    let along = offset.dot(dir);
    let outside = offset.norm_sq() - radius * radius;

    if outside > 0.0 && along < 0.0 {
        return None;
    }

    let discriminant = along * along - outside;
    if discriminant < 0.0 {
        return None;
    }

    Some((along - discriminant.sqrt()).max(0.0))
}

fn ray_boundary(origin: Vec2, dir: Vec2) -> f32 {
    let width = MAP_SIZE as f32;
    let mut distance = f32::INFINITY;

    for (coordinate, component) in [(origin.x, dir.x), (origin.y, dir.y)] {
        if component.abs() <= EPSILON {
            continue;
        }
        let face = if component > 0.0 { width } else { 0.0 };
        distance = distance.min((face - coordinate) / component);
    }

    distance.max(0.0)
}

/// Exact copy of the pinned engine's Amanatides-Woo wall traversal.  In
/// particular, equal-distance ties do not replace an earlier hit.
fn ray_walls(
    conf: &GameConfig,
    origin: Vec2,
    dir: Vec2,
    max_distance: f32,
) -> Option<(f32, u8, u8)> {
    let size = MAP_SIZE as isize;
    let mut cell = [origin.x.floor() as isize, origin.y.floor() as isize];
    let coordinates = [origin.x, origin.y];
    let components = [dir.x, dir.y];
    let mut step = [0isize; 2];
    let mut next = [f32::INFINITY; 2];
    let mut delta = [f32::INFINITY; 2];

    for axis in 0..2 {
        if components[axis].abs() <= EPSILON {
            continue;
        }
        delta[axis] = 1.0 / components[axis].abs();
        if components[axis] > 0.0 {
            step[axis] = 1;
            next[axis] = ((cell[axis] + 1) as f32 - coordinates[axis]) / components[axis];
        } else {
            step[axis] = -1;
            next[axis] = (cell[axis] as f32 - coordinates[axis]) / components[axis];
        }
    }

    let mut distance = 0.0;
    loop {
        if cell[0] < 0 || cell[0] >= size || cell[1] < 0 || cell[1] >= size {
            return None;
        }
        if distance > max_distance {
            return None;
        }
        if conf.is_wall(cell[0], cell[1]) {
            return Some((distance, cell[0] as u8, cell[1] as u8));
        }

        let axis = if next[0] <= next[1] { 0 } else { 1 };
        if !next[axis].is_finite() {
            return None;
        }
        distance = next[axis];
        cell[axis] += step[axis];
        next[axis] += delta[axis];
    }
}

fn consider(
    distance: f32,
    hit: FirstHit,
    best_distance: &mut f32,
    first_hit: &mut Option<FirstHit>,
) {
    if distance < *best_distance {
        *best_distance = distance;
        *first_hit = Some(hit);
    }
}

/// Trace one blaster shot with the pinned engine's first-collision and splash
/// semantics.  Allies are intentionally absent from `ShotScene`, so they are
/// transparent just as they are in `step_blasters`.
pub fn trace_shot(
    conf: &GameConfig,
    scene: &ShotScene<'_>,
    origin: Vec2,
    direction: Vec2,
) -> ShotResult {
    let dir = direction.normalize_or_zero();
    if dir == Vec2::ZERO {
        return ShotResult {
            blast_point: origin,
            first_hit: None,
            physical_mask: splash_mask(conf, scene.enemies, origin, false),
            damageable_mask: splash_mask(conf, scene.enemies, origin, true),
        };
    }

    let range = conf.bot.blaster_range;
    let boundary_distance = ray_boundary(origin, dir);
    let mut best_distance = boundary_distance.min(range);
    let mut first_hit = if boundary_distance <= range {
        Some(FirstHit::Boundary)
    } else {
        None
    };

    for enemy in scene.enemies {
        if let Some(distance) = ray_circle(origin, dir, enemy.centre, conf.bot.radius) {
            consider(
                distance,
                FirstHit::Enemy(enemy.id),
                &mut best_distance,
                &mut first_hit,
            );
        }
    }

    if let Some(distance) = ray_circle(origin, dir, scene.payload.centre, scene.payload.radius) {
        consider(
            distance,
            FirstHit::Payload,
            &mut best_distance,
            &mut first_hit,
        );
    }

    for (index, deposit) in scene.deposits.iter().enumerate() {
        if let Some(distance) = ray_circle(origin, dir, deposit.centre, deposit.radius) {
            let hit = if index == 0 {
                FirstHit::DepositMe
            } else {
                FirstHit::DepositOther
            };
            consider(distance, hit, &mut best_distance, &mut first_hit);
        }
    }

    if let Some((distance, x, y)) = ray_walls(conf, origin, dir, best_distance) {
        consider(
            distance,
            FirstHit::Wall { x, y },
            &mut best_distance,
            &mut first_hit,
        );
    }

    let blast_point = origin + dir * best_distance;
    ShotResult {
        blast_point,
        first_hit,
        physical_mask: splash_mask(conf, scene.enemies, blast_point, false),
        damageable_mask: splash_mask(conf, scene.enemies, blast_point, true),
    }
}

fn splash_mask(
    conf: &GameConfig,
    enemies: &[EnemyGeometry],
    blast_point: Vec2,
    damageable_only: bool,
) -> u32 {
    let reach = conf.bot.radius + conf.bot.base_blaster_splash_radius;
    enemies.iter().fold(0u32, |mask, enemy| {
        if (!damageable_only || enemy.damageable)
            && enemy.centre.dist_sq(&blast_point) <= reach * reach
        {
            mask | (1u32 << enemy.id)
        } else {
            mask
        }
    })
}

#[inline]
pub fn marginal_damage_mask(result: &ShotResult, already_covered: u32) -> u32 {
    result.damageable_mask & !already_covered
}
