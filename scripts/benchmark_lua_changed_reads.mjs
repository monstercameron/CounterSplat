import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const engineApi = await readFile(new URL('../crates/scripting-lua/src/engine_api.lua', import.meta.url), 'utf8');
// This fake host always returns an unchanged component. It measures only the
// Wasmoon/Lua/JavaScript bridge, not Rust execution or frame rate.
const target = {ecs_operation(_layer, raw) {
  const op = JSON.parse(raw);
  if (op.op !== 'get_if_changed') throw new Error('Unexpected operation');
  return JSON.stringify({found:false, value:null});
}};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', engineApi, []);
// Reproduce the previous callback's wrapper-table response for the unchanged case.
scene.vm.global.set('__benchmark_old_call', request => {
  const result = scene.call(request, true);
  return {found:result.found, value:result.value};
});
// The intermediate implementation still built a Lua request table but used a
// boolean sentinel for the unchanged response.
scene.vm.global.set('__benchmark_table_call', request => {
  const result = scene.call(request, true);
  return result.found ? result.value : false;
});
scene.vm.doStringSync(`
  local old_call, table_call, kind = __benchmark_old_call, __benchmark_table_call, type
  __benchmark_old_call, __benchmark_table_call = nil, nil
  local function old_invoke_flat(op, args)
    args.op = op
    local result = old_call(args)
    if result.found then return result.value end
  end
  local function old_read(id, name, revision)
    local args = {id=id, name=name, revision=revision}
    if kind(id) == 'string' and kind(name) == 'string' and
        (revision == nil or kind(revision) == 'string') then
      return old_invoke_flat('get_if_changed', args)
    end
    error('invalid benchmark input')
  end
  function benchmark_old()
    for i=1,2000 do assert(old_read('entity', 'PlayerControls', 'revision') == nil) end
  end
  local function table_read(id, name, revision)
    local args = {id=id, name=name, revision=revision}
    if kind(id) == 'string' and kind(name) == 'string' and
        (revision == nil or kind(revision) == 'string') then
      args.op = 'get_if_changed'
      local result = table_call(args)
      if result ~= false then return result end
      return nil
    end
    error('invalid benchmark input')
  end
  function benchmark_table()
    for i=1,2000 do assert(table_read('entity', 'PlayerControls', 'revision') == nil) end
  end
  function benchmark_new()
    for i=1,2000 do assert(ecs.get_if_changed('entity', 'PlayerControls', 'revision') == nil) end
  end
`);
const functions = {old:scene.vm.global.get('benchmark_old'),
  table:scene.vm.global.get('benchmark_table'), current:scene.vm.global.get('benchmark_new')};
for (let i=0; i<3; i++) { functions.old(); functions.table(); functions.current(); }
const samples = {old:[], table:[], current:[]};
for (let i=0; i<11; i++) {
  for (const name of i % 2 ? ['current', 'table', 'old'] : ['old', 'table', 'current']) {
    const start = performance.now();
    functions[name]();
    samples[name].push(performance.now() - start);
  }
}
for (const sample of Object.values(samples)) sample.sort((a,b) => a-b);
console.log(JSON.stringify({iterations:2000, wrapper_ms:samples.old[5], table_sentinel_ms:samples.table[5], scalar_sentinel_ms:samples.current[5]}));
scene.close();
