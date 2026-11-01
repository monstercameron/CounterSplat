import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const api = await readFile(new URL('../crates/scripting-lua/src/engine_api.lua', import.meta.url), 'utf8');
const target = {ecs_operation(_layer, raw) {
  const op = JSON.parse(raw);
  if (op.op !== 'set_field' && op.op !== 'set_fields') throw new Error('invalid operation');
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
  local old_vector = {1,2,3}; arrays[old_vector] = true
  local new_vector = ecs.array(); new_vector[1],new_vector[2],new_vector[3] = 1,2,3
  local old_fields = {velocity=old_vector, grounded=true}
  local new_fields = {velocity=new_vector, grounded=true}
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
  function benchmark_old_pair()
    for i=1,2000 do old_set_pair('entity','PhysicsBody',old_fields) end
  end
  function benchmark_new_pair()
    for i=1,2000 do ecs.set_fields('entity','PhysicsBody',new_fields) end
  end
`);
for (const kind of ['vector','scalar','pair']) {
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
