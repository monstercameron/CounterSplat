//! Hosts the rigid-body props: the gameplay scene's scripts describe and push them through the ECS
//! (`countersplat_physics::prop_runtime`), and the Viewer simulates and draws them.
use crate::Viewer;
use countersplat_physics::prop_runtime::Ecs;
use countersplat_render::props::PropInstance;

struct SceneEcs<'a>(&'a mut crate::ui_layer::SceneUi);

impl Ecs for SceneEcs<'_> {
    fn ecs(&mut self, operation: serde_json::Value) -> Result<serde_json::Value, String> {
        countersplat_scene::voxel_host::reply_value(&self.0.script_operation(&operation.to_string())?)
    }
}

impl Viewer {
    /// Advances the props by `seconds` of script time and hands their poses to the renderer. A layout error is
    /// reported once (until it changes or clears) instead of every frame.
    pub(crate) fn step_props(&mut self, seconds: f32) {
        let result = self.props.tick(&mut SceneEcs(&mut self.ui), seconds);
        let instances: Vec<PropInstance> = self
            .props
            .instances()
            .into_iter()
            .map(|p| PropInstance {
                cylinder: p.cylinder,
                position: p.position,
                rotation: p.rotation,
                scale: p.scale,
                color: p.color,
                cap_color: p.cap_color,
            })
            .collect();
        let result = result.and_then(|()| self.prop_renderer.set(&self.queue, &instances).map_err(str::to_owned));
        match result {
            Ok(()) => self.prop_error = None,
            Err(error) => {
                if self.prop_error.as_deref() != Some(error.as_str()) {
                    web_sys::console::warn_1(&format!("Props: {error}").into());
                    self.prop_error = Some(error);
                }
            }
        }
    }
}
