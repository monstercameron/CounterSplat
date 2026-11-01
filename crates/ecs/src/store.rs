//! Type-erased component storage; every concrete store is owned by the simulation thread.
use crate::Entity;
use std::{any::Any, collections::BTreeMap};

pub(crate) trait AnyStore: Any + Send {
    fn remove(&mut self, entity: Entity) -> bool;
    fn has(&self, entity: Entity) -> bool;
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub(crate) struct Store<T: Send + 'static> {
    pub values: BTreeMap<Entity, T>,
    // Zero means disabled or exhausted. Once enabled, overflow permanently
    // disables cache tokens for this store rather than reusing an old token.
    pub revision: u64,
    pub tracking: bool,
}
impl<T: Send + 'static> Store<T> {
    pub fn mark_changed(revision: &mut u64) {
        if *revision != 0 {
            *revision = revision.checked_add(1).unwrap_or(0);
        }
    }
}
impl<T: Send + 'static> AnyStore for Store<T> {
    fn remove(&mut self, entity: Entity) -> bool {
        let removed = self.values.remove(&entity).is_some();
        if removed {
            Self::mark_changed(&mut self.revision);
        }
        removed
    }
    fn has(&self, entity: Entity) -> bool {
        self.values.contains_key(&entity)
    }
    fn name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
