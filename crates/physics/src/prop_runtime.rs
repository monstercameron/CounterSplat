//! Connects a [`PropWorld`] to a game's ECS through its JSON operations (`get_resource`, `drain`, `set`), so scripts
//! describe props and push them while Rust simulates and draws them.
//!
//! - The `props.layout` resource lists the props: `{version, gravity: [x, y, z], bodies: [{entity?, shape: "box" |
//!   "cylinder", half_extents | radius + half_height, position, yaw, dynamic, mass, friction, restitution,
//!   linear_damping, angular_damping, color: [r, g, b], cap_color?: [r, g, b], visible?}]}` (`visible: false` for
//!   collision-only bodies such as the floor under the props). A new `version` rebuilds the world;
//!   a null resource removes it.
//! - `prop.impulse` events `{entity, impulse: [x, y, z], point: [x, y, z]}` push the prop bound to `entity`.
//! - `prop.reset` events put every dynamic prop back where the layout placed it.
//! - Each tick writes moving props' positions into their entity's `Transform`, so hit tests follow them.

use crate::props::{PropDesc, PropPose, PropShape, PropWorld};
use serde_json::{json, Value};

/// The game's ECS operations, as JSON in and out.
pub trait Ecs {
    fn ecs(&mut self, operation: Value) -> Result<Value, String>;
}

/// What the renderer needs for one prop: its shape, pose, scale (box half extents, or radius / half height /
/// radius for a cylinder) and colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropInstance {
    pub cylinder: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub color: [f32; 3],
    pub cap_color: [f32; 3],
}

struct Prop {
    entity: Option<String>,
    dynamic: bool,
    spawn: ([f32; 3], f32),
    scale: [f32; 3],
    cylinder: bool,
    color: [f32; 3],
    cap_color: [f32; 3],
    visible: bool,
    /// The last position written to the entity, so a resting prop is not rewritten every tick.
    written: Option<[f32; 3]>,
}

#[derive(Default)]
pub struct PropRuntime {
    world: Option<PropWorld>,
    version: Option<Value>,
    props: Vec<Prop>,
}

const MAX_PROPS: usize = 256;

impl PropRuntime {
    /// Reads the layout and the scripts' events, advances the props by `seconds` and writes their positions back.
    pub fn tick(&mut self, ecs: &mut impl Ecs, seconds: f32) -> Result<(), String> {
        let layout = ecs.ecs(json!({"op": "get_resource", "name": "props.layout"}))?;
        if layout.is_null() {
            self.world = None;
            self.version = None;
            self.props.clear();
            return Ok(());
        }
        let version = layout.get("version").cloned().unwrap_or(Value::Null);
        if self.version.as_ref() != Some(&version) {
            self.version = Some(version);
            self.build(&layout)?;
        }
        let Some(world) = self.world.as_mut() else {
            return Ok(());
        };
        for event in drain(ecs, "prop.reset")? {
            let _ = event;
            for (index, prop) in self.props.iter_mut().enumerate().filter(|(_, p)| p.dynamic) {
                world.reset(index, prop.spawn.0, prop.spawn.1).map_err(str::to_owned)?;
                prop.written = None;
            }
        }
        for event in drain(ecs, "prop.impulse")? {
            let entity = event.get("entity").and_then(Value::as_str);
            let index = self.props.iter().position(|p| p.entity.is_some() && p.entity.as_deref() == entity);
            if let (Some(index), Some(impulse), Some(point)) = (index, vec3(event.get("impulse")), vec3(event.get("point"))) {
                world.apply_impulse(index, impulse, point).map_err(str::to_owned)?;
            }
        }
        world.step(seconds);
        for (index, prop) in self.props.iter_mut().enumerate() {
            let (Some(entity), true) = (prop.entity.as_ref(), prop.dynamic) else {
                continue;
            };
            let Some(pose) = world.pose(index) else {
                continue;
            };
            if prop.written != Some(pose.position) {
                ecs.ecs(json!({"op": "set", "id": entity, "name": "Transform", "value": {"position": pose.position}}))?;
                prop.written = Some(pose.position);
            }
        }
        Ok(())
    }

    fn build(&mut self, layout: &Value) -> Result<(), String> {
        self.world = None;
        self.props.clear();
        let gravity = vec3(layout.get("gravity")).unwrap_or([0.0, -9.81, 0.0]);
        let mut world = PropWorld::new(gravity).map_err(str::to_owned)?;
        let bodies = layout.get("bodies").and_then(Value::as_array).ok_or("props.layout needs a bodies list")?;
        if bodies.len() > MAX_PROPS {
            return Err(format!("props.layout has more than {MAX_PROPS} bodies"));
        }
        for body in bodies {
            let number = |key: &str, default: f32| body.get(key).and_then(Value::as_f64).map_or(default, |v| v as f32);
            let (shape, scale, cylinder) = match body.get("shape").and_then(Value::as_str) {
                Some("box") => {
                    let half = vec3(body.get("half_extents")).ok_or("a box prop needs half_extents")?;
                    (PropShape::Box { half_extents: half }, half, false)
                }
                Some("cylinder") => {
                    let (radius, half_height) = (number("radius", 0.0), number("half_height", 0.0));
                    (PropShape::Cylinder { radius, half_height }, [radius, half_height, radius], true)
                }
                _ => return Err("a prop's shape must be \"box\" or \"cylinder\"".into()),
            };
            let position = vec3(body.get("position")).ok_or("a prop needs a position")?;
            let yaw = number("yaw", 0.0);
            let dynamic = body.get("dynamic").and_then(Value::as_bool).unwrap_or(false);
            world
                .add(PropDesc {
                    shape,
                    position,
                    yaw,
                    dynamic,
                    mass: number("mass", 0.0),
                    friction: number("friction", 0.5),
                    restitution: number("restitution", 0.0),
                    linear_damping: number("linear_damping", 0.0),
                    angular_damping: number("angular_damping", 0.0),
                })
                .map_err(str::to_owned)?;
            let body_color = color(body.get("color")).unwrap_or([0.6, 0.6, 0.6]);
            self.props.push(Prop {
                entity: body.get("entity").and_then(Value::as_str).map(str::to_owned),
                dynamic,
                spawn: (position, yaw),
                scale,
                cylinder,
                color: body_color,
                cap_color: color(body.get("cap_color")).unwrap_or(body_color),
                visible: body.get("visible").and_then(Value::as_bool).unwrap_or(true),
                written: None,
            });
        }
        self.world = Some(world);
        Ok(())
    }

    /// The props to draw this frame.
    pub fn instances(&self) -> Vec<PropInstance> {
        let Some(world) = self.world.as_ref() else {
            return Vec::new();
        };
        self.props
            .iter()
            .enumerate()
            .filter(|(_, prop)| prop.visible)
            .filter_map(|(index, prop)| {
                let PropPose { position, rotation, .. } = world.pose(index)?;
                Some(PropInstance {
                    cylinder: prop.cylinder,
                    position,
                    rotation,
                    scale: prop.scale,
                    color: prop.color,
                    cap_color: prop.cap_color,
                })
            })
            .collect()
    }
}

fn drain(ecs: &mut impl Ecs, name: &str) -> Result<Vec<Value>, String> {
    Ok(match ecs.ecs(json!({"op": "drain", "name": name}))? {
        Value::Array(events) => events,
        _ => Vec::new(),
    })
}

fn vec3(value: Option<&Value>) -> Option<[f32; 3]> {
    let items = value?.as_array()?;
    if items.len() != 3 {
        return None;
    }
    let mut out = [0.0; 3];
    for (o, v) in out.iter_mut().zip(items) {
        *o = v.as_f64().filter(|v| v.is_finite())? as f32;
    }
    Some(out)
}

/// A linear RGB colour, each channel 0..8 (HDR allowed, as the line layers accept).
fn color(value: Option<&Value>) -> Option<[f32; 3]> {
    vec3(value).filter(|c| c.iter().all(|v| (0.0..=8.0).contains(v)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[derive(Default)]
    struct Fake {
        resources: Map<String, Value>,
        events: Map<String, Value>,
        transforms: Map<String, Value>,
        writes: usize,
    }

    impl Ecs for Fake {
        fn ecs(&mut self, op: Value) -> Result<Value, String> {
            let name = op["name"].as_str().unwrap_or_default().to_owned();
            Ok(match op["op"].as_str().unwrap() {
                "get_resource" => self.resources.get(&name).cloned().unwrap_or(Value::Null),
                "drain" => self.events.remove(&name).unwrap_or(json!([])),
                "set" => {
                    assert_eq!(name, "Transform");
                    self.writes += 1;
                    self.transforms.insert(op["id"].as_str().unwrap().to_owned(), op["value"].clone());
                    Value::Null
                }
                other => panic!("unexpected op {other}"),
            })
        }
    }

    fn layout(version: u32) -> Value {
        json!({"version": version, "gravity": [0.0, -9.81, 0.0], "bodies": [
            {"shape": "box", "half_extents": [10.0, 0.5, 10.0], "position": [0.0, -0.5, 0.0], "visible": false},
            {"entity": "wall", "shape": "box", "half_extents": [1.1, 0.5, 0.1], "position": [0.0, 0.5, 0.0],
             "color": [0.5, 0.45, 0.4]},
            {"entity": "can_1", "shape": "cylinder", "radius": 0.033, "half_height": 0.061,
             "position": [0.0, 1.062, 0.0], "dynamic": true, "mass": 0.015, "friction": 0.4, "restitution": 0.25,
             "linear_damping": 0.05, "angular_damping": 0.4, "color": [0.8, 0.05, 0.04], "cap_color": [0.8, 0.8, 0.8]}
        ]})
    }

    fn run(runtime: &mut PropRuntime, fake: &mut Fake, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            runtime.tick(fake, 1.0 / 60.0).unwrap();
        }
    }

    #[test]
    fn scripts_lay_out_push_and_reset_props_through_the_ecs() {
        let mut fake = Fake::default();
        let mut runtime = PropRuntime::default();
        runtime.tick(&mut fake, 1.0 / 60.0).unwrap();
        assert!(runtime.instances().is_empty(), "no layout, no props");
        fake.resources.insert("props.layout".into(), layout(1));
        run(&mut runtime, &mut fake, 2.0);
        let instances = runtime.instances();
        assert_eq!(instances.len(), 2, "the collision-only floor is not drawn");
        assert!(instances[1].cylinder && instances[1].cap_color == [0.8, 0.8, 0.8]);
        let rest = fake.transforms["can_1"]["position"][1].as_f64().unwrap();
        assert!((rest - 1.061).abs() < 0.01, "the can rests on the wall: {rest}");
        assert!(!fake.transforms.contains_key("wall"), "fixed props are not rewritten");
        let writes = fake.writes;
        run(&mut runtime, &mut fake, 0.5);
        assert_eq!(fake.writes, writes, "a sleeping can is not rewritten every tick");

        fake.events.insert("prop.impulse".into(), json!([
            {"entity": "can_1", "impulse": [0.0, 0.0, 0.1], "point": [0.0, 1.08, -0.033]},
            {"entity": "nobody", "impulse": [0.0, 0.0, 9.0], "point": [0.0, 0.0, 0.0]}
        ]));
        run(&mut runtime, &mut fake, 3.0);
        let knocked = fake.transforms["can_1"]["position"].clone();
        assert!(knocked[1].as_f64().unwrap() < 0.2 && knocked[2].as_f64().unwrap() > 0.15, "knocked off: {knocked}");

        fake.events.insert("prop.reset".into(), json!([{}]));
        run(&mut runtime, &mut fake, 1.5);
        let back = fake.transforms["can_1"]["position"][1].as_f64().unwrap();
        assert!((back - 1.061).abs() < 0.01, "reset onto the wall: {back}");

        // A new version rebuilds; a null layout removes the props.
        fake.resources.insert("props.layout".into(), layout(2));
        runtime.tick(&mut fake, 1.0 / 60.0).unwrap();
        assert_eq!(runtime.instances().len(), 2);
        fake.resources.insert("props.layout".into(), Value::Null);
        runtime.tick(&mut fake, 1.0 / 60.0).unwrap();
        assert!(runtime.instances().is_empty());
    }

    #[test]
    fn invalid_layouts_are_reported() {
        let mut fake = Fake::default();
        let mut runtime = PropRuntime::default();
        for bad in [
            json!({"version": 1, "bodies": [{"shape": "sphere", "position": [0, 0, 0]}]}),
            json!({"version": 2, "bodies": [{"shape": "box", "position": [0, 0, 0]}]}),
            json!({"version": 3, "bodies": [{"shape": "cylinder", "radius": 0.03, "half_height": 0.06,
                   "position": [0, 0, 0], "dynamic": true}]}),
            json!({"version": 4}),
        ] {
            fake.resources.insert("props.layout".into(), bad.clone());
            assert!(runtime.tick(&mut fake, 0.016).is_err(), "{bad}");
        }
    }
}
