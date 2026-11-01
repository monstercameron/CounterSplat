//! A separately instantiated ECS menu scene, rendered over the paused game.
use countersplat_ecs::{Entity, World};
use countersplat_scene::SceneManifest;
use countersplat_ui::{InputEvent, Rect, UiElement, UiEventKind};
use countersplat_ui_ecs::UiLayout;
use countersplat_ui_wgpu::{UiRenderer, UiTarget};

pub struct MenuUi {
    world: World,
    renderer: UiRenderer,
    layout: UiLayout,
    backdrop: Entity,
}
impl MenuUi {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        scene: &SceneManifest,
    ) -> Result<Self, String> {
        let mut world = World::new();
        countersplat_ui_ecs::register(&mut world)?;
        let entities = countersplat_scene_ecs::instantiate_layer(&mut world, scene, "ui")?;
        let backdrop = *entities
            .get("backdrop")
            .ok_or("menu scene needs a backdrop entity")?;
        if !world.has::<UiElement>(backdrop) {
            return Err("menu backdrop needs a UiElement".into());
        }
        let layout = UiLayout::new(&world, entities);
        Ok(Self {
            world,
            renderer: UiRenderer::new(device, format),
            layout,
            backdrop,
        })
    }
    pub fn script_operation(&mut self, request: &str) -> Result<String, String> {
        self.world.apply_script_json(request)
    }
    pub fn input(&mut self, event: InputEvent) -> Result<(), &'static str> {
        self.renderer.push_input(event)
    }
    pub fn set_arrange_mode(&mut self, enabled: bool) {
        self.renderer.set_arrange_mode(enabled);
    }
    pub fn layout(&self) -> Result<String, String> {
        self.layout.save(&self.world)
    }
    pub fn restore_layout(&mut self, value: &str) -> Result<(), String> {
        self.layout.restore(&mut self.world, value)
    }
    pub fn reset_layout(&mut self) {
        self.layout.reset(&mut self.world);
    }
    pub fn take_layout_changed(&mut self) -> bool {
        self.layout.take_changed()
    }
    pub fn wants_keyboard(&self) -> bool {
        self.renderer.wants_keyboard()
    }
    pub fn wants_pointer(&self) -> bool {
        self.renderer.wants_pointer()
    }
    pub fn draw(&mut self, target: UiTarget<'_>) -> Result<Vec<wgpu::CommandBuffer>, String> {
        let viewport = Rect::new(
            0.0,
            0.0,
            target.size[0] as f32 / target.pixels_per_point,
            target.size[1] as f32 / target.pixels_per_point,
        );
        if let Some(backdrop) = self.world.get_mut::<UiElement>(self.backdrop) {
            backdrop.rect = viewport;
        }
        let frame = countersplat_ui_ecs::extract(&self.world, viewport).map_err(str::to_owned)?;
        let output = self.renderer.draw(target, &frame).map_err(str::to_owned)?;
        for event in countersplat_ui_ecs::apply_events(&mut self.world, output.events) {
            if matches!(event.kind, UiEventKind::MovedBy(_)) {
                self.layout.changed();
            }
        }
        Ok(output.command_buffers)
    }
    pub fn after_submit(&mut self) {
        self.renderer.after_submit();
    }
}
