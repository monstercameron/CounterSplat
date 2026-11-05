//! Rigid-body props: boxes and upright cylinders that rest, slide, tip and tumble under gravity, and take impulses
//! (a bullet hitting a can). A small Rapier world of its own, independent of the level's voxel collision: the
//! caller places a floor (a wide fixed box) and any fixed props, then dynamic ones on top.
//!
//! Units are the caller's (the game uses level units scaled by its units-per-metre); every parameter here is plain
//! physics: mass in kilograms, friction and restitution coefficients, damping per second.

use rapier3d::prelude::{
    ColliderBuilder, PhysicsWorld as RapierWorld, RigidBodyBuilder, RigidBodyHandle, Vector,
};

/// A prop's collision shape. Cylinders stand along their local Y axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropShape {
    Box { half_extents: [f32; 3] },
    Cylinder { radius: f32, half_height: f32 },
}

/// One prop to add: its shape, starting pose (position and rotation about Y) and material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropDesc {
    pub shape: PropShape,
    pub position: [f32; 3],
    pub yaw: f32,
    /// Fixed props (a wall, the floor) never move; dynamic ones need a positive `mass`.
    pub dynamic: bool,
    pub mass: f32,
    pub friction: f32,
    pub restitution: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
}

/// A prop's current pose: position and rotation quaternion (x, y, z, w), and whether it has come to rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropPose {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub sleeping: bool,
}

/// The fixed step the world advances by; `step` runs as many as the elapsed time needs (at most `MAX_STEPS`).
const STEP_SECONDS: f32 = 1.0 / 120.0;
const MAX_STEPS: u32 = 12;

pub struct PropWorld {
    inner: RapierWorld,
    bodies: Vec<RigidBodyHandle>,
    pending: f32,
}

impl PropWorld {
    /// An empty world with `gravity` (units per second squared, usually negative Y).
    pub fn new(gravity: [f32; 3]) -> Result<Self, &'static str> {
        if !gravity.iter().all(|v| v.is_finite() && v.abs() <= 1000.0) {
            return Err("prop gravity must be finite");
        }
        let mut inner = RapierWorld::new();
        inner.gravity = Vector::new(gravity[0], gravity[1], gravity[2]);
        inner.integration_parameters.dt = STEP_SECONDS;
        Ok(Self {
            inner,
            bodies: Vec::new(),
            pending: 0.0,
        })
    }

    /// Adds a prop and returns its index (props are numbered in the order they are added).
    pub fn add(&mut self, desc: PropDesc) -> Result<usize, &'static str> {
        let finite = |v: f32| v.is_finite();
        let size_ok = match desc.shape {
            PropShape::Box { half_extents } => half_extents.iter().all(|&v| finite(v) && v > 0.0 && v <= 10_000.0),
            PropShape::Cylinder { radius, half_height } => {
                finite(radius) && finite(half_height) && radius > 0.0 && half_height > 0.0 && radius <= 1000.0
                    && half_height <= 1000.0
            }
        };
        if !size_ok
            || !desc.position.iter().all(|v| finite(*v) && v.abs() <= 10_000_000.0)
            || !finite(desc.yaw)
            || !(finite(desc.friction) && desc.friction >= 0.0 && desc.friction <= 10.0)
            || !(finite(desc.restitution) && (0.0..=1.0).contains(&desc.restitution))
            || !(finite(desc.linear_damping) && (0.0..=100.0).contains(&desc.linear_damping))
            || !(finite(desc.angular_damping) && (0.0..=100.0).contains(&desc.angular_damping))
            || (desc.dynamic && !(finite(desc.mass) && desc.mass > 0.0 && desc.mass <= 100_000.0))
        {
            return Err("prop shape, pose or material is invalid");
        }
        let body = if desc.dynamic {
            // Small fast props (a can hit by a bullet) need continuous collision so they cannot pass through the wall.
            RigidBodyBuilder::dynamic()
                .linear_damping(desc.linear_damping)
                .angular_damping(desc.angular_damping)
                .ccd_enabled(true)
        } else {
            RigidBodyBuilder::fixed()
        }
        .translation(Vector::new(desc.position[0], desc.position[1], desc.position[2]))
        .rotation(Vector::new(0.0, desc.yaw, 0.0));
        let collider = match desc.shape {
            PropShape::Box { half_extents } => ColliderBuilder::cuboid(half_extents[0], half_extents[1], half_extents[2]),
            PropShape::Cylinder { radius, half_height } => ColliderBuilder::cylinder(half_height, radius),
        }
        .friction(desc.friction)
        .restitution(desc.restitution);
        let collider = if desc.dynamic { collider.mass(desc.mass) } else { collider };
        self.bodies.push(self.inner.insert(body, collider).0);
        Ok(self.bodies.len() - 1)
    }

    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Applies an `impulse` (mass times velocity change, e.g. kg·m/s) at the world `point`, waking the prop. A point
    /// away from the centre of mass also sets it spinning. Fixed props ignore impulses.
    pub fn apply_impulse(&mut self, prop: usize, impulse: [f32; 3], point: [f32; 3]) -> Result<(), &'static str> {
        if !impulse.iter().chain(&point).all(|v| v.is_finite() && v.abs() <= 1.0e6) {
            return Err("prop impulse or point is invalid");
        }
        let handle = *self.bodies.get(prop).ok_or("no such prop")?;
        let body = self.inner.bodies.get_mut(handle).ok_or("no such prop")?;
        if body.is_dynamic() {
            body.apply_impulse_at_point(
                Vector::new(impulse[0], impulse[1], impulse[2]),
                Vector::new(point[0], point[1], point[2]),
                true,
            );
        }
        Ok(())
    }

    /// Advances the world by `seconds` of game time in fixed steps; a long gap (a paused tab) is capped rather than
    /// simulated in full.
    pub fn step(&mut self, seconds: f32) {
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        self.pending = (self.pending + seconds).min(STEP_SECONDS * MAX_STEPS as f32);
        while self.pending >= STEP_SECONDS {
            self.inner.step();
            self.pending -= STEP_SECONDS;
        }
    }

    /// The prop's pose now.
    pub fn pose(&self, prop: usize) -> Option<PropPose> {
        let body = self.inner.bodies.get(*self.bodies.get(prop)?)?;
        let t = body.translation();
        let r = body.rotation();
        Some(PropPose {
            position: [t.x, t.y, t.z],
            rotation: [r.x, r.y, r.z, r.w],
            sleeping: body.is_sleeping(),
        })
    }

    /// Puts a dynamic prop back at `position`, upright with `yaw`, at rest.
    pub fn reset(&mut self, prop: usize, position: [f32; 3], yaw: f32) -> Result<(), &'static str> {
        if !position.iter().all(|v| v.is_finite()) || !yaw.is_finite() {
            return Err("prop reset pose is invalid");
        }
        let handle = *self.bodies.get(prop).ok_or("no such prop")?;
        let body = self.inner.bodies.get_mut(handle).ok_or("no such prop")?;
        body.set_translation(Vector::new(position[0], position[1], position[2]), false);
        body.set_rotation(rapier3d::prelude::Rotation::from_rotation_y(yaw), false);
        body.set_linvel(Vector::ZERO, false);
        body.set_angvel(Vector::ZERO, false);
        body.wake_up(true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAN_RADIUS: f32 = 0.033;
    const CAN_HALF_HEIGHT: f32 = 0.061;

    fn floor() -> PropDesc {
        PropDesc {
            shape: PropShape::Box { half_extents: [10.0, 0.5, 10.0] },
            position: [0.0, -0.5, 0.0],
            yaw: 0.0,
            dynamic: false,
            mass: 0.0,
            friction: 0.6,
            restitution: 0.1,
            linear_damping: 0.0,
            angular_damping: 0.0,
        }
    }

    fn wall() -> PropDesc {
        PropDesc {
            shape: PropShape::Box { half_extents: [1.1, 0.5, 0.1] },
            position: [0.0, 0.5, 0.0],
            ..floor()
        }
    }

    fn can(x: f32) -> PropDesc {
        PropDesc {
            shape: PropShape::Cylinder { radius: CAN_RADIUS, half_height: CAN_HALF_HEIGHT },
            position: [x, 1.0 + CAN_HALF_HEIGHT + 0.001, 0.0],
            yaw: 0.0,
            dynamic: true,
            mass: 0.015,
            friction: 0.4,
            restitution: 0.25,
            linear_damping: 0.05,
            angular_damping: 0.4,
        }
    }

    fn settle(world: &mut PropWorld, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            world.step(1.0 / 60.0);
        }
    }

    #[test]
    fn cans_rest_on_the_wall_and_fall_to_the_floor_when_shot() {
        let mut world = PropWorld::new([0.0, -9.81, 0.0]).unwrap();
        world.add(floor()).unwrap();
        world.add(wall()).unwrap();
        let cans: Vec<usize> = (0..10).map(|i| world.add(can(-0.9 + 0.2 * i as f32)).unwrap()).collect();
        settle(&mut world, 3.0);
        for &c in &cans {
            let pose = world.pose(c).unwrap();
            assert!((pose.position[1] - (1.0 + CAN_HALF_HEIGHT)).abs() < 0.01, "can {c} stands on the wall: {pose:?}");
            assert!(pose.rotation[3].abs() > 0.999, "can {c} stays upright: {pose:?}");
            assert!(pose.sleeping, "can {c} comes to rest");
        }
        // A shot from in front (-z towards +z), slightly above the middle, through the third can.
        let target = cans[2];
        let centre = world.pose(target).unwrap().position;
        world.apply_impulse(target, [0.0, 0.0, 0.1], [centre[0], centre[1] + 0.02, centre[2] - CAN_RADIUS]).unwrap();
        settle(&mut world, 3.0);
        let hit = world.pose(target).unwrap();
        assert!(hit.position[1] < 0.2, "the shot can lands on the floor: {hit:?}");
        assert!(hit.position[2] > 0.15, "and lands behind the wall: {hit:?}");
        for &c in cans.iter().filter(|&&c| c != target) {
            let pose = world.pose(c).unwrap();
            assert!(pose.position[1] > 1.0, "can {c} is still on the wall: {pose:?}");
        }
        // Reset puts it back upright on the wall, where it settles again.
        world.reset(target, [centre[0], centre[1] + 0.001, centre[2]], 0.0).unwrap();
        settle(&mut world, 2.0);
        let back = world.pose(target).unwrap();
        assert!((back.position[1] - centre[1]).abs() < 0.01 && back.rotation[3].abs() > 0.999, "{back:?}");
    }

    #[test]
    fn fixed_props_ignore_impulses_and_bad_input_is_rejected() {
        let mut world = PropWorld::new([0.0, -9.81, 0.0]).unwrap();
        let wall = world.add(wall()).unwrap();
        world.apply_impulse(wall, [0.0, 0.0, 100.0], [0.0, 0.5, 0.0]).unwrap();
        settle(&mut world, 1.0);
        assert_eq!(world.pose(wall).unwrap().position, [0.0, 0.5, 0.0]);
        assert!(world.add(PropDesc { mass: 0.0, ..can(0.0) }).is_err(), "dynamic props need mass");
        assert!(world.add(PropDesc { restitution: 2.0, ..can(0.0) }).is_err());
        assert!(world.apply_impulse(wall, [f32::NAN, 0.0, 0.0], [0.0; 3]).is_err());
        assert!(world.apply_impulse(99, [0.0; 3], [0.0; 3]).is_err());
        assert!(PropWorld::new([0.0, f32::INFINITY, 0.0]).is_err());
    }

    #[test]
    fn long_gaps_are_capped_instead_of_simulated() {
        let mut world = PropWorld::new([0.0, -9.81, 0.0]).unwrap();
        let ball = world.add(can(0.0)).unwrap();
        world.step(10.0);
        // At most MAX_STEPS fixed steps ran: about 0.1 s of free fall, not 10 s.
        let y = world.pose(ball).unwrap().position[1];
        assert!(y > 1.0, "fell only briefly: {y}");
    }
}
