//! ECS adapters for script-visible audio scene data. The host owns audio playback and asset I/O.
use countersplat_audio::{AudioEmitter, AudioLayer};
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};

pub const AUDIO_LAYER_COMPONENT: &str = "AudioLayer";
pub const AUDIO_EMITTER_COMPONENT: &str = "AudioEmitter";

struct LayerAdapter;
impl ComponentAdapter for LayerAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<AudioLayer>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let layer: AudioLayer = serde_json::from_value(value).map_err(|e| e.to_string())?;
        layer.validate().map_err(|e| e.to_string())?;
        world.insert(entity, layer)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<AudioLayer>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<AudioLayer>(entity)
    }
}
struct EmitterAdapter;
impl ComponentAdapter for EmitterAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<AudioEmitter>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let emitter: AudioEmitter = serde_json::from_value(value).map_err(|e| e.to_string())?;
        emitter.validate().map_err(|e| e.to_string())?;
        world.insert(entity, emitter)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<AudioEmitter>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<AudioEmitter>(entity)
    }
}

/// Make audio components accessible through generic ECS commands and the Lua bridge.
pub fn register(world: &mut World) -> Result<(), String> {
    if world
        .registered_component_names()
        .iter()
        .any(|name| *name == AUDIO_LAYER_COMPONENT || *name == AUDIO_EMITTER_COMPONENT)
    {
        return Err("audio component adapter is already registered".into());
    }
    world.register_component_adapter(AUDIO_LAYER_COMPONENT, LayerAdapter)?;
    world.register_component_adapter(AUDIO_EMITTER_COMPONENT, EmitterAdapter)
}

/// Owned data sent from the ECS owner to a host that manages the audio worker and assets.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSnapshot {
    pub layers: Vec<(Entity, AudioLayer)>,
    pub emitters: Vec<(Entity, AudioEmitter)>,
}
pub fn extract(world: &World) -> AudioSnapshot {
    let layers = world
        .component_view::<AudioLayer>()
        .map(|view| {
            view.iter()
                .filter(|(_, value)| value.validate().is_ok())
                .map(|(entity, value)| (entity, value.clone()))
                .collect()
        })
        .unwrap_or_default();
    let emitters = world
        .component_view::<AudioEmitter>()
        .map(|view| {
            view.iter()
                .filter(|(_, value)| value.validate().is_ok())
                .map(|(entity, value)| (entity, value.clone()))
                .collect()
        })
        .unwrap_or_default();
    AudioSnapshot { layers, emitters }
}
