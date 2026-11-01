//! Minimal ECS owner + scripting worker integration.
use countersplat_ecs::World;
use countersplat_scripting_lua::{ChannelBridge, ScriptOptions, ScriptWorker, WorldRequest};
use std::{error::Error, sync::mpsc, thread};

fn main() -> Result<(), Box<dyn Error>> {
    let (request_sender, requests) = mpsc::sync_channel::<WorldRequest>(64);
    let ecs = thread::spawn(move || {
        let mut world = World::new();
        while let Ok(request) = requests.recv() {
            let response = world.apply_command(request.command);
            let _ = request.reply.send(response);
        }
        world
    });

    let scripts =
        ScriptWorker::spawn(ChannelBridge::new(request_sender), ScriptOptions::default())?;
    scripts
        .execute(
            r#"
        player = ecs.spawn()
        ecs.set(player, "Position", {x = 0, y = 0})
        ecs.system("move", function(dt)
            local position = ecs.get(player, "Position")
            position.x = position.x + dt
            ecs.set(player, "Position", position)
        end)
    "#,
        )?
        .recv()??;
    scripts.run_systems(0.5)?.recv()??;
    scripts.shutdown()?;

    let world = ecs.join().map_err(|_| "ECS worker panicked")?;
    println!("Final world snapshot: {:#?}", world.snapshot()?);
    Ok(())
}
