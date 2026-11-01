import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const engineApi = await readFile(new URL('../crates/scripting-lua/src/engine_api.lua', import.meta.url), 'utf8');
const camera = {position:[1,2,3], yaw:-2, pitch:0, fov_degrees:71, view_offset:[0,0,0]};
const target = {ecs_operation(_layer, raw) {
  const op = JSON.parse(raw);
  const value = op.op === 'get' ? camera : op.op === 'has' ? true : null;
  return JSON.stringify({found:op.op !== 'missing', value});
}};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', engineApi, []);
// Reproduce the previous request and response wrappers, including the
// nested result encoding. Both paths use the same fake host.
function encodeResult(value) {
  if (Array.isArray(value)) return {kind:'array', entries:value.map(encodeResult)};
  if (value !== null && typeof value === 'object') {
    const entries = Object.create(null);
    for (const key of Object.keys(value)) entries[key] = encodeResult(value[key]);
    return {kind:'object', entries};
  }
  return value;
}
scene.vm.global.set('__benchmark_old_call', request => {
  const result = scene.call(request, true);
  return {found:result.found, value:encodeResult(result.value)};
});
scene.vm.doStringSync(`
  local old_call, kind, iterate, sequence = __benchmark_old_call, type, pairs, ipairs
  __benchmark_old_call = nil
  local arrays = setmetatable({}, {__mode='k'})
  local function decode(value)
    if kind(value) ~= 'table' then return value end
    local result = value.entries
    if value.kind == 'array' then
      arrays[result] = true
      for i, item in sequence(result) do result[i] = decode(item) end
    elseif value.kind == 'object' then
      for key, item in iterate(result) do result[key] = decode(item) end
    else error('invalid bridge value') end
    return result
  end
  local function old_flat(op, args)
    args.op = op
    local result = old_call(args)
    if result.found then return decode(result.value) end
  end
  local function old_two(op, key1, value1, key2, value2)
    local args = {[key1]=value1, [key2]=value2}
    if kind(value1) == 'string' and kind(value2) == 'string' then return old_flat(op, args) end
    error('invalid benchmark input')
  end
  function benchmark_old()
    for i=1,1000 do
      assert(old_two('get','id','entity','name','PlayerCamera').position[2] == 2)
      assert(old_two('has','id','entity','name','PlayerCamera'))
    end
  end
  function benchmark_new()
    for i=1,1000 do
      assert(ecs.get('entity','PlayerCamera').position[2] == 2)
      assert(ecs.has('entity','PlayerCamera'))
    end
  end
`);
const fns = {old:scene.vm.global.get('benchmark_old'), current:scene.vm.global.get('benchmark_new')};
for (let i=0; i<3; i++) {fns.old(); fns.current();}
const samples = {old:[], current:[]};
for (let i=0; i<11; i++) {
  for (const name of i % 2 ? ['current','old'] : ['old','current']) {
    const start = performance.now(); fns[name](); samples[name].push(performance.now()-start);
  }
}
for (const sample of Object.values(samples)) sample.sort((a,b)=>a-b);
console.log(JSON.stringify({groups:1000, wrapper_ms:samples.old[5], scalar_ms:samples.current[5]}));
scene.close();
