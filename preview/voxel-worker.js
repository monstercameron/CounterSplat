// Voxel worker bootstrap: collision data and queries are Rust (`voxel_worker_handle`), separate from the WebGPU owner.
import init, { voxel_worker_handle } from './countersplat_web.js';
const ready = init();
self.onmessage = ({data}) => { ready.then(() => voxel_worker_handle(data)); };
