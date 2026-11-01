import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const api = await readFile(new URL('../crates/scripting-lua/src/engine_api.lua', import.meta.url), 'utf8');
const target = {ecs_operation(_layer, raw) {
  const op = JSON.parse(raw);
  if (op.op !== 'query' || !Array.isArray(op.components)) throw new Error('invalid query');
  return JSON.stringify({found:true, value:['entity']});
}};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', api, []);
scene.vm.global.set('__benchmark_old_call', request => {
  const result = scene.call(request);
  return {found:result.found, value:{kind:'array', entries:result.value}};
});
scene.vm.doStringSync(`
  local old_call, kind, iterate, sequence = __benchmark_old_call, type, pairs, ipairs
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
  local function decode(value)
    if kind(value) ~= 'table' then return value end
    local result = value.entries
    if value.kind == 'array' then
      arrays[result] = true
      for i,item in sequence(result) do result[i] = decode(item) end
    elseif value.kind == 'object' then
      for key,item in iterate(result) do result[key] = decode(item) end
    else error('invalid bridge value') end
    return result
  end
  local function old_query(components)
    local args = {op='query', components=components}
    local result = old_call(encode(args, {}, 0, {n=0}))
    if result.found then return decode(result.value) end
  end
  function benchmark_old()
    for i=1,2000 do assert(old_query({'PhysicsBody'})[1] == 'entity') end
  end
  function benchmark_new()
    for i=1,2000 do assert(ecs.query({'PhysicsBody'})[1] == 'entity') end
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
console.log(JSON.stringify({iterations:2000, general_ms:samples.old[5], single_ms:samples.current[5]}));
scene.close();
