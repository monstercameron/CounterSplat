import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const api = await readFile(new URL('../crates/scripting-lua/src/engine_api.lua', import.meta.url), 'utf8');
const target = {ecs_operation(_layer, raw) {
  const op = JSON.parse(raw);
  if (op.op !== 'set_field' && op.op !== 'set_fields' && op.op !== 'emit') throw new Error('invalid operation');
  return JSON.stringify({found:false, value:null});
}};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', api, []);
scene.vm.global.set('__benchmark_old_call', request => {
  const result = scene.call(request);
  return {found:result.found, value:result.value};
});
scene.vm.doStringSync(`
  local old_call, kind, iterate = __benchmark_old_call, type, pairs
  __benchmark_old_call = nil
  local arrays = setmetatable({}, {__mode='k'})
  local function encode(value, seen, depth, count)
    count.n = count.n + 1
    assert(depth <= 128 and count.n <= 100000)
    if kind(value) ~= 'table' then return value end
    assert(not seen[value]); seen[value] = true
    local result = {}
    if arrays[value] then result.__countersplat_array = true end
    for key, item in iterate(value) do result[key] = encode(item, seen, depth + 1, count) end
    seen[value] = nil
    return result
  end
  local function old_set(id, name, field, value)
    local args = {op='set_field', id=id, name=name, field=field, value=value}
    old_call(encode(args, {}, 0, {n=0}))
  end
  local function old_set_pair(id, name, fields)
    local args = {op='set_fields', id=id, name=name, fields=fields}
    old_call(encode(args, {}, 0, {n=0}))
  end
  local function old_emit(name, value)
    local args = {op='emit', name=name, value=value}
    old_call(encode(args, {}, 0, {n=0}))
  end
  local old_vector = {1,2,3}; arrays[old_vector] = true
  local new_vector = ecs.array(); new_vector[1],new_vector[2],new_vector[3] = 1,2,3
  local old_fields = {velocity=old_vector, grounded=true}
  local new_fields = {velocity=new_vector, grounded=true}
  local old_three = {view_offset=old_vector, view_bob_pitch=0.2, view_bob_roll=-0.1}
  local new_three = {view_offset=new_vector, view_bob_pitch=0.2, view_bob_roll=-0.1}
  local old_event = {entity='entity', component='PlayerCamera', delta=old_vector}
  local new_event = {entity='entity', component='PlayerCamera', delta=new_vector}
  local large_event = {clip='idle', index=12, visible=true, opacity=1, key_threshold=.5,
    key_softness=.1, despill=.2, vertical_offset=0, motion_x=0, motion_y=0,
    motion_roll=0, direction=1, loop=true, transition_seconds=.1, loop_blend_seconds=.1}
  function benchmark_old_emit()
    for i=1,2000 do old_emit('physics.move',old_event) end
  end
  function benchmark_new_emit()
    for i=1,2000 do ecs.emit('physics.move',new_event) end
  end
  function benchmark_old_large_emit()
    for i=1,2000 do old_emit('first_person.frame',large_event) end
  end
  function benchmark_new_large_emit()
    for i=1,2000 do ecs.emit('first_person.frame',large_event) end
  end
  function benchmark_old_vector()
    for i=1,2000 do old_set('entity','PhysicsBody','velocity',old_vector) end
  end
  function benchmark_new_vector()
    for i=1,2000 do ecs.set_field('entity','PhysicsBody','velocity',new_vector) end
  end
  function benchmark_old_scalar()
    for i=1,2000 do old_set('entity','PlayerControls','enabled',false) end
  end
  function benchmark_new_scalar()
    for i=1,2000 do ecs.set_field('entity','PlayerControls','enabled',false) end
  end
  function benchmark_old_three()
    for i=1,2000 do old_set_pair('entity','PlayerCamera',old_three) end
  end
  function benchmark_new_three()
    for i=1,2000 do ecs.set_fields('entity','PlayerCamera',new_three) end
  end
  function benchmark_old_pair()
    for i=1,2000 do old_set_pair('entity','PhysicsBody',old_fields) end
  end
  function benchmark_new_pair()
    for i=1,2000 do ecs.set_fields('entity','PhysicsBody',new_fields) end
  end
`);
for (const kind of ['vector','scalar','pair','three','emit','large_emit']) {
  const fns = {old:scene.vm.global.get('benchmark_old_'+kind), current:scene.vm.global.get('benchmark_new_'+kind)};
  for (let i=0; i<3; i++) {fns.old(); fns.current();}
  const samples = {old:[], current:[]};
  for (let i=0; i<11; i++) {
    for (const name of i % 2 ? ['current','old'] : ['old','current']) {
      const start = performance.now(); fns[name](); samples[name].push(performance.now()-start);
    }
  }
  for (const sample of Object.values(samples)) sample.sort((a,b)=>a-b);
  console.log(JSON.stringify({kind, iterations:2000, general_ms:samples.old[5], scalar_ms:samples.current[5]}));
}
scene.close();
