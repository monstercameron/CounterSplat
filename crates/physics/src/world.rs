//! A small, game-neutral wrapper around the Rapier rigid-body world.
//!
//! Callers decide when to step and what bodies represent. Handles are opaque to keep the backend an implementation
//! detail; scan collision is built from the same sparse-voxel format used by the asset loader.

use countersplat_assets::svo::{grid_cells, Octree, WallBox};
use rapier3d::prelude::{
    ColliderBuilder, PhysicsWorld as RapierWorld, RigidBodyBuilder, RigidBodyHandle, Vector,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORLD_ID: AtomicU64 = AtomicU64::new(1);

/// A body identifier owned by one [`World`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyHandle {
    world: u64,
    raw: RigidBodyHandle,
}

/// Whether a body is stationary or moves under forces and collisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Fixed,
    Dynamic,
}

/// An axis-aligned box described in world metres.
#[derive(Debug, Clone, Copy)]
pub struct BoxBody {
    pub kind: BodyKind,
    pub center: [f32; 3],
    pub half_extents: [f32; 3],
    pub friction: f32,
    pub restitution: f32,
}

/// How an occupied scan is turned into collision floor boxes.
#[derive(Debug, Clone, Copy)]
pub struct FloorOptions {
    /// Empty cells required above a floor candidate.
    pub clearance_voxels: u32,
    /// Fill small missing patches before merging cells.
    pub fill_holes: bool,
    /// Median-smooth the extracted height grid.
    pub smooth: bool,
    pub thickness_m: f32,
    pub friction: f32,
}

impl Default for FloorOptions {
    fn default() -> Self {
        Self {
            clearance_voxels: 2,
            fill_holes: true,
            smooth: true,
            thickness_m: 1.0,
            friction: 0.8,
        }
    }
}

/// Parameters for fitting scan obstacles to occupied voxel blocks.
#[derive(Debug, Clone, Copy)]
pub struct WallOptions {
    /// Empty cells needed above candidate floor surfaces.
    pub clearance_voxels: u32,
    /// Half-open height band, in 4-voxel blocks above the nearby floor.
    pub band_blocks: (usize, usize),
    /// Search radius, in 4-voxel blocks, for a nearby floor.
    pub reach_blocks: usize,
    pub friction: f32,
}

/// A simulation world with a caller-selected gravity and fixed substep count.
pub struct World {
    id: u64,
    inner: RapierWorld,
    substeps: u32,
}

impl World {
    pub fn new(gravity: [f32; 3], substeps: u32) -> Result<Self, &'static str> {
        if !gravity.iter().all(|v| v.is_finite()) || substeps == 0 {
            return Err("gravity must be finite and substeps must be nonzero");
        }
        let mut inner = RapierWorld::new();
        inner.gravity = Vector::new(gravity[0], gravity[1], gravity[2]);
        Ok(Self {
            id: NEXT_WORLD_ID.fetch_add(1, Ordering::Relaxed),
            inner,
            substeps,
        })
    }

    /// Adds a fixed or dynamic box and returns its body handle.
    pub fn add_box(&mut self, desc: BoxBody) -> Result<BodyHandle, &'static str> {
        if !desc.center.iter().all(|v| v.is_finite())
            || !desc.half_extents.iter().all(|v| v.is_finite() && *v > 0.0)
            || !desc.friction.is_finite()
            || desc.friction < 0.0
            || !desc.restitution.is_finite()
            || !(0.0..=1.0).contains(&desc.restitution)
        {
            return Err("box position, extents or material are invalid");
        }
        let body = match desc.kind {
            BodyKind::Fixed => RigidBodyBuilder::fixed(),
            BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
        }
        .translation(Vector::new(desc.center[0], desc.center[1], desc.center[2]));
        let collider = ColliderBuilder::cuboid(
            desc.half_extents[0],
            desc.half_extents[1],
            desc.half_extents[2],
        )
        .friction(desc.friction)
        .restitution(desc.restitution);
        Ok(BodyHandle {
            world: self.id,
            raw: self.inner.insert(body, collider).0,
        })
    }

    /// Builds static floor colliders from an SVO's walkable cells. Returns their shared body handle.
    pub fn add_scan_floor(
        &mut self,
        octree: &Octree,
        options: FloorOptions,
    ) -> Result<BodyHandle, &'static str> {
        let cells = octree.floor_cells(options.clearance_voxels);
        self.add_floor_cells(
            &cells,
            grid_cells(&octree.meta),
            octree.meta.grid_min,
            octree.meta.resolution,
            options,
        )
    }

    /// Builds static floor colliders from occupied cell coordinates and grid metadata.
    pub fn add_floor_cells(
        &mut self,
        cells: &[[u32; 3]],
        dims: [u32; 3],
        origin: [f32; 3],
        resolution: f32,
        options: FloorOptions,
    ) -> Result<BodyHandle, &'static str> {
        if cells.is_empty()
            || dims.contains(&0)
            || !resolution.is_finite()
            || resolution <= 0.0
            || !origin.iter().all(|v| v.is_finite())
            || !options.thickness_m.is_finite()
            || options.thickness_m <= 0.0
            || !options.friction.is_finite()
            || options.friction < 0.0
        {
            return Err("floor cells, dimensions or material are invalid");
        }
        let (nx, nz) = (dims[0] as usize, dims[2] as usize);
        let len = nx.checked_mul(nz).ok_or("floor dimensions overflow")?;
        let mut heights = vec![None; len];
        for cell in cells {
            if (0..3).any(|i| cell[i] >= dims[i]) {
                return Err("floor cell is outside the grid");
            }
            let top = (cell[1] as f32 + 1.0) * resolution;
            let slot = &mut heights[cell[0] as usize * nz + cell[2] as usize];
            *slot = Some(slot.map_or(top, |previous: f32| previous.max(top)));
        }
        if options.fill_holes {
            heights = crate::floor::fill_holes(&heights, nx, nz);
        }
        if options.smooth {
            heights = crate::floor::smooth(&heights, nx, nz);
        }
        let boxes = crate::floor::merge(&heights, nx, nz);
        let mut iter = boxes.into_iter();
        let first = iter.next().ok_or("floor produced no collision boxes")?;
        let collider = |b: crate::floor::FloorBox| {
            let width = b.w as f32 * resolution;
            let depth = b.d as f32 * resolution;
            ColliderBuilder::cuboid(width / 2.0, options.thickness_m / 2.0, depth / 2.0)
                .translation(Vector::new(
                    b.x as f32 * resolution + width / 2.0,
                    b.top - options.thickness_m / 2.0,
                    b.z as f32 * resolution + depth / 2.0,
                ))
                .friction(options.friction)
        };
        let body =
            RigidBodyBuilder::fixed().translation(Vector::new(origin[0], origin[1], origin[2]));
        let handle = self.inner.insert(body, collider(first)).0;
        for item in iter {
            self.inner.insert_collider(collider(item), Some(handle));
        }
        Ok(BodyHandle {
            world: self.id,
            raw: handle,
        })
    }

    /// Fits obstacle boxes to occupied scan voxels and adds them as one static body.
    /// Returns `None` if the scan contains no supported obstacles in the selected band.
    pub fn add_scan_walls(
        &mut self,
        octree: &Octree,
        options: WallOptions,
    ) -> Result<Option<BodyHandle>, &'static str> {
        if options.band_blocks.0 >= options.band_blocks.1
            || !options.friction.is_finite()
            || options.friction < 0.0
        {
            return Err("wall band or friction is invalid");
        }
        let (columns, dims) = octree.wall_columns(
            options.clearance_voxels,
            options.band_blocks,
            options.reach_blocks,
        );
        let boxes = octree.wall_boxes(&columns, dims, options.band_blocks);
        self.add_static_boxes(&boxes, octree.meta.grid_min, options.friction)
    }

    /// Adds a collection of axis-aligned boxes in coordinates relative to `origin` as one static body.
    pub fn add_static_boxes(
        &mut self,
        boxes: &[WallBox],
        origin: [f32; 3],
        friction: f32,
    ) -> Result<Option<BodyHandle>, &'static str> {
        if !origin.iter().all(|value| value.is_finite())
            || !friction.is_finite()
            || friction < 0.0
            || boxes.iter().any(|item| {
                (0..3).any(|axis| {
                    !item.min[axis].is_finite()
                        || !item.max[axis].is_finite()
                        || item.min[axis] >= item.max[axis]
                })
            })
        {
            return Err("box bounds, origin or friction are invalid");
        }
        let mut items = boxes.iter();
        let Some(first) = items.next() else {
            return Ok(None);
        };
        let collider = |item: &WallBox| {
            let half = [0, 1, 2].map(|axis| (item.max[axis] - item.min[axis]) * 0.5);
            let center = [0, 1, 2].map(|axis| (item.max[axis] + item.min[axis]) * 0.5);
            ColliderBuilder::cuboid(half[0], half[1], half[2])
                .translation(Vector::new(center[0], center[1], center[2]))
                .friction(friction)
        };
        let body =
            RigidBodyBuilder::fixed().translation(Vector::new(origin[0], origin[1], origin[2]));
        let raw = self.inner.insert(body, collider(first)).0;
        for item in items {
            self.inner.insert_collider(collider(item), Some(raw));
        }
        Ok(Some(BodyHandle {
            world: self.id,
            raw,
        }))
    }

    /// Sets a body's linear velocity in metres per second.
    pub fn set_linear_velocity(
        &mut self,
        handle: BodyHandle,
        velocity: [f32; 3],
    ) -> Result<(), &'static str> {
        if handle.world != self.id || !velocity.iter().all(|value| value.is_finite()) {
            return Err("invalid body or velocity");
        }
        let body = self
            .inner
            .bodies
            .get_mut(handle.raw)
            .ok_or("body no longer exists")?;
        body.set_linvel(Vector::new(velocity[0], velocity[1], velocity[2]), true);
        Ok(())
    }

    /// Steps a caller-supplied amount of simulation time, divided into the configured substeps.
    pub fn step(&mut self, seconds: f32) -> Result<(), &'static str> {
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err("step duration must be positive and finite");
        }
        self.inner.integration_parameters.dt = seconds / self.substeps as f32;
        for _ in 0..self.substeps {
            self.inner.step();
        }
        Ok(())
    }

    /// Returns a body's world position, or `None` for a stale or foreign handle.
    pub fn position(&self, handle: BodyHandle) -> Option<[f32; 3]> {
        if handle.world != self.id {
            return None;
        }
        let position = self.inner.bodies.get(handle.raw)?.translation();
        Some([position.x, position.y, position.z])
    }

    /// Removes a body and its colliders. Returns false for a stale or foreign handle.
    pub fn remove(&mut self, handle: BodyHandle) -> bool {
        if handle.world != self.id || self.inner.bodies.get(handle.raw).is_none() {
            return false;
        }
        self.inner.remove_body_with_colliders(handle.raw, true);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_body_falls_onto_scan_floor() {
        let mut world = World::new([0.0, -9.81, 0.0], 2).unwrap();
        let options = FloorOptions {
            fill_holes: false,
            smooth: false,
            ..Default::default()
        };
        world
            .add_floor_cells(&[[0, 0, 0]], [1, 2, 1], [0.0; 3], 4.0, options)
            .unwrap();
        let body = world
            .add_box(BoxBody {
                kind: BodyKind::Dynamic,
                center: [2.0, 8.0, 2.0],
                half_extents: [0.5; 3],
                friction: 0.5,
                restitution: 0.0,
            })
            .unwrap();
        for _ in 0..240 {
            world.step(1.0 / 60.0).unwrap();
        }
        let y = world.position(body).unwrap()[1];
        assert!(
            (4.45..4.56).contains(&y),
            "body should settle on the voxel floor: {y}"
        );
        assert!(world.remove(body));
        assert!(world.position(body).is_none());
    }
    #[test]
    fn static_scan_boxes_stop_a_dynamic_body_and_handles_are_world_scoped() {
        let mut world = World::new([0.0; 3], 2).unwrap();
        let wall = WallBox {
            min: [0.0, -2.0, -2.0],
            max: [0.2, 2.0, 2.0],
        };
        world
            .add_static_boxes(&[wall], [0.0; 3], 0.5)
            .unwrap()
            .unwrap();
        let body = world
            .add_box(BoxBody {
                kind: BodyKind::Dynamic,
                center: [-2.0, 0.0, 0.0],
                half_extents: [0.25; 3],
                friction: 0.5,
                restitution: 0.0,
            })
            .unwrap();
        let mut other = World::new([0.0; 3], 1).unwrap();
        other
            .add_box(BoxBody {
                kind: BodyKind::Dynamic,
                center: [0.0; 3],
                half_extents: [0.25; 3],
                friction: 0.5,
                restitution: 0.0,
            })
            .unwrap();
        assert!(other.position(body).is_none());
        assert!(!other.remove(body));
        world.set_linear_velocity(body, [5.0, 0.0, 0.0]).unwrap();
        for _ in 0..120 {
            world.step(1.0 / 60.0).unwrap();
        }
        assert!(
            world.position(body).unwrap()[0] < -0.2,
            "body must not cross the wall"
        );
    }
    #[test]
    fn rejects_invalid_inputs_before_building_collision() {
        let mut world = World::new([0.0, -9.81, 0.0], 1).unwrap();
        assert!(world.step(0.0).is_err());
        assert!(world
            .add_floor_cells(
                &[[2, 0, 0]],
                [1, 1, 1],
                [0.0; 3],
                1.0,
                FloorOptions::default()
            )
            .is_err());
    }
}
