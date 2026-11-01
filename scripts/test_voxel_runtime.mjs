import assert from 'node:assert/strict';
import { VoxelRuntime } from '../preview/voxel_runtime.js';

class FakeWorker {
  messages = [];
  postMessage(value) { this.messages.push(value); }
  terminate() { this.terminated = true; }
  send(value) { this.onmessage({data:value}); }
}
const body = (radius=0.2) => ({enabled:true,collision_enabled:true,gravity_enabled:false,radius,collision_offset_y:0,gravity:18,velocity:[0,0,0],grounded:false});
class FakeGame {
  values = {
    player:{PlayerCamera:{position:[0,0,0],yaw:0,pitch:0,fov_degrees:60},PhysicsBody:{...body(),collision_offset_y:-1.4}},
    prop:{Transform:{position:[10,0,0]},PhysicsBody:body(0.4)},
    level:{VoxelCollider:{enabled:true,visible:false,physics:true,player_radius:0.2}, VoxelColliderState:{}}
  };
  resources = {'scene.entities':{active_level:'level',player:'player'}};
  events = [];
  ecs_operation(_, encoded) {
    const op=JSON.parse(encoded); let value;
    if (op.op === 'get_resource') value=this.resources[op.name];
    else if (op.op === 'set_resource') this.resources[op.name]=op.value;
    else if (op.op === 'is_alive') value=!!this.values[op.id];
    else if (op.op === 'get') value=structuredClone(this.values[op.id]?.[op.name]);
    else if (op.op === 'set') this.values[op.id][op.name]=structuredClone(op.value);
    else if (op.op === 'emit') this.events.push(op);
    return JSON.stringify({value});
  }
  set_voxel_debug(data,size) { this.debug=[...data]; this.size=size; }
  clear_voxel_debug() { this.debug=[]; }
}
const game=new FakeGame(), worker=new FakeWorker();
game.values.player.PhysicsBody.capsule_half_height=0.6;
const runtime=new VoxelRuntime(game,new URL('http://localhost/assets/levels/1234abcd/'),()=>true,worker);
const events=[], probes=[]; const lua={drain(name) { return name === 'physics.move' ? events.splice(0) : name === 'physics.ground_request' ? probes.splice(0) : []; }};
worker.send({type:'ready',resolution:0.05}); runtime.tick(lua,0);
assert.equal(game.resources['voxel.capture_movement'],true);
assert.equal(game.events.at(-1).name,'physics.ready');
// Ground queries are routed as data; Lua, not the browser host, decides how to use them.
probes.push({entity:'player',token:'spawn',position:[0,0,0],radius:0.1,max_drop:4});
runtime.tick(lua,1); const ground=worker.messages.findLast(value=>value.type==='ground');
worker.send({type:'ground',id:ground.id,height:-1.5});
assert.equal(game.events.at(-1).name,'physics.ground_result');
assert.equal(game.events.at(-1).value.height,-1.5);
assert.deepEqual(game.values.player.PlayerCamera.position,[0,0,0]);
probes.push({entity:'player',token:'failed-spawn',position:[0,0,0],radius:0.1,max_drop:4});
runtime.tick(lua,2); const failedGround=worker.messages.findLast(value=>value.type==='ground');
worker.send({type:'error',operation:'ground',id:failedGround.id,message:'bad ground'});
assert.equal(game.events.at(-2).name,'physics.ground_result');
assert.equal(game.events.at(-2).value.height,null);
assert.equal(game.events.at(-1).name,'physics.error');
// Two entities have independent collision radii, requests, results and contact state.
game.values.player.PlayerCamera.position=[1,0,0];
game.values.player.PhysicsBody.velocity=[1,0,0];
game.values.prop.Transform.position=[10,-1,0];
game.values.prop.PhysicsBody.velocity=[0,-2,0];
events.push({entity:'player',component:'PlayerCamera',delta:[1,0,0]}, {entity:'prop',component:'Transform',delta:[0,-1,0]});
runtime.tick(lua,16);
assert.deepEqual(game.values.player.PlayerCamera.position,[0,0,0]);
assert.deepEqual(game.values.prop.Transform.position,[10,0,0]);
const moves=worker.messages.filter(value => value.type === 'move');
assert.equal(moves.length,2); assert.equal(moves[0].radius,0.2); assert.equal(moves[1].radius,0.4);
assert.equal(moves[0].halfHeight,0.6); assert.equal(moves[1].halfHeight,0);
assert.deepEqual(moves[0].position,[0,-1.4,0]);
worker.send({type:'move',id:moves[1].id,position:[10,-0.25,0]});
assert.equal(game.values.prop.PhysicsBody.grounded,false);
assert.equal(game.values.prop.PhysicsBody.velocity[1],-2);
assert.equal(game.events.at(-1).name,'physics.motion_resolved');
assert.deepEqual(game.events.at(-1).value.moved,[0,-0.25,0]);
worker.send({type:'move',id:moves[0].id,position:[0.5,-1.4,0]});
assert.deepEqual(game.values.player.PlayerCamera.position,[0.5,0,0]);
assert.equal(game.values.player.PhysicsBody.velocity[0],1);
assert.equal(game.events.at(-1).name,'physics.motion_resolved');
// A radius edit cancels the stale response and retries the movement with the new radius.
game.values.player.PlayerCamera.position=[1.5,0,0]; events.push({entity:'player',component:'PlayerCamera',delta:[1,0,0]});
runtime.tick(lua,32);
const stale=worker.messages.findLast(value=>value.type==='move');
game.values.player.PhysicsBody.radius=0.3; runtime.tick(lua,48);
const retry=worker.messages.findLast(value=>value.type==='move');
assert.notEqual(stale.id,retry.id); assert.equal(retry.radius,0.3);
game.values.player.PhysicsBody.capsule_half_height=0.75; runtime.tick(lua,49);
const retall=worker.messages.findLast(value=>value.type==='move');
assert.notEqual(retall.id,retry.id); assert.equal(retall.halfHeight,0.75);
worker.send({type:'move',id:stale.id,position:[1.5,-1.4,0]});
worker.send({type:'move',id:retry.id,position:[1.5,-1.4,0]});
assert.deepEqual(game.values.player.PlayerCamera.position,[0.5,0,0]);
worker.send({type:'move',id:retall.id,position:[0.9,-1.4,0]});
assert.deepEqual(game.values.player.PlayerCamera.position,[0.9,0,0]);
// Disabling collision on one entity releases its pending movement without affecting the other.
game.values.prop.Transform.position=[11,-0.25,0]; events.push({entity:'prop',component:'Transform',delta:[1,0,0]});
runtime.tick(lua,64); const old=worker.messages.findLast(value=>value.type==='move');
game.values.prop.PhysicsBody.collision_enabled=false; runtime.tick(lua,80);
assert.deepEqual(game.values.prop.Transform.position,[11,-0.25,0]);
worker.send({type:'move',id:old.id,position:[10.2,-0.25,0]});
assert.deepEqual(game.values.prop.Transform.position,[11,-0.25,0]);
// A fast movement is split into safe subqueries, and a failure stays local to that entity.
game.values.player.PlayerCamera.position=[20.9,0,0]; events.push({entity:'player',component:'PlayerCamera',delta:[20,0,0]});
runtime.tick(lua,96); const fast=worker.messages.findLast(value=>value.type==='move');
assert(fast.deltas.length > 1 && fast.deltas.every(delta=>Math.hypot(...delta) <= 5.76 + 1e-9));
worker.send({type:'error',operation:'move',id:fast.id,message:'movement failed'});
assert.equal(game.events.at(-1).name,'physics.motion_error');
assert.equal(game.values.level.VoxelColliderState.status,'ready');
game.values.level.VoxelCollider.visible=true; runtime.tick(lua,500);
const debug=worker.messages.findLast(value => value.type === 'debug');
worker.send({type:'debug',id:debug.id,points:new Float32Array([0,0,0,1,1,1])});
assert.equal(game.values.level.VoxelColliderState.visible_voxels,2);
game.values.level.VoxelCollider.enabled=false; runtime.tick(lua,1000);
assert.equal(game.resources['voxel.capture_movement'],false); assert.deepEqual(game.debug,[]);
runtime.stop(); assert.equal(worker.terminated,true);
console.log('Per-entity voxel collision coordination tests passed.');
