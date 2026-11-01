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
scene.vm.doStringSync(`
  local old_call, kind = __benchmark_old_call, type
  __benchmark_old_call = nil
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
  function benchmark_new()
    for i=1,2000 do assert(ecs.get_if_changed('entity', 'PlayerControls', 'revision') == nil) end
  end
`);
const functions = {old:scene.vm.global.get('benchmark_old'), current:scene.vm.global.get('benchmark_new')};
for (let i=0; i<3; i++) { functions.old(); functions.current(); }
const samples = {old:[], current:[]};
for (let i=0; i<11; i++) {
  for (const name of i % 2 ? ['current', 'old'] : ['old', 'current']) {
    const start = performance.now();
    functions[name]();
    samples[name].push(performance.now() - start);
  }
}
for (const sample of Object.values(samples)) sample.sort((a,b) => a-b);
console.log(JSON.stringify({iterations:2000, wrapper_ms:samples.old[5], sentinel_ms:samples.current[5]}));
scene.close();
