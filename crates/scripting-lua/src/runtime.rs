use crate::WorldBridge;
use countersplat_ecs::{Entity, WorldOperation, WorldOutcome};
use mlua::{
    FromLuaMulti, Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, LuaSerdeExt, StdLib,
    Table, Value, VmState,
};
use serde_json::Value as JsonValue;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    str::FromStr,
};

const SYSTEMS: &str = "countersplat.lua.systems";
const HANDLERS: &str = "countersplat.lua.handlers";
const HOOK_INTERVAL: u32 = 1_000;
type Bridge = Rc<RefCell<Box<dyn WorldBridge>>>;
// Readers hold an immutable snapshot; registration copies only while a pass is active.
type SystemOrder = Rc<RefCell<Rc<Vec<String>>>>;

/// Per-VM execution controls. Set an option to None to remove that limit.
///
/// Instruction limits are approximate, checked every 1,000 Lua instructions,
/// and reset for each host call. They do not bound time spent in host callbacks
/// or waiting for another thread.
#[derive(Clone, Debug)]
pub struct ScriptOptions {
    /// Maximum memory used by the Lua allocator.
    pub memory_limit_bytes: Option<usize>,
    /// Approximate Lua instruction budget per host call.
    pub instruction_limit: Option<u64>,
    /// Maximum number of pending commands submitted to a [crate::ScriptWorker].
    pub worker_queue_capacity: usize,
}

impl Default for ScriptOptions {
    fn default() -> Self {
        Self {
            memory_limit_bytes: Some(64 * 1024 * 1024),
            instruction_limit: Some(10_000_000),
            worker_queue_capacity: 64,
        }
    }
}

/// A thread-local Lua VM and its bridge to the ECS owner.
///
/// Construct this inside its owner thread or use [crate::ScriptWorker], which
/// constructs it there for you. The Lua VM is intentionally not Send.
pub struct LuaRuntime {
    lua: Lua,
    system_order: SystemOrder,
    budget: Option<(Rc<Cell<u64>>, u64)>,
}

impl LuaRuntime {
    /// Create a runtime using the supplied world bridge and execution controls.
    pub fn new(bridge: impl WorldBridge + 'static, options: ScriptOptions) -> mlua::Result<Self> {
        if options.instruction_limit == Some(0)
            || options.memory_limit_bytes == Some(0)
            || options.worker_queue_capacity == 0
        {
            return Err(mlua::Error::runtime(
                "script limits must be greater than zero",
            ));
        }
        // Coroutine hooks are separate in Lua, so do not expose coroutines
        // while relying on the main-thread instruction hook.
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH,
            LuaOptions::default(),
        )?;
        if let Some(limit) = options.memory_limit_bytes {
            lua.set_memory_limit(limit)?;
        }
        let globals = lua.globals();
        // These base-library functions can access the host filesystem.
        globals.set("dofile", Value::Nil)?;
        globals.set("loadfile", Value::Nil)?;
        lua.set_named_registry_value(SYSTEMS, lua.create_table()?)?;
        lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
        let order = Rc::new(RefCell::new(Rc::new(Vec::new())));
        let bridge: Bridge = Rc::new(RefCell::new(Box::new(bridge)));
        install_ecs_api(&lua, &bridge, &order)?;
        lua.load(include_str!("engine_api.lua"))
            .set_name("engine_api")
            .exec()?;
        let budget = if let Some(limit) = options.instruction_limit {
            let remaining = Rc::new(Cell::new(limit));
            let hook_remaining = Rc::clone(&remaining);
            lua.set_hook(
                HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
                move |_, _| {
                    let left = hook_remaining.get();
                    if left <= HOOK_INTERVAL as u64 {
                        return Err(mlua::Error::runtime("Lua instruction budget exceeded"));
                    }
                    hook_remaining.set(left - HOOK_INTERVAL as u64);
                    Ok(VmState::Continue)
                },
            )?;
            Some((remaining, limit))
        } else {
            None
        };
        Ok(Self {
            lua,
            system_order: order,
            budget,
        })
    }

    fn reset_budget(&self) {
        if let Some((remaining, limit)) = &self.budget {
            remaining.set(*limit);
        }
    }

    /// Execute a script in the VM's persistent global environment.
    pub fn execute(&self, source: &str) -> mlua::Result<()> {
        self.reset_budget();
        self.lua.load(source).set_name("script").exec()
    }

    /// System names in their registration order.
    pub fn systems(&self) -> Vec<String> {
        self.system_order.borrow().as_ref().clone()
    }

    /// Run one named system. The callback receives delta time in seconds.
    pub fn run_system(&self, name: &str, dt: f64) -> mlua::Result<()> {
        validate_dt(dt)?;
        self.reset_budget();
        let systems: Table = self.lua.named_registry_value(SYSTEMS)?;
        let callback: Option<Function> = systems.get(name)?;
        callback
            .ok_or_else(|| mlua::Error::runtime(format!("unknown Lua system '{name}'")))?
            .call(dt)
    }

    /// Run a snapshot of registered system names in registration order.
    ///
    /// Systems removed during a run are skipped; newly registered systems run
    /// on the next call. A failure stops the current pass and is returned.
    pub fn run_systems(&self, dt: f64) -> mlua::Result<()> {
        validate_dt(dt)?;
        self.reset_budget();
        let names = Rc::clone(&self.system_order.borrow());
        let systems: Table = self.lua.named_registry_value(SYSTEMS)?;
        for name in names.iter() {
            let callback: Option<Function> = systems.get(name.as_str())?;
            if let Some(callback) = callback {
                callback.call::<()>(dt)?;
            }
        }
        Ok(())
    }

    /// Deliver a host event to all registered Lua callbacks, in registration order.
    ///
    /// The handler list is snapshotted before dispatch. This does not consume
    /// ECS event queues; use the Lua ecs.drain API for those.
    pub fn dispatch(&self, name: &str, payload: &JsonValue) -> mlua::Result<()> {
        self.reset_budget();
        let handlers: Table = self.lua.named_registry_value(HANDLERS)?;
        let Some(list) = handlers.get::<Option<Table>>(name)? else {
            return Ok(());
        };
        let callbacks = list
            .sequence_values::<Function>()
            .collect::<mlua::Result<Vec<_>>>()?;
        for callback in callbacks {
            callback.call::<()>(self.lua.to_value(payload)?)?;
        }
        Ok(())
    }
}

fn validate_dt(dt: f64) -> mlua::Result<()> {
    if dt.is_finite() && dt >= 0.0 {
        Ok(())
    } else {
        Err(mlua::Error::runtime(
            "delta time must be finite and nonnegative",
        ))
    }
}

fn bind<A, R, F>(lua: &Lua, api: &Table, bridge: &Bridge, name: &str, f: F) -> mlua::Result<()>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&Lua, &Bridge, A) -> mlua::Result<R> + 'static,
{
    let bridge = Rc::clone(bridge);
    api.set(
        name,
        lua.create_function(move |lua, args| f(lua, &bridge, args))?,
    )
}

fn request(bridge: &Bridge, operation: WorldOperation) -> mlua::Result<WorldOutcome> {
    bridge
        .try_borrow_mut()
        .map_err(|_| mlua::Error::runtime("ECS bridge is already in use"))?
        .request(operation)
        .map_err(mlua::Error::runtime)
}

fn entity(id: String) -> mlua::Result<Entity> {
    Entity::from_str(&id)
        .map_err(|error| mlua::Error::runtime(format!("invalid entity ID '{id}': {error}")))
}

fn unexpected() -> mlua::Error {
    mlua::Error::runtime("ECS bridge returned an unexpected response")
}
fn applied(outcome: WorldOutcome) -> mlua::Result<()> {
    if matches!(outcome, WorldOutcome::Applied) {
        Ok(())
    } else {
        Err(unexpected())
    }
}
fn maybe_value(lua: &Lua, outcome: WorldOutcome) -> mlua::Result<Value> {
    match outcome {
        WorldOutcome::Value(Some(value)) => lua.to_value(&value),
        WorldOutcome::Value(None) => Ok(Value::Nil),
        _ => Err(unexpected()),
    }
}
// serde_json maps NaN and infinity to null. Reject them before conversion so a
// script cannot silently corrupt a component or resource. Limit nested tables
// and detect cycles before recursive deserialization.
fn json_input(lua: &Lua, value: Value) -> mlua::Result<JsonValue> {
    // Primitive values cannot contain cycles or exceed the nesting/node limits.
    // Preserve the general decoder for binary strings and every compound value.
    match &value {
        Value::Nil => return Ok(JsonValue::Null),
        Value::LightUserData(pointer) if pointer.0.is_null() => return Ok(JsonValue::Null),
        Value::Boolean(value) => return Ok(JsonValue::Bool(*value)),
        Value::Integer(value) => return Ok(JsonValue::Number((*value).into())),
        Value::Number(value) => {
            return serde_json::Number::from_f64(*value)
                .map(JsonValue::Number)
                .ok_or_else(|| mlua::Error::runtime("JSON values must use finite numbers"));
        }
        Value::String(value) => {
            if let Ok(text) = value.to_str() {
                return Ok(JsonValue::String(String::from(&*text)));
            }
        }
        _ => {}
    }
    json_input_general(lua, value)
}

// Most values have only a few nested tables. Track their active ancestors on
// the stack, then use a set for deeper inputs so cycle checks remain bounded.
struct ActiveTables {
    inline: [usize; 8],
    len: usize,
    deep: Option<HashSet<usize>>,
}

impl ActiveTables {
    fn new() -> Self {
        Self {
            inline: [0; 8],
            len: 0,
            deep: None,
        }
    }

    fn insert(&mut self, address: usize) -> bool {
        if let Some(deep) = &mut self.deep {
            return deep.insert(address);
        }
        if self.inline[..self.len].contains(&address) {
            return false;
        }
        if self.len < self.inline.len() {
            self.inline[self.len] = address;
            self.len += 1;
        } else {
            let mut deep = HashSet::with_capacity(self.len * 2);
            deep.extend(self.inline);
            deep.insert(address);
            self.deep = Some(deep);
        }
        true
    }

    fn remove(&mut self, address: usize) {
        if let Some(deep) = &mut self.deep {
            deep.remove(&address);
        } else {
            self.len -= 1;
            debug_assert_eq!(self.inline[self.len], address);
        }
    }
}

fn json_input_general(lua: &Lua, value: Value) -> mlua::Result<JsonValue> {
    fn validate(
        value: &Value,
        active: &mut ActiveTables,
        depth: usize,
        visited: &mut usize,
    ) -> mlua::Result<()> {
        *visited += 1;
        if *visited > 100_000 {
            return Err(mlua::Error::runtime(
                "JSON values cannot exceed 100,000 nodes",
            ));
        }
        if depth > 128 {
            return Err(mlua::Error::runtime(
                "JSON table nesting exceeds 128 levels",
            ));
        }
        match value {
            Value::Number(number) if !number.is_finite() => {
                return Err(mlua::Error::runtime("JSON values must use finite numbers"));
            }
            Value::Table(table) => {
                let address = table.to_pointer() as usize;
                if !active.insert(address) {
                    return Err(mlua::Error::runtime(
                        "JSON values cannot contain cyclic tables",
                    ));
                }
                for entry in table.clone().pairs::<Value, Value>() {
                    let (_, item) = entry?;
                    validate(&item, active, depth + 1, visited)?;
                }
                active.remove(address);
            }
            _ => {}
        }
        Ok(())
    }
    let mut visited = 0;
    validate(&value, &mut ActiveTables::new(), 0, &mut visited)?;
    lua.from_value(value)
}
fn entity_strings(entities: Vec<Entity>) -> Vec<String> {
    entities.into_iter().map(|id| id.to_string()).collect()
}

fn install_ecs_api(lua: &Lua, bridge: &Bridge, order: &SystemOrder) -> mlua::Result<()> {
    let api = lua.create_table()?;
    api.set("null", lua.null())?;
    api.set(
        "array",
        lua.create_function(|lua, ()| {
            let result = lua.create_table()?;
            result.set_metatable(Some(lua.array_metatable()))?;
            Ok(result)
        })?,
    )?;

    bind(lua, &api, bridge, "spawn", |_, bridge, ()| {
        match request(bridge, WorldOperation::Spawn)? {
            WorldOutcome::Spawned(id) => Ok(id.to_string()),
            _ => Err(unexpected()),
        }
    })?;
    bind(
        lua,
        &api,
        bridge,
        "despawn",
        |_, bridge, (id, recursive): (String, Option<bool>)| match request(
            bridge,
            WorldOperation::Despawn {
                entity: entity(id)?,
                recursive: recursive.unwrap_or(false),
            },
        )? {
            WorldOutcome::Count(count) => Ok(count),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "is_alive",
        |_, bridge, id: String| match request(
            bridge,
            WorldOperation::IsAlive {
                entity: entity(id)?,
            },
        )? {
            WorldOutcome::Bool(value) => Ok(value),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "entities",
        |lua, bridge, ()| match request(
            bridge,
            WorldOperation::Query {
                components: Vec::new(),
            },
        )? {
            WorldOutcome::Entities(ids) => lua.to_value(&entity_strings(ids)),
            _ => Err(unexpected()),
        },
    )?;
    bind(lua, &api, bridge, "hierarchy", |lua, bridge, ()| {
        maybe_value(lua, request(bridge, WorldOperation::Hierarchy)?)
    })?;
    bind(
        lua,
        &api,
        bridge,
        "structure_revision",
        |_, bridge, ()| match request(bridge, WorldOperation::StructureRevision)? {
            WorldOutcome::Value(Some(JsonValue::String(revision))) => Ok(revision),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "snapshot",
        |lua, bridge, ()| match request(bridge, WorldOperation::Snapshot)? {
            WorldOutcome::Snapshot(snapshot) => {
                let result = lua.create_table()?;
                let entities = lua.create_table()?;
                entities.set_metatable(Some(lua.array_metatable()))?;
                for (index, item) in snapshot.entities.into_iter().enumerate() {
                    let entry = lua.create_table()?;
                    entry.set("id", item.entity.to_string())?;
                    entry.set("parent", item.parent.map(|id| id.to_string()))?;
                    entry.set("components", lua.to_value(&item.components)?)?;
                    entities.raw_set(index + 1, entry)?;
                }
                result.set("entities", entities)?;
                result.set("resources", lua.to_value(&snapshot.resources)?)?;
                Ok(result)
            }
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "components",
        |lua, bridge, id: String| match request(
            bridge,
            WorldOperation::Components {
                entity: entity(id)?,
            },
        )? {
            WorldOutcome::Names(names) => lua.to_value(&names),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "query",
        |lua, bridge, names: Vec<String>| match request(
            bridge,
            WorldOperation::Query { components: names },
        )? {
            WorldOutcome::Entities(ids) => lua.to_value(&entity_strings(ids)),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "get",
        |lua, bridge, (id, name): (String, String)| {
            maybe_value(
                lua,
                request(
                    bridge,
                    WorldOperation::ReadComponent {
                        entity: entity(id)?,
                        name,
                    },
                )?,
            )
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "get_if_changed",
        |lua, bridge, (id, name, revision): (String, String, Option<String>)| {
            maybe_value(
                lua,
                request(
                    bridge,
                    WorldOperation::ReadComponentIfChanged {
                        entity: entity(id)?,
                        name,
                        revision,
                    },
                )?,
            )
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "get_many",
        |lua, bridge, (id, names): (String, Vec<String>)| {
            maybe_value(
                lua,
                request(
                    bridge,
                    WorldOperation::ReadComponents {
                        entity: entity(id)?,
                        names,
                    },
                )?,
            )
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "has",
        |_, bridge, (id, name): (String, String)| match request(
            bridge,
            WorldOperation::HasComponent {
                entity: entity(id)?,
                name,
            },
        )? {
            WorldOutcome::Bool(value) => Ok(value),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "set",
        |lua, bridge, (id, name, value): (String, String, Value)| {
            applied(request(
                bridge,
                WorldOperation::WriteComponent {
                    entity: entity(id)?,
                    name,
                    value: json_input(lua, value)?,
                },
            )?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "set_field",
        |lua, bridge, (id, name, field, value): (String, String, String, Value)| {
            applied(request(
                bridge,
                WorldOperation::WriteComponentField {
                    entity: entity(id)?,
                    name,
                    field,
                    value: json_input(lua, value)?,
                },
            )?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "set_fields",
        |lua, bridge, (id, name, fields): (String, String, Value)| {
            let JsonValue::Object(fields) = json_input(lua, fields)? else {
                return Err(mlua::Error::runtime("component fields must be an object"));
            };
            applied(request(
                bridge,
                WorldOperation::WriteComponentFields {
                    entity: entity(id)?,
                    name,
                    fields,
                },
            )?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "remove",
        |lua, bridge, (id, name): (String, String)| {
            maybe_value(
                lua,
                request(
                    bridge,
                    WorldOperation::RemoveComponent {
                        entity: entity(id)?,
                        name,
                    },
                )?,
            )
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "get_resource",
        |lua, bridge, name: String| {
            maybe_value(lua, request(bridge, WorldOperation::ReadResource { name })?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "get_resource_field",
        |lua, bridge, (name, field): (String, String)| {
            maybe_value(
                lua,
                request(bridge, WorldOperation::ReadResourceField { name, field })?,
            )
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "set_resource",
        |lua, bridge, (name, value): (String, Value)| {
            applied(request(
                bridge,
                WorldOperation::WriteResource {
                    name,
                    value: json_input(lua, value)?,
                },
            )?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "remove_resource",
        |lua, bridge, name: String| {
            maybe_value(
                lua,
                request(bridge, WorldOperation::RemoveResource { name })?,
            )
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "emit",
        |lua, bridge, (name, value): (String, Value)| {
            applied(request(
                bridge,
                WorldOperation::EmitEvent {
                    name,
                    value: json_input(lua, value)?,
                },
            )?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "drain",
        |lua, bridge, name: String| match request(bridge, WorldOperation::DrainEvents { name })? {
            WorldOutcome::Events(events) => lua.to_value(&events),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "set_parent",
        |_, bridge, (child, parent): (String, Option<String>)| {
            applied(request(
                bridge,
                WorldOperation::SetParent {
                    child: entity(child)?,
                    parent: parent.map(entity).transpose()?,
                },
            )?)
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "parent",
        |_, bridge, id: String| match request(
            bridge,
            WorldOperation::Parent {
                entity: entity(id)?,
            },
        )? {
            WorldOutcome::Entity(parent) => Ok(parent.map(|id| id.to_string())),
            _ => Err(unexpected()),
        },
    )?;
    bind(
        lua,
        &api,
        bridge,
        "children",
        |lua, bridge, id: String| match request(
            bridge,
            WorldOperation::Children {
                entity: entity(id)?,
            },
        )? {
            WorldOutcome::Entities(children) => lua.to_value(&entity_strings(children)),
            _ => Err(unexpected()),
        },
    )?;

    let system_order = Rc::clone(order);
    api.set(
        "system",
        lua.create_function(move |lua, (name, callback): (String, Function)| {
            if name.trim().is_empty() {
                return Err(mlua::Error::runtime("system name cannot be empty"));
            }
            let mut order = system_order
                .try_borrow_mut()
                .map_err(|_| mlua::Error::runtime("system registry is in use"))?;
            let systems: Table = lua.named_registry_value(SYSTEMS)?;
            systems.set(name.as_str(), callback)?;
            if !order.contains(&name) {
                Rc::make_mut(&mut order).push(name);
            }
            Ok(())
        })?,
    )?;
    let system_order = Rc::clone(order);
    api.set(
        "remove_system",
        lua.create_function(move |lua, name: String| {
            let mut order = system_order
                .try_borrow_mut()
                .map_err(|_| mlua::Error::runtime("system registry is in use"))?;
            let systems: Table = lua.named_registry_value(SYSTEMS)?;
            let existed = order.contains(&name);
            systems.set(name.as_str(), Value::Nil)?;
            if existed {
                Rc::make_mut(&mut order).retain(|item| item != &name);
            }
            Ok(existed)
        })?,
    )?;
    api.set(
        "on",
        lua.create_function(|lua, (name, callback): (String, Function)| {
            if name.trim().is_empty() {
                return Err(mlua::Error::runtime("event name cannot be empty"));
            }
            let handlers: Table = lua.named_registry_value(HANDLERS)?;
            let list = match handlers.get::<Option<Table>>(name.as_str())? {
                Some(list) => list,
                None => {
                    let list = lua.create_table()?;
                    handlers.set(name, list.clone())?;
                    list
                }
            };
            list.raw_set(list.raw_len() + 1, callback)?;
            Ok(())
        })?,
    )?;
    api.set(
        "off",
        lua.create_function(|lua, name: String| {
            let handlers: Table = lua.named_registry_value(HANDLERS)?;
            handlers.set(name, Value::Nil)
        })?,
    )?;
    lua.globals().set("ecs", api)
}

#[cfg(test)]
mod scalar_input_tests {
    use super::*;

    // Retain the previous implementation as an independent comparison for
    // correctness and the opt-in nested-input benchmark.
    fn json_input_hashset_reference(lua: &Lua, value: Value) -> mlua::Result<JsonValue> {
        fn validate(
            value: &Value,
            active: &mut HashSet<usize>,
            depth: usize,
            visited: &mut usize,
        ) -> mlua::Result<()> {
            *visited += 1;
            if *visited > 100_000 {
                return Err(mlua::Error::runtime(
                    "JSON values cannot exceed 100,000 nodes",
                ));
            }
            if depth > 128 {
                return Err(mlua::Error::runtime(
                    "JSON table nesting exceeds 128 levels",
                ));
            }
            match value {
                Value::Number(number) if !number.is_finite() => {
                    return Err(mlua::Error::runtime("JSON values must use finite numbers"))
                }
                Value::Table(table) => {
                    let address = table.to_pointer() as usize;
                    if !active.insert(address) {
                        return Err(mlua::Error::runtime(
                            "JSON values cannot contain cyclic tables",
                        ));
                    }
                    for entry in table.clone().pairs::<Value, Value>() {
                        let (_, item) = entry?;
                        validate(&item, active, depth + 1, visited)?;
                    }
                    active.remove(&address);
                }
                _ => {}
            }
            Ok(())
        }
        let mut visited = 0;
        validate(&value, &mut HashSet::new(), 0, &mut visited)?;
        lua.from_value(value)
    }

    #[test]
    fn nested_input_matches_hashset_validation() {
        let lua = Lua::new();
        let fixtures = [
            ("return {1,2,3}", true),
            ("local child={x=1}; return {a=child, b=child}", true),
            ("local value={leaf=true}; for i=1,7 do value={child=value} end; return value", true),
            ("local value={leaf=true}; for i=1,8 do value={child=value} end; return value", true),
            ("local value={leaf=true}; for i=1,9 do value={child=value} end; return value", true),
            ("local value={leaf=true}; for i=1,32 do value={child=value} end; return {a=value,b={x=1}}", true),
            ("local root={}; local child=root; for i=1,32 do child.next={}; child=child.next end; child.loop=root; return root", false),
            ("local value={}; for i=1,130 do value={child=value} end; return value", false),
            ("local value={leaf=true}; for i=1,18 do value={value,value} end; return value", false),
            ("return {number=0/0}", false),
        ];
        for (source, valid) in fixtures {
            let value: Value = lua.load(source).eval().unwrap();
            let expected =
                json_input_hashset_reference(&lua, value.clone()).map_err(|e| e.to_string());
            assert_eq!(expected.is_ok(), valid, "{source}");
            let actual = json_input_general(&lua, value).map_err(|e| e.to_string());
            assert_eq!(actual, expected, "{source}");
        }
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_nested_input() {
        use std::{hint::black_box, time::Instant};
        let lua = Lua::new();
        let fixtures = [
            ("vector", "return {1.25,2.5,-3.75}", 100_000),
            (
                "object",
                "return {position={1.25,2.5,-3.75},enabled=true}",
                50_000,
            ),
            (
                "wide",
                "local a={}; for i=1,1000 do a[i]={x=i,y=2} end; return a",
                50,
            ),
            (
                "deep",
                "local a={leaf=true}; for i=1,96 do a={child=a} end; return a",
                500,
            ),
        ];
        for (name, source, iterations) in fixtures {
            let value: Value = lua.load(source).eval().unwrap();
            let mut samples = [Vec::new(), Vec::new()];
            for round in 0..9 {
                for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let start = Instant::now();
                    for _ in 0..iterations {
                        let result = if mode == 0 {
                            json_input_hashset_reference(black_box(&lua), black_box(value.clone()))
                        } else {
                            json_input_general(black_box(&lua), black_box(value.clone()))
                        };
                        black_box(result.unwrap());
                    }
                    samples[mode].push(start.elapsed().as_nanos() as f64 / iterations as f64);
                }
            }
            samples[0].sort_by(f64::total_cmp);
            samples[1].sort_by(f64::total_cmp);
            println!(
                "{name}: hash set {:.0} ns, inline {:.0} ns",
                samples[0][4], samples[1][4]
            );
        }
    }

    #[test]
    fn primitive_inputs_match_the_general_decoder() {
        let lua = Lua::new();
        let array = lua.create_sequence_from([1, 2, 3]).unwrap();
        let cycle = lua.create_table().unwrap();
        cycle.set("self", cycle.clone()).unwrap();
        let cases = vec![
            Value::Nil,
            lua.null(),
            Value::Boolean(false),
            Value::Boolean(true),
            Value::Integer(i64::MIN),
            Value::Integer(0),
            Value::Integer(i64::MAX),
            Value::Number(-0.0),
            Value::Number(f64::MIN_POSITIVE),
            Value::Number(f64::MAX),
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(f64::NEG_INFINITY),
            Value::String(lua.create_string("").unwrap()),
            Value::String(lua.create_string("héllo 🌍").unwrap()),
            Value::String(lua.create_string([0xff, 0x80]).unwrap()),
            Value::Table(array),
            Value::Table(cycle),
            Value::Function(lua.create_function(|_, ()| Ok(())).unwrap()),
        ];
        for value in cases {
            let expected = json_input_general(&lua, value.clone())
                .map(|v| serde_json::to_string(&v).unwrap())
                .map_err(|e| e.to_string());
            let actual = json_input(&lua, value)
                .map(|v| serde_json::to_string(&v).unwrap())
                .map_err(|e| e.to_string());
            assert_eq!(actual, expected);
        }
        // Include normal, subnormal, signed, and non-finite bit patterns.
        let mut bits = 0x86f0_0137_7be2_91adu64;
        for _ in 0..10_000 {
            bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
            let value = Value::Number(f64::from_bits(bits));
            let expected = json_input_general(&lua, value.clone())
                .map(|v| serde_json::to_string(&v).unwrap())
                .map_err(|e| e.to_string());
            let actual = json_input(&lua, value)
                .map(|v| serde_json::to_string(&v).unwrap())
                .map_err(|e| e.to_string());
            assert_eq!(actual, expected, "bits {bits:016x}");
        }
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_scalar_input() {
        use std::{hint::black_box, time::Instant};
        let lua = Lua::new();
        let values = [
            ("boolean", Value::Boolean(false)),
            ("integer", Value::Integer(42)),
            ("number", Value::Number(1.25)),
            ("string", Value::String(lua.create_string("value").unwrap())),
            (
                "vector",
                Value::Table(lua.create_sequence_from([1.0, 2.0, 3.0]).unwrap()),
            ),
        ];
        for (name, value) in values {
            let mut samples = [Vec::new(), Vec::new()];
            for round in 0..9 {
                for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let start = Instant::now();
                    for _ in 0..100_000 {
                        let result = if mode == 0 {
                            json_input_general(black_box(&lua), black_box(value.clone()))
                        } else {
                            json_input(black_box(&lua), black_box(value.clone()))
                        };
                        black_box(result.unwrap());
                    }
                    samples[mode].push(start.elapsed().as_nanos() as f64 / 100_000.0);
                }
            }
            samples[0].sort_by(f64::total_cmp);
            samples[1].sort_by(f64::total_cmp);
            println!(
                "{name}: general {:.1} ns, direct {:.1} ns",
                samples[0][4], samples[1][4]
            );
        }
    }
}

#[cfg(test)]
mod system_schedule_tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    struct Sink;
    impl WorldBridge for Sink {
        fn request(&mut self, _: WorldOperation) -> Result<WorldOutcome, String> {
            Ok(WorldOutcome::Applied)
        }
    }

    fn clone_and_run(runtime: &LuaRuntime, dt: f64) -> mlua::Result<()> {
        validate_dt(dt)?;
        runtime.reset_budget();
        let names = runtime.systems();
        let systems: Table = runtime.lua.named_registry_value(SYSTEMS)?;
        for name in names {
            let callback: Option<Function> = systems.get(name)?;
            if let Some(callback) = callback {
                callback.call::<()>(dt)?;
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_system_schedule() {
        for (count, iterations) in [
            (0, 20_000),
            (1, 20_000),
            (16, 5_000),
            (100, 1_000),
            (1000, 100),
        ] {
            let runtime = LuaRuntime::new(
                Sink,
                ScriptOptions {
                    instruction_limit: None,
                    ..ScriptOptions::default()
                },
            )
            .unwrap();
            runtime
                .execute(&format!(
                    r#"for i=1,{count} do
                ecs.system("system_"..i, function(dt) end)
            end"#
                ))
                .unwrap();
            let mut samples = [Vec::new(), Vec::new()];
            for round in 0..9 {
                for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let start = Instant::now();
                    for _ in 0..iterations {
                        if mode == 0 {
                            black_box(clone_and_run(&runtime, black_box(1.0 / 60.0)).unwrap());
                        } else {
                            black_box(runtime.run_systems(black_box(1.0 / 60.0)).unwrap());
                        }
                    }
                    samples[mode].push(start.elapsed().as_nanos() as f64 / iterations as f64);
                }
            }
            samples[0].sort_by(f64::total_cmp);
            samples[1].sort_by(f64::total_cmp);
            println!(
                "{count} systems: cloned {:.0} ns, shared {:.0} ns",
                samples[0][4], samples[1][4]
            );
        }
    }
}
