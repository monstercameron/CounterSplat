//! Owned, serializable messages for interacting with a world on its owner thread.
//!
//! The crate does not prescribe a transport: a channel, request queue or local
//! call can all carry these types. Replies retain request IDs so a caller can
//! correlate responses even when several subsystems submit requests.
use crate::{ComponentFields, Entity, Value, World};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The script-visible state of one entity at the time of a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub entity: Entity,
    pub parent: Option<Entity>,
    /// Dynamic components and typed components registered through an adapter.
    pub components: BTreeMap<String, Value>,
}

/// An owned point-in-time view that can be sent to another thread.
///
/// Only explicitly exposed components and dynamic resources are included.
/// Events are queues and are retrieved separately; taking a snapshot does not
/// consume them. Native resources remain private unless an application exposes
/// their values as dynamic resources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub entities: Vec<EntitySnapshot>,
    pub resources: BTreeMap<String, Value>,
}

/// An operation with a caller-selected correlation ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldCommand {
    pub request_id: u64,
    pub operation: WorldOperation,
}

/// The result of exactly one command, retaining its correlation ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldReply {
    pub request_id: u64,
    pub outcome: Result<WorldOutcome, String>,
}

/// A request using only owned data, suitable for crossing a thread boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorldOperation {
    Spawn,
    /// When recursive is false, surviving children are detached.
    Despawn {
        entity: Entity,
        recursive: bool,
    },
    IsAlive {
        entity: Entity,
    },
    Components {
        entity: Entity,
    },
    Parent {
        entity: Entity,
    },
    Children {
        entity: Entity,
    },
    ReadComponent {
        entity: Entity,
        name: String,
    },
    /// Read one object field, distinguishing a missing field from JSON null.
    ReadComponentField {
        entity: Entity,
        name: String,
        field: String,
    },
    /// Read only when an adapter's scoped content revision has changed.
    ReadComponentIfChanged {
        entity: Entity,
        name: String,
        revision: Option<String>,
    },
    /// Read several components of one entity; missing names are omitted.
    ReadComponents {
        entity: Entity,
        names: Vec<String>,
    },
    /// Test presence without reading or serializing the component value.
    HasComponent {
        entity: Entity,
        name: String,
    },
    WriteComponent {
        entity: Entity,
        name: String,
        value: Value,
    },
    /// Update one field on an existing object-valued component.
    WriteComponentField {
        entity: Entity,
        name: String,
        field: String,
        value: Value,
    },
    /// Update multiple fields on an existing object-valued component.
    WriteComponentFields {
        entity: Entity,
        name: String,
        fields: ComponentFields,
    },
    RemoveComponent {
        entity: Entity,
        name: String,
    },
    /// An empty component list matches every live entity.
    Query {
        components: Vec<String>,
    },
    /// Return only live entity IDs and parents, without copying component or resource data.
    Hierarchy,
    /// String revision for caching entity structure and component membership.
    StructureRevision,
    ReadResource {
        name: String,
    },
    /// Read one object field without copying the rest of a large resource.
    ReadResourceField {
        name: String,
        field: String,
    },
    WriteResource {
        name: String,
        value: Value,
    },
    RemoveResource {
        name: String,
    },
    SetParent {
        child: Entity,
        parent: Option<Entity>,
    },
    EmitEvent {
        name: String,
        value: Value,
    },
    DrainEvents {
        name: String,
    },
    /// Drain the first nonempty channel at or after start, preserving the rest.
    DrainNextEvents {
        names: Vec<String>,
        start: usize,
    },
    Snapshot,
}

/// The owned result of a successful operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorldOutcome {
    Spawned(Entity),
    Bool(bool),
    Count(usize),
    Value(Option<Value>),
    Names(Vec<String>),
    Entity(Option<Entity>),
    Entities(Vec<Entity>),
    Events(Vec<Value>),
    Snapshot(WorldSnapshot),
    Applied,
}

impl World {
    /// Builds an independent view of the explicitly exposed world state.
    ///
    /// A custom component adapter can reject serialization; in that case the
    /// error is returned and no partial snapshot is published.
    pub fn snapshot(&self) -> Result<WorldSnapshot, String> {
        let mut entities = Vec::with_capacity(self.len());
        for entity in self.entities() {
            let mut components = BTreeMap::new();
            for name in self.component_value_names(entity)? {
                if let Some(value) = self.read_component_value(entity, &name)? {
                    components.insert(name, value);
                }
            }
            entities.push(EntitySnapshot {
                entity,
                parent: self.parent(entity),
                components,
            });
        }
        let resources = self
            .dynamic_resource_names()
            .into_iter()
            .filter_map(|name| {
                self.get_dynamic_resource(&name)
                    .cloned()
                    .map(|value| (name, value))
            })
            .collect();
        Ok(WorldSnapshot {
            entities,
            resources,
        })
    }

    /// Applies a request on the owner thread and preserves its correlation ID.
    pub fn apply_command(&mut self, command: WorldCommand) -> WorldReply {
        WorldReply {
            request_id: command.request_id,
            outcome: self.apply_operation(command.operation),
        }
    }

    /// Applies one operation directly; useful for a local or channel-based adapter.
    pub fn apply_operation(&mut self, operation: WorldOperation) -> Result<WorldOutcome, String> {
        use WorldOperation as Op;
        use WorldOutcome as Out;
        match operation {
            Op::Spawn => Ok(Out::Spawned(self.spawn())),
            Op::Despawn { entity, recursive } => {
                let count = if recursive {
                    self.despawn_recursive(entity)
                } else {
                    usize::from(self.despawn(entity))
                };
                Ok(Out::Count(count))
            }
            Op::IsAlive { entity } => Ok(Out::Bool(self.is_alive(entity))),
            Op::Components { entity } => Ok(Out::Names(self.component_value_names(entity)?)),
            Op::Parent { entity } => {
                self.require_alive(entity)?;
                Ok(Out::Entity(self.parent(entity)))
            }
            Op::Children { entity } => {
                self.require_alive(entity)?;
                Ok(Out::Entities(self.children(entity)))
            }
            Op::ReadComponent { entity, name } => {
                Ok(Out::Value(self.read_component_value(entity, &name)?))
            }
            Op::ReadComponentField {
                entity,
                name,
                field,
            } => Ok(Out::Value(
                self.read_component_field(entity, &name, &field)?,
            )),
            Op::ReadComponentIfChanged {
                entity,
                name,
                revision,
            } => {
                let change = self.read_component_if_changed(entity, &name, revision.as_deref())?;
                let value = change
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| error.to_string())?;
                Ok(Out::Value(value))
            }
            Op::ReadComponents { entity, names } => Ok(Out::Value(Some(serde_json::json!(self
                .read_component_values(
                entity, &names
            )?)))),
            Op::HasComponent { entity, name } => {
                Ok(Out::Bool(self.has_component_value(entity, &name)?))
            }
            Op::WriteComponent {
                entity,
                name,
                value,
            } => {
                self.write_component_value(entity, &name, value)?;
                Ok(Out::Applied)
            }
            Op::WriteComponentField {
                entity,
                name,
                field,
                value,
            } => {
                self.write_component_field(entity, &name, &field, value)?;
                Ok(Out::Applied)
            }
            Op::WriteComponentFields {
                entity,
                name,
                fields,
            } => {
                self.write_component_fields(entity, &name, fields)?;
                Ok(Out::Applied)
            }
            Op::RemoveComponent { entity, name } => {
                Ok(Out::Value(self.remove_component_value(entity, &name)?))
            }
            Op::Query { components } => {
                let names = components.iter().map(String::as_str).collect::<Vec<_>>();
                Ok(Out::Entities(self.query_components(&names)?))
            }
            Op::Hierarchy => Ok(Out::Value(Some(serde_json::Value::Array(
                self.entities()
                    .into_iter()
                    .map(|id| {
                        serde_json::json!({
                            "id": id.to_string(),
                            "parent": self.parent(id).map(|parent| parent.to_string()),
                        })
                    })
                    .collect(),
            )))),
            Op::StructureRevision => Ok(Out::Value(Some(serde_json::json!(self
                .structure_revision()
                .to_string())))),
            Op::ReadResource { name } => Ok(Out::Value(self.get_dynamic_resource(&name).cloned())),
            Op::ReadResourceField { name, field } => Ok(Out::Value(
                self.get_dynamic_resource(&name)
                    .and_then(serde_json::Value::as_object)
                    .and_then(|object| object.get(&field))
                    .cloned(),
            )),
            Op::WriteResource { name, value } => {
                self.set_dynamic_resource(&name, value);
                Ok(Out::Applied)
            }
            Op::RemoveResource { name } => Ok(Out::Value(self.remove_dynamic_resource(&name))),
            Op::SetParent { child, parent } => {
                self.set_parent(child, parent)?;
                Ok(Out::Applied)
            }
            Op::EmitEvent { name, value } => {
                self.emit_event(&name, value);
                Ok(Out::Applied)
            }
            Op::DrainEvents { name } => Ok(Out::Events(self.drain_events(&name))),
            Op::DrainNextEvents { names, start } => Ok(Out::Value(
                self.drain_next_events(&names, start)
                    .map(|(index, events)| serde_json::json!({ "index": index, "events": events })),
            )),
            Op::Snapshot => Ok(Out::Snapshot(self.snapshot()?)),
        }
    }
}
