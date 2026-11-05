// Map worker bootstrap: map loading and rasterising are Rust (`minimap_worker_handle`).
import init, { minimap_worker_handle } from './countersplat_web.js';
const ready = init();
self.onmessage = ({data}) => { ready.then(() => minimap_worker_handle(data)); };
