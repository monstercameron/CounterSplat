// Run potentially non-terminating Lua in a worker so the parent test can
// terminate it if the instruction budget regresses.
import {parentPort} from 'node:worker_threads';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const target = {ecs_operation() { return '{"found":false,"value":null}'; }};
const cases = [
  {script:"ecs.system('bad', function() while true do end end)", dispatch:false},
  {script:"ecs.system('bad', function() while true do pcall(function() while true do end end) end end)", dispatch:false},
  {script:"ecs.on('bad', function() while true do xpcall(function() while true do end end, function() return 'caught' end) end end)", dispatch:true},
  {script:"ecs.system('bad', function() while true do pcall(__run_systems, 0) end end)", dispatch:false},
];
const results=[];
for (const item of cases) {
  const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', [item.script]);
  try {
    if (item.dispatch) scene.dispatch('bad', {});
    else scene.run(1/60);
    results.push('returned');
  } catch (error) {
    results.push(error.message);
  }
  // The limit must be cleared after a failure so later safe work can run.
  scene.vm.doStringSync("ecs.remove_system('bad'); ecs.off('bad'); ecs.system('safe', function() end)");
  scene.run(1/60);
  scene.close();
}
parentPort.postMessage(results);
