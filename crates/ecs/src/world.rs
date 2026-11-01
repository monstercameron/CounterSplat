//! Simulation-thread-owned ECS storage and script-neutral interfaces.
use crate::{
    store::{AnyStore, Store},
    Entity, Value,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    marker::PhantomData,
    ops::Deref,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

static NEXT_WORLD: AtomicU64 = AtomicU64::new(1);

/// Optional lifecycle notifications. Tracking is disabled until explicitly enabled on a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleEvent {
    Spawned(Entity),
    Despawned(Entity),
    ParentChanged {
        child: Entity,
        parent: Option<Entity>,
    },
}

/// Read-only access to a typed component store in deterministic entity order.
pub struct ComponentView<'a, T> {
    values: &'a BTreeMap<Entity, T>,
}

impl<T> ComponentView<'_, T> {
    /// Number of components in this view.
    pub fn len(&self) -> usize {
        self.values.len()
    }
    /// Whether the component store is empty.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    /// Borrows the component belonging to a live entity, if present.
    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.values.get(&entity)
    }
    /// Checks whether the entity has this component.
    pub fn contains(&self, entity: Entity) -> bool {
        self.values.contains_key(&entity)
    }
    /// Visits entity IDs in deterministic order.
    pub fn entities(&self) -> impl DoubleEndedIterator<Item = Entity> + ExactSizeIterator + '_ {
        self.values.keys().copied()
    }
    /// Visits entity IDs and borrowed components in deterministic order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (Entity, &T)> + '_ {
        self.values.iter().map(|(entity, value)| (*entity, value))
    }
}

/// An ordered, exact-size view of the entities in a component index.
pub type EntityIndex<'a> = Box<dyn ExactSizeIterator<Item = Entity> + 'a>;

// Compare the scoped token without allocating a new String on an unchanged read.
fn component_token_matches(
    entity: Entity,
    name: &str,
    kind: char,
    revision: u64,
    known: &str,
) -> bool {
    use std::fmt::{self, Write};
    struct Matcher<'a> {
        remaining: &'a str,
    }
    impl Write for Matcher<'_> {
        fn write_str(&mut self, part: &str) -> fmt::Result {
            self.remaining = self.remaining.strip_prefix(part).ok_or(fmt::Error)?;
            Ok(())
        }
    }
    let mut matcher = Matcher { remaining: known };
    write!(
        &mut matcher,
        "{entity}|{}:{name}|{kind}{revision}",
        name.len()
    )
    .is_ok()
        && matcher.remaining.is_empty()
}

/// Named field values for an atomic update to an object-valued component.
pub type ComponentFields = serde_json::Map<String, Value>;

/// A changed component snapshot. Use `present` to distinguish a missing
/// component from one whose JSON value is null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentChange {
    /// Opaque scoped token, or None when this adapter cannot be cached.
    pub revision: Option<String>,
    /// Whether the component exists.
    pub present: bool,
    /// The component value, or JSON null when absent.
    pub value: Value,
}

/// A narrow adapter exposing a Rust-owned component to scripting and other external consumers.
///
/// Methods execute on the world-owning thread. Adapters may validate input or expose a restricted view; they must
/// not block waiting for another subsystem. Values cross the boundary as owned JSON-compatible data.
pub trait ComponentAdapter: Send + Sync + 'static {
    /// Returns the exposed value, or None when the component is absent.
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String>;
    /// Writes the same JSON value exposed by read, returning false when absent.
    /// The default uses read; typed adapters may serialize directly to avoid an
    /// intermediate value. On error, the writer may contain partial output.
    fn write_json(
        &self,
        world: &World,
        entity: Entity,
        output: &mut Vec<u8>,
    ) -> Result<bool, String> {
        let Some(value) = self.read(world, entity)? else {
            return Ok(false);
        };
        serde_json::to_writer(output, &value).map_err(|error| error.to_string())?;
        Ok(true)
    }
    /// Validates and writes a value; return an error to reject the operation.
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String>;
    /// Updates one field of an existing object-valued component. The default
    /// reads the exposed object, changes the field, then uses the normal write
    /// path so adapter validation remains authoritative.
    fn write_field(
        &self,
        world: &mut World,
        entity: Entity,
        field: &str,
        value: Value,
    ) -> Result<(), String> {
        write_object_field_via_adapter(self, world, entity, field, value)
    }
    /// Updates several fields together, validating the combined result through
    /// the usual write path. Adapters may specialize this without committing a
    /// partial update when validation fails.
    fn write_fields(
        &self,
        world: &mut World,
        entity: Entity,
        fields: ComponentFields,
    ) -> Result<(), String> {
        write_object_fields_via_adapter(self, world, entity, fields)
    }
    /// Removes the component and returns its previous exposed value.
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String>;
    /// Reports component presence for queries and component listings.
    fn contains(&self, world: &World, entity: Entity) -> bool;
    /// A monotonic revision for every value and presence exposed by this
    /// adapter. It must change whenever any such result may change and must
    /// never reuse an earlier revision in this world. Return None when changes
    /// can occur outside the tracked world, including through interior mutation.
    fn content_revision(&self, _world: &World) -> Option<u64> {
        None
    }

    /// Optionally supplies exactly the live entities accepted by contains,
    /// once each and in entity order. A typed adapter can return
    /// Some(world.iter_with::<T>()). Return None if no exact, efficient index
    /// is available; an incomplete or unordered index would yield incorrect
    /// query results.
    fn indexed_entities<'a>(&self, _world: &'a World) -> Option<EntityIndex<'a>> {
        None
    }
}

/// Implements an adapter field write through its ordinary read/write API.
/// Specialized adapters can use this as a fallback for fields they do not
/// update directly; missing and non-object components return an error.
pub fn write_object_field_via_adapter<A: ComponentAdapter + ?Sized>(
    adapter: &A,
    world: &mut World,
    entity: Entity,
    field: &str,
    value: Value,
) -> Result<(), String> {
    let mut component = adapter.read(world, entity)?.ok_or("component is missing")?;
    let object = component
        .as_object_mut()
        .ok_or("component is not an object")?;
    object.insert(field.to_owned(), value);
    adapter.write(world, entity, component)
}

/// Implements a multi-field update through an adapter's ordinary read/write
/// API. The adapter receives a complete candidate only after all fields merge.
pub fn write_object_fields_via_adapter<A: ComponentAdapter + ?Sized>(
    adapter: &A,
    world: &mut World,
    entity: Entity,
    fields: ComponentFields,
) -> Result<(), String> {
    let mut component = adapter.read(world, entity)?.ok_or("component is missing")?;
    let object = component
        .as_object_mut()
        .ok_or("component is not an object")?;
    object.extend(fields);
    adapter.write(world, entity, component)
}

struct SerdeAdapter<T>(PhantomData<fn() -> T>);
impl<T: Serialize + DeserializeOwned + Send + 'static> ComponentAdapter for SerdeAdapter<T> {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<T>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| error.to_string())
    }
    fn write_json(
        &self,
        world: &World,
        entity: Entity,
        output: &mut Vec<u8>,
    ) -> Result<bool, String> {
        let Some(value) = world.get::<T>(entity) else {
            return Ok(false);
        };
        serde_json::to_writer(output, value).map_err(|error| error.to_string())?;
        Ok(true)
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let component = serde_json::from_value::<T>(value).map_err(|error| error.to_string())?;
        world.insert(entity, component)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        // Serialize first: a conversion error must not remove the component.
        let Some(value) = self.read(world, entity)? else {
            return Ok(None);
        };
        world.remove::<T>(entity);
        Ok(Some(value))
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<T>(entity)
    }
    fn indexed_entities<'a>(&self, world: &'a World) -> Option<EntityIndex<'a>> {
        Some(world.iter_with::<T>())
    }
}

// Every dynamic store gets a fresh world-wide sequence number on mutation.
// Removing the last value may discard the store: a later store gets a newer
// sequence number, so old tokens cannot become valid again.
struct DynamicStore {
    values: BTreeMap<Entity, Value>,
    revision: u64,
}
impl Deref for DynamicStore {
    type Target = BTreeMap<Entity, Value>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
fn next_dynamic_revision(sequence: &mut u64) -> u64 {
    if *sequence != 0 {
        *sequence = sequence.checked_add(1).unwrap_or(0);
    }
    *sequence
}

/// An ECS world owned by a single simulation worker. It is `Send` so ownership may move to that worker.
pub struct World {
    id: u64,
    generations: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,
    live_count: usize,
    structure_revision: u64,
    stores: HashMap<TypeId, Box<dyn AnyStore>>,
    resources: HashMap<TypeId, Box<dyn Any + Send>>,
    dynamic_components: BTreeMap<String, DynamicStore>,
    // Zero permanently disables dynamic cache tokens after overflow.
    dynamic_revision: u64,
    dynamic_resources: BTreeMap<String, Value>,
    adapters: BTreeMap<String, Arc<dyn ComponentAdapter>>,
    events: BTreeMap<String, VecDeque<Value>>,
    parents: BTreeMap<Entity, Entity>,
    children: BTreeMap<Entity, BTreeSet<Entity>>,
    lifecycle: Option<VecDeque<LifecycleEvent>>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
impl World {
    /// Creates an empty world with a distinct process-local identity.
    pub fn new() -> Self {
        let id = NEXT_WORLD
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("world identifier space exhausted");
        Self {
            id,
            generations: Vec::new(),
            alive: Vec::new(),
            free: Vec::new(),
            live_count: 0,
            structure_revision: 0,
            stores: HashMap::new(),
            resources: HashMap::new(),
            dynamic_components: BTreeMap::new(),
            dynamic_revision: 1,
            dynamic_resources: BTreeMap::new(),
            adapters: BTreeMap::new(),
            events: BTreeMap::new(),
            parents: BTreeMap::new(),
            children: BTreeMap::new(),
            lifecycle: None,
        }
    }

    /// Process-local world identifier used by its entity handles.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Number of living entities.
    pub fn len(&self) -> usize {
        self.live_count
    }
    /// Whether this world has no living entities.
    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    /// Revision of entity, parent, component-membership, and adapter changes.
    /// Component value edits do not change it. Custom adapters whose presence
    /// depends on values must provide their own invalidation for such changes.
    pub fn structure_revision(&self) -> u64 {
        self.structure_revision
    }

    fn structure_changed(&mut self) {
        self.structure_revision = self.structure_revision.wrapping_add(1);
    }

    /// Spawns an entity. A reused slot receives its next generation.
    pub fn spawn(&mut self) -> Entity {
        let index = if let Some(index) = self.free.pop() {
            self.alive[index as usize] = true;
            index
        } else {
            let index = u32::try_from(self.generations.len()).expect("entity slot space exhausted");
            self.generations.push(0);
            self.alive.push(true);
            index
        };
        self.live_count += 1;
        let entity = Entity {
            world: self.id,
            index,
            generation: self.generations[index as usize],
        };
        self.structure_changed();
        self.record_lifecycle(LifecycleEvent::Spawned(entity));
        entity
    }

    /// Despawns just this entity. Its children remain alive and are detached.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        for store in self.stores.values_mut() {
            store.remove(entity);
        }
        for store in self.dynamic_components.values_mut() {
            if store.values.remove(&entity).is_some() {
                store.revision = next_dynamic_revision(&mut self.dynamic_revision);
            }
        }
        self.dynamic_components.retain(|_, store| !store.is_empty());
        if let Some(parent) = self.parents.remove(&entity) {
            let empty = if let Some(children) = self.children.get_mut(&parent) {
                children.remove(&entity);
                children.is_empty()
            } else {
                false
            };
            if empty {
                self.children.remove(&parent);
            }
        }
        if let Some(children) = self.children.remove(&entity) {
            for child in children {
                self.parents.remove(&child);
                self.record_lifecycle(LifecycleEvent::ParentChanged {
                    child,
                    parent: None,
                });
            }
        }
        let slot = entity.index as usize;
        self.alive[slot] = false;
        self.live_count -= 1;
        // Never recycle an exhausted generation: stale handles must not become valid after wraparound.
        if let Some(next) = self.generations[slot].checked_add(1) {
            self.generations[slot] = next;
            self.free.push(entity.index);
        }
        self.structure_changed();
        self.record_lifecycle(LifecycleEvent::Despawned(entity));
        true
    }

    /// Despawns an entity and all descendants, returning the number removed.
    pub fn despawn_recursive(&mut self, entity: Entity) -> usize {
        if !self.is_alive(entity) {
            return 0;
        }
        let mut pending = vec![entity];
        let mut order = Vec::new();
        while let Some(next) = pending.pop() {
            pending.extend(self.children(next));
            order.push(next);
        }
        let mut removed = 0;
        for item in order.into_iter().rev() {
            removed += usize::from(self.despawn(item));
        }
        removed
    }

    /// Checks the world identity, slot and generation of a handle.
    pub fn is_alive(&self, entity: Entity) -> bool {
        let slot = entity.index as usize;
        entity.world == self.id
            && slot < self.alive.len()
            && self.alive[slot]
            && self.generations[slot] == entity.generation
    }

    /// Living entities in deterministic slot order.
    pub fn entities(&self) -> Vec<Entity> {
        self.iter_entities().collect()
    }

    /// Iterates living entities in deterministic slot order without copying them.
    pub fn iter_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive
            .iter()
            .enumerate()
            .filter(|(_, alive)| **alive)
            .map(|(index, _)| Entity {
                world: self.id,
                index: index as u32,
                generation: self.generations[index],
            })
    }

    /// Adds or replaces a typed Rust component. Components must be movable to the simulation thread.
    pub fn insert<T: Send + 'static>(
        &mut self,
        entity: Entity,
        component: T,
    ) -> Result<(), String> {
        self.require_alive(entity)?;
        let store = self.stores.entry(TypeId::of::<T>()).or_insert_with(|| {
            Box::new(Store::<T> {
                values: BTreeMap::new(),
                revision: 0,
                tracking: false,
            })
        });
        let store = store
            .as_any_mut()
            .downcast_mut::<Store<T>>()
            .expect("component store type mismatch");
        let inserted = store.values.insert(entity, component).is_none();
        Store::<T>::mark_changed(&mut store.revision);
        if inserted {
            self.structure_changed();
        }
        Ok(())
    }
    /// Borrows a typed component, or returns None if absent or stale.
    pub fn get<T: Send + 'static>(&self, entity: Entity) -> Option<&T> {
        self.typed::<T>()?.values.get(&entity)
    }
    /// Mutably borrows a typed component, or returns None if absent or stale.
    pub fn get_mut<T: Send + 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        let Store {
            values, revision, ..
        } = self.typed_mut::<T>()?;
        let value = values.get_mut(&entity)?;
        Store::<T>::mark_changed(revision);
        Some(value)
    }
    /// Checks for a typed component on an entity.
    pub fn has<T: Send + 'static>(&self, entity: Entity) -> bool {
        self.typed::<T>()
            .is_some_and(|store| store.values.contains_key(&entity))
    }
    /// Removes and returns a typed component if present.
    pub fn remove<T: Send + 'static>(&mut self, entity: Entity) -> Option<T> {
        let store = self.typed_mut::<T>()?;
        let value = store.values.remove(&entity);
        if value.is_some() {
            Store::<T>::mark_changed(&mut store.revision);
            self.structure_changed();
        }
        value
    }
    /// Enable conservative per-type change tracking. Inserts, removals,
    /// despawns, and successful mutable borrows change the revision. Call this
    /// only for types that cannot mutate through a shared reference; a mutable
    /// borrow counts even when the caller leaves the value unchanged.
    pub fn track_component_changes<T: Send + 'static>(&mut self) {
        let store = self.stores.entry(TypeId::of::<T>()).or_insert_with(|| {
            Box::new(Store::<T> {
                values: BTreeMap::new(),
                revision: 0,
                tracking: false,
            })
        });
        let store = store
            .as_any_mut()
            .downcast_mut::<Store<T>>()
            .expect("component store type mismatch");
        if !store.tracking {
            store.tracking = true;
            store.revision = 1;
        }
    }

    /// Revision of an opted-in typed store, or None when tracking is disabled
    /// or the counter has exhausted its range.
    pub fn component_revision<T: Send + 'static>(&self) -> Option<u64> {
        self.typed::<T>()
            .and_then(|store| (store.revision != 0).then_some(store.revision))
    }

    /// Borrows the component index for repeated lookups and ordered iteration.
    pub fn component_view<T: Send + 'static>(&self) -> Option<ComponentView<'_, T>> {
        self.typed::<T>().map(|store| ComponentView {
            values: &store.values,
        })
    }

    /// Borrows all components of this type in deterministic entity order.
    pub fn query<T: Send + 'static>(&self) -> Vec<(Entity, &T)> {
        self.typed::<T>().map_or_else(Vec::new, |store| {
            store
                .values
                .iter()
                .map(|(entity, value)| (*entity, value))
                .collect()
        })
    }
    /// Joins two typed component stores in deterministic entity order.
    pub fn query2<A: Send + 'static, B: Send + 'static>(&self) -> Vec<(Entity, &A, &B)> {
        let (Some(a), Some(b)) = (self.typed::<A>(), self.typed::<B>()) else {
            return Vec::new();
        };
        // Both stores are entity-ordered. Iterating the smaller one preserves
        // result order while avoiding a full scan and repeated store lookups.
        if a.values.len() <= b.values.len() {
            a.values
                .iter()
                .filter_map(|(entity, a)| b.values.get(entity).map(|b| (*entity, a, b)))
                .collect()
        } else {
            b.values
                .iter()
                .filter_map(|(entity, b)| a.values.get(entity).map(|a| (*entity, a, b)))
                .collect()
        }
    }
    /// Returns entities containing a typed component in deterministic order.
    pub fn with<T: Send + 'static>(&self) -> Vec<Entity> {
        self.typed::<T>()
            .map_or_else(Vec::new, |store| store.values.keys().copied().collect())
    }
    /// Iterates a typed component index without first copying its entity IDs.
    pub fn iter_with<T: Send + 'static>(&self) -> EntityIndex<'_> {
        match self.typed::<T>() {
            Some(store) => Box::new(store.values.keys().copied()),
            None => Box::new(std::iter::empty()),
        }
    }
    /// Rust type names present on an entity, useful for native diagnostics.
    pub fn component_names(&self, entity: Entity) -> Vec<&'static str> {
        let mut names: Vec<_> = self
            .stores
            .values()
            .filter(|store| store.has(entity))
            .map(|store| store.name())
            .collect();
        names.sort_unstable();
        names
    }

    /// Adds or replaces the one native resource of this type.
    pub fn insert_resource<R: Send + 'static>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }
    /// Borrows a native resource by type.
    pub fn resource<R: Send + 'static>(&self) -> Option<&R> {
        self.resources.get(&TypeId::of::<R>())?.downcast_ref::<R>()
    }
    /// Mutably borrows a native resource by type.
    pub fn resource_mut<R: Send + 'static>(&mut self) -> Option<&mut R> {
        self.resources
            .get_mut(&TypeId::of::<R>())?
            .downcast_mut::<R>()
    }
    /// Removes and returns a native resource by type.
    pub fn remove_resource<R: Send + 'static>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())?
            .downcast::<R>()
            .ok()
            .map(|value| *value)
    }

    /// Registers a custom script-facing view of a typed component. Existing dynamic names cannot be shadowed.
    pub fn register_component_adapter(
        &mut self,
        name: &str,
        adapter: impl ComponentAdapter,
    ) -> Result<(), String> {
        validate_name(name)?;
        if self.adapters.contains_key(name) || self.dynamic_components.contains_key(name) {
            return Err(format!("component name `{name}` is already in use"));
        }
        self.adapters.insert(name.to_owned(), Arc::new(adapter));
        self.structure_changed();
        Ok(())
    }
    /// Registers JSON serialization/deserialization for a Rust component type under a script-visible name.
    pub fn register_serde_component<T: Serialize + DeserializeOwned + Send + 'static>(
        &mut self,
        name: &str,
    ) -> Result<(), String> {
        self.register_component_adapter(name, SerdeAdapter::<T>(PhantomData))
    }
    /// Registered external component names in sorted order.
    pub fn registered_component_names(&self) -> Vec<&str> {
        self.adapters.keys().map(String::as_str).collect()
    }

    /// Stores a dynamic component. Registered typed names must use `write_component_value` instead.
    pub fn set_dynamic(&mut self, entity: Entity, name: &str, value: Value) -> Result<(), String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if self.adapters.contains_key(name) {
            return Err(format!("component `{name}` is registered; use its adapter"));
        }
        let revision = next_dynamic_revision(&mut self.dynamic_revision);
        let inserted = if let Some(store) = self.dynamic_components.get_mut(name) {
            let inserted = store.values.insert(entity, value).is_none();
            store.revision = revision;
            inserted
        } else {
            let mut values = BTreeMap::new();
            values.insert(entity, value);
            self.dynamic_components
                .insert(name.to_owned(), DynamicStore { values, revision });
            true
        };
        if inserted {
            self.structure_changed();
        }
        Ok(())
    }
    /// Borrows a dynamic value; registered native components use their adapter.
    pub fn get_dynamic(&self, entity: Entity, name: &str) -> Option<&Value> {
        if !self.is_alive(entity) {
            return None;
        }
        self.dynamic_components.get(name)?.get(&entity)
    }
    /// Removes a dynamic value, returning its previous value when present.
    pub fn remove_dynamic(&mut self, entity: Entity, name: &str) -> Option<Value> {
        if !self.is_alive(entity) {
            return None;
        }
        let store = self.dynamic_components.get_mut(name)?;
        let value = store.values.remove(&entity);
        if value.is_some() {
            store.revision = next_dynamic_revision(&mut self.dynamic_revision);
        }
        if store.is_empty() {
            self.dynamic_components.remove(name);
        }
        if value.is_some() {
            self.structure_changed();
        }
        value
    }
    /// Named dynamic components on this entity, in sorted order.
    pub fn dynamic_component_names(&self, entity: Entity) -> Vec<String> {
        if !self.is_alive(entity) {
            return Vec::new();
        }
        self.dynamic_components
            .iter()
            .filter(|(_, store)| store.contains_key(&entity))
            .map(|(name, _)| name.clone())
            .collect()
    }
    /// Matches every named dynamic component; an empty list matches all living entities.
    pub fn query_dynamic(&self, names: &[&str]) -> Vec<Entity> {
        if names.is_empty() {
            return self.entities();
        }
        // Component stores contain only live entities. Starting from the
        // smallest one avoids scanning every entity for a sparse component.
        let mut stores = Vec::with_capacity(names.len());
        for name in names {
            let Some(store) = self.dynamic_components.get(*name) else {
                return Vec::new();
            };
            stores.push(store);
        }
        let anchor = stores
            .iter()
            .enumerate()
            .min_by_key(|(_, store)| store.len())
            .map(|(index, _)| index)
            .expect("nonempty names have a store");
        stores[anchor]
            .keys()
            .copied()
            .filter(|entity| {
                stores
                    .iter()
                    .enumerate()
                    .all(|(index, store)| index == anchor || store.contains_key(entity))
            })
            .collect()
    }

    /// Reads either a dynamic component or an explicitly registered typed component.
    pub fn read_component_value(
        &self,
        entity: Entity,
        name: &str,
    ) -> Result<Option<Value>, String> {
        Ok(self
            .borrow_component_value(entity, name)?
            .map(Cow::into_owned))
    }

    /// Reads a component without cloning dynamic values. Native adapters return
    /// owned values; dynamic values remain borrowed until this world is released.
    pub fn borrow_component_value(
        &self,
        entity: Entity,
        name: &str,
    ) -> Result<Option<Cow<'_, Value>>, String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if let Some(adapter) = self.adapters.get(name) {
            Ok(adapter.read(self, entity)?.map(Cow::Owned))
        } else {
            Ok(self
                .dynamic_components
                .get(name)
                .and_then(|store| store.get(&entity))
                .map(Cow::Borrowed))
        }
    }
    pub(crate) fn is_adapted_component(&self, name: &str) -> bool {
        self.adapters.contains_key(name)
    }

    /// Writes a script-visible component as JSON without materializing a copy.
    /// Returns false when absent; errors may leave partial bytes in the writer.
    pub fn write_component_json(
        &self,
        entity: Entity,
        name: &str,
        output: &mut Vec<u8>,
    ) -> Result<bool, String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if let Some(adapter) = self.adapters.get(name) {
            adapter.write_json(self, entity, output)
        } else if let Some(value) = self
            .dynamic_components
            .get(name)
            .and_then(|store| store.get(&entity))
        {
            serde_json::to_writer(output, value).map_err(|error| error.to_string())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Returns None only when a cacheable component reports the same revision.
    /// Dynamic values are tracked by their store; untracked/custom adapters
    /// always return a fresh snapshot. Tokens are scoped to the world, entity,
    /// component name, and storage kind.
    pub fn read_component_if_changed(
        &self,
        entity: Entity,
        name: &str,
        known: Option<&str>,
    ) -> Result<Option<ComponentChange>, String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        let (kind, revision) = if let Some(adapter) = self.adapters.get(name) {
            ('a', adapter.content_revision(self))
        } else {
            (
                'd',
                self.dynamic_components
                    .get(name)
                    .and_then(|store| (self.dynamic_revision != 0).then_some(store.revision)),
            )
        };
        if let (Some(revision), Some(known)) = (revision, known) {
            if component_token_matches(entity, name, kind, revision, known) {
                return Ok(None);
            }
        }
        let revision =
            revision.map(|revision| format!("{entity}|{}:{name}|{kind}{revision}", name.len()));
        let value = self.read_component_value(entity, name)?;
        Ok(Some(ComponentChange {
            revision,
            present: value.is_some(),
            value: value.unwrap_or(Value::Null),
        }))
    }

    /// Reads several named components from one entity. Missing components are
    /// omitted; present JSON null values remain present in the returned map.
    pub fn read_component_values(
        &self,
        entity: Entity,
        names: &[String],
    ) -> Result<BTreeMap<String, Value>, String> {
        self.require_alive(entity)?;
        let mut values = BTreeMap::new();
        for name in names {
            if let Some(value) = self.read_component_value(entity, name)? {
                values.insert(name.clone(), value);
            }
        }
        Ok(values)
    }

    /// Checks script-visible presence without serializing or cloning the component.
    pub fn has_component_value(&self, entity: Entity, name: &str) -> Result<bool, String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        Ok(if let Some(adapter) = self.adapters.get(name) {
            adapter.contains(self, entity)
        } else {
            self.dynamic_components
                .get(name)
                .is_some_and(|store| store.contains_key(&entity))
        })
    }

    /// Writes a dynamic or registered native component after validating its handle and name.
    pub fn write_component_value(
        &mut self,
        entity: Entity,
        name: &str,
        value: Value,
    ) -> Result<(), String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if let Some(adapter) = self.adapters.get(name).cloned() {
            adapter.write(self, entity, value)
        } else {
            self.set_dynamic(entity, name, value)
        }
    }
    /// Updates one field of an existing script-visible object component.
    /// Dynamic components are updated in place; native adapters retain their
    /// own validation and may specialize the field write.
    pub fn write_component_field(
        &mut self,
        entity: Entity,
        name: &str,
        field: &str,
        value: Value,
    ) -> Result<(), String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if let Some(adapter) = self.adapters.get(name).cloned() {
            adapter.write_field(self, entity, field, value)
        } else {
            let store = self
                .dynamic_components
                .get_mut(name)
                .ok_or("component is missing")?;
            let object = store
                .values
                .get_mut(&entity)
                .ok_or("component is missing")?
                .as_object_mut()
                .ok_or("component is not an object")?;
            object.insert(field.to_owned(), value);
            store.revision = next_dynamic_revision(&mut self.dynamic_revision);
            Ok(())
        }
    }

    /// Updates several fields on an existing object component as one operation.
    /// Adapters can validate the combined candidate before committing it.
    pub fn write_component_fields(
        &mut self,
        entity: Entity,
        name: &str,
        fields: ComponentFields,
    ) -> Result<(), String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if let Some(adapter) = self.adapters.get(name).cloned() {
            adapter.write_fields(self, entity, fields)
        } else {
            let store = self
                .dynamic_components
                .get_mut(name)
                .ok_or("component is missing")?;
            let object = store
                .values
                .get_mut(&entity)
                .ok_or("component is missing")?
                .as_object_mut()
                .ok_or("component is not an object")?;
            object.extend(fields);
            store.revision = next_dynamic_revision(&mut self.dynamic_revision);
            Ok(())
        }
    }

    /// Removes a dynamic or registered native component and returns its previous value.
    pub fn remove_component_value(
        &mut self,
        entity: Entity,
        name: &str,
    ) -> Result<Option<Value>, String> {
        self.require_alive(entity)?;
        validate_name(name)?;
        if let Some(adapter) = self.adapters.get(name).cloned() {
            adapter.remove(self, entity)
        } else {
            Ok(self.remove_dynamic(entity, name))
        }
    }
    /// Script-visible component names, including registered typed components actually present on the entity.
    pub fn component_value_names(&self, entity: Entity) -> Result<Vec<String>, String> {
        self.require_alive(entity)?;
        let mut names = self.dynamic_component_names(entity);
        names.extend(
            self.adapters
                .iter()
                .filter(|(_, adapter)| adapter.contains(self, entity))
                .map(|(name, _)| name.clone()),
        );
        names.sort();
        Ok(names)
    }
    /// Matches all dynamic or registered typed component names; an empty list matches every live entity.
    pub fn query_components(&self, names: &[&str]) -> Result<Vec<Entity>, String> {
        // Validate every name even if an earlier component has no store.
        for name in names {
            validate_name(name)?;
        }
        if names.is_empty() {
            return Ok(self.entities());
        }
        // The physics loop and many scripts query one component per tick.
        // Avoid allocating the join's temporary store and adapter lists.
        if let [name] = names {
            if let Some(adapter) = self.adapters.get(*name) {
                return Ok(match adapter.indexed_entities(self) {
                    Some(entities) => entities.collect(),
                    None => self
                        .iter_entities()
                        .filter(|entity| adapter.contains(self, *entity))
                        .collect(),
                });
            }
            return Ok(self
                .dynamic_components
                .get(*name)
                .map_or_else(Vec::new, |store| store.keys().copied().collect()));
        }

        // Resolve each name once, rather than looking up its adapter or store
        // for every entity. Registered adapters remain the source of truth for
        // native components; dynamic stores never shadow an adapter.
        let mut stores = Vec::with_capacity(names.len());
        let mut adapters = Vec::with_capacity(names.len());
        for name in names {
            if let Some(adapter) = self.adapters.get(*name) {
                adapters.push(adapter);
            } else if let Some(store) = self.dynamic_components.get(*name) {
                stores.push(store);
            } else {
                return Ok(Vec::new());
            }
        }

        // Compare index sizes before choosing an anchor, without copying their
        // entity IDs. Custom adapters can omit an index and retain the normal
        // contains-based fallback.
        let dynamic_anchor = stores
            .iter()
            .enumerate()
            .min_by_key(|(_, store)| store.len())
            .map(|(index, store)| (index, store.len()));
        let mut best_len = dynamic_anchor.map_or(usize::MAX, |(_, len)| len);
        let mut indexed: Option<(usize, EntityIndex<'_>)> = None;
        for (index, adapter) in adapters.iter().enumerate() {
            if let Some(entities) = adapter.indexed_entities(self) {
                let len = entities.len();
                if len == 0 {
                    return Ok(Vec::new());
                }
                if len < best_len {
                    best_len = len;
                    indexed = Some((index, entities));
                }
            }
        }
        if let Some((anchor, entities)) = indexed {
            if stores.is_empty() && adapters.len() == 1 {
                return Ok(entities.collect());
            }
            Ok(entities
                .filter(|entity| {
                    stores.iter().all(|store| store.contains_key(entity))
                        && adapters.iter().enumerate().all(|(index, adapter)| {
                            index == anchor || adapter.contains(self, *entity)
                        })
                })
                .collect())
        } else if let Some((anchor, _)) = dynamic_anchor {
            // Dynamic stores only contain live handles in deterministic order.
            if stores.len() == 1 && adapters.is_empty() {
                return Ok(stores[anchor].keys().copied().collect());
            }
            Ok(stores[anchor]
                .keys()
                .copied()
                .filter(|entity| {
                    stores
                        .iter()
                        .enumerate()
                        .all(|(index, store)| index == anchor || store.contains_key(entity))
                        && adapters
                            .iter()
                            .all(|adapter| adapter.contains(self, *entity))
                })
                .collect())
        } else {
            Ok(self
                .entities()
                .into_iter()
                .filter(|entity| {
                    adapters
                        .iter()
                        .all(|adapter| adapter.contains(self, *entity))
                })
                .collect())
        }
    }

    /// Dynamic resources are separate from typed Rust resources; applications explicitly choose what scripts see.
    pub fn set_dynamic_resource(&mut self, name: &str, value: Value) {
        if let Some(existing) = self.dynamic_resources.get_mut(name) {
            *existing = value;
        } else {
            self.dynamic_resources.insert(name.to_owned(), value);
        }
    }
    /// Borrows a script-visible dynamic resource.
    pub fn get_dynamic_resource(&self, name: &str) -> Option<&Value> {
        self.dynamic_resources.get(name)
    }
    /// Removes a dynamic resource and returns its previous value.
    pub fn remove_dynamic_resource(&mut self, name: &str) -> Option<Value> {
        self.dynamic_resources.remove(name)
    }
    /// Lists dynamic resource names in sorted order.
    pub fn dynamic_resource_names(&self) -> Vec<String> {
        self.dynamic_resources.keys().cloned().collect()
    }

    /// Queues an owned event for the world thread; consumers explicitly drain each channel.
    pub fn emit_event(&mut self, name: &str, value: Value) {
        if let Some(events) = self.events.get_mut(name) {
            events.push_back(value);
        } else {
            let mut events = VecDeque::new();
            events.push_back(value);
            self.events.insert(name.to_owned(), events);
        }
    }
    /// Removes and returns every queued event for a channel in emission order.
    pub fn drain_events(&mut self, name: &str) -> Vec<Value> {
        self.events.remove(name).map_or_else(Vec::new, Vec::from)
    }
    /// Drains the first nonempty channel at or after start in caller-supplied
    /// order. Other channels remain queued, so callers can dispatch this batch
    /// before checking later channels and observe events emitted by handlers.
    pub fn drain_next_events<S: AsRef<str>>(
        &mut self,
        names: &[S],
        start: usize,
    ) -> Option<(usize, Vec<Value>)> {
        names
            .iter()
            .enumerate()
            .skip(start)
            .find_map(|(index, name)| {
                self.events
                    .remove(name.as_ref())
                    .map(|events| (index, Vec::from(events)))
            })
    }

    /// Number of events awaiting consumption on a channel.
    pub fn event_count(&self, name: &str) -> usize {
        self.events.get(name).map_or(0, VecDeque::len)
    }

    /// Sets or clears a parent. Both entities must be alive in this world and cycles are rejected.
    pub fn set_parent(&mut self, child: Entity, parent: Option<Entity>) -> Result<(), String> {
        self.require_alive(child)?;
        if let Some(parent) = parent {
            self.require_alive(parent)?;
            let mut cursor = Some(parent);
            while let Some(next) = cursor {
                if next == child {
                    return Err("parent relationship would form a cycle".into());
                }
                cursor = self.parents.get(&next).copied();
            }
        }
        let old = self.parents.get(&child).copied();
        if old == parent {
            return Ok(());
        }
        if let Some(old) = old {
            self.parents.remove(&child);
            let empty = if let Some(siblings) = self.children.get_mut(&old) {
                siblings.remove(&child);
                siblings.is_empty()
            } else {
                false
            };
            if empty {
                self.children.remove(&old);
            }
        }
        if let Some(parent) = parent {
            self.parents.insert(child, parent);
            self.children.entry(parent).or_default().insert(child);
        }
        self.structure_changed();
        self.record_lifecycle(LifecycleEvent::ParentChanged { child, parent });
        Ok(())
    }
    /// Returns the parent of a living entity, if it has one.
    pub fn parent(&self, entity: Entity) -> Option<Entity> {
        if self.is_alive(entity) {
            self.parents.get(&entity).copied()
        } else {
            None
        }
    }
    /// Returns the direct children of a living entity in deterministic order.
    pub fn children(&self, entity: Entity) -> Vec<Entity> {
        if !self.is_alive(entity) {
            return Vec::new();
        }
        self.children
            .get(&entity)
            .map_or_else(Vec::new, |children| children.iter().copied().collect())
    }
    /// Enables/disables lifecycle recording; disabling drops pending notifications.
    pub fn set_lifecycle_tracking(&mut self, enabled: bool) {
        if enabled {
            self.lifecycle.get_or_insert_with(VecDeque::new);
        } else {
            self.lifecycle = None;
        }
    }
    /// Takes lifecycle notifications recorded since the preceding drain.
    pub fn drain_lifecycle(&mut self) -> Vec<LifecycleEvent> {
        self.lifecycle
            .as_mut()
            .map_or_else(Vec::new, |events| events.drain(..).collect())
    }

    pub(crate) fn require_alive(&self, entity: Entity) -> Result<(), String> {
        if self.is_alive(entity) {
            Ok(())
        } else {
            Err(format!(
                "entity {entity} is stale or belongs to another world"
            ))
        }
    }
    fn record_lifecycle(&mut self, event: LifecycleEvent) {
        if let Some(events) = self.lifecycle.as_mut() {
            events.push_back(event);
        }
    }
    fn typed<T: Send + 'static>(&self) -> Option<&Store<T>> {
        self.stores
            .get(&TypeId::of::<T>())?
            .as_any()
            .downcast_ref::<Store<T>>()
    }
    fn typed_mut<T: Send + 'static>(&mut self) -> Option<&mut Store<T>> {
        self.stores
            .get_mut(&TypeId::of::<T>())?
            .as_any_mut()
            .downcast_mut::<Store<T>>()
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 256
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        Err("component name must be 1–256 bytes without surrounding whitespace or control characters".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    #[test]
    fn scoped_token_matching_handles_unicode_and_delimiters() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        for name in ["plain", "quoted\\\"|:é", "long|0:name"] {
            let token = format!("{first}|{}:{name}|d{}", name.len(), 123);
            assert!(component_token_matches(first, name, 'd', 123, &token));
            assert!(!component_token_matches(first, name, 'a', 123, &token));
            assert!(!component_token_matches(second, name, 'd', 123, &token));
            assert!(!component_token_matches(first, name, 'd', 124, &token));
            assert!(!component_token_matches(
                first,
                name,
                'd',
                123,
                &(token.clone() + "extra")
            ));
            assert!(!component_token_matches(first, "other", 'd', 123, &token));
        }
    }
    #[test]
    fn exhausted_dynamic_sequence_disables_every_cached_store() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .set_dynamic(entity, "First", serde_json::json!(1))
            .unwrap();
        world
            .set_dynamic(entity, "Second", serde_json::json!(2))
            .unwrap();
        let first = world
            .read_component_if_changed(entity, "First", None)
            .unwrap()
            .unwrap()
            .revision
            .unwrap();
        let second = world
            .read_component_if_changed(entity, "Second", None)
            .unwrap()
            .unwrap()
            .revision
            .unwrap();
        world.dynamic_revision = u64::MAX;
        world
            .set_dynamic(entity, "First", serde_json::json!(3))
            .unwrap();
        for (name, old) in [("First", first), ("Second", second)] {
            let change = world
                .read_component_if_changed(entity, name, Some(&old))
                .unwrap()
                .unwrap();
            assert!(change.revision.is_none());
        }
        world
            .set_dynamic(entity, "First", serde_json::json!(4))
            .unwrap();
        assert!(world
            .read_component_if_changed(entity, "First", None)
            .unwrap()
            .unwrap()
            .revision
            .is_none());
    }
    #[test]
    fn exhausted_revisions_never_reuse_old_tokens() {
        let mut world = World::new();
        let entity = world.spawn();
        world.track_component_changes::<u32>();
        world.typed_mut::<u32>().unwrap().revision = u64::MAX;
        world.insert(entity, 1_u32).unwrap();
        assert_eq!(world.component_revision::<u32>(), None);
        world.track_component_changes::<u32>();
        world.insert(entity, 2_u32).unwrap();
        assert_eq!(world.component_revision::<u32>(), None);
    }
}
