//! Thread-owned entity/component storage with explicit, language-neutral scripting interfaces.
//!
//! A [`World`] belongs to one simulation worker. Other threads exchange [`WorldCommand`] values and receive
//! [`WorldReply`] or [`WorldSnapshot`] data; they never need a mutable world reference. Components may be ordinary
//! Rust types or named JSON values. Rust-owned components can opt into script access through adapters.
//!
//! Entity identifiers carry a world identifier and a slot generation. Despawning invalidates old handles; handles
//! from a different world cannot accidentally address an entity in this one. IDs are process-local, not save IDs.
//!
//! # Example
//!
//! Dynamic components hold script-owned data. Native Rust components stay
//! private until explicitly registered with a serialization adapter.
//!
//! ```
//! use countersplat_ecs::{World, WorldOperation, WorldOutcome};
//! use serde_json::json;
//!
//! let mut world = World::new();
//! let entity = world.spawn();
//! world.set_dynamic(entity, "Health", json!(100))?;
//! let result = world.apply_operation(WorldOperation::Query {
//!     components: vec!["Health".into()],
//! })?;
//! assert_eq!(result, WorldOutcome::Entities(vec![entity]));
//! # Ok::<(), String>(())
//! ```

mod bridge;
mod entity;
mod schedule;
mod script;
mod store;
mod world;

pub use bridge::{
    EntitySnapshot, WorldCommand, WorldOperation, WorldOutcome, WorldReply, WorldSnapshot,
};
pub use entity::Entity;
pub use schedule::{Schedule, System, SystemError};
pub use serde_json::Value;
pub use world::{
    write_object_field_via_adapter, write_object_fields_via_adapter, ComponentAdapter,
    ComponentFields, ComponentView, EntityIndex, LifecycleEvent, World,
};
