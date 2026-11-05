// Chunk worker bootstrap: decoding is Rust (`chunk_worker_handle`); this only loads the module and forwards messages.
import init, { chunk_worker_handle } from './pkg/countersplat_web.js';
const ready = init();
self.onmessage = ({data}) => { ready.then(() => chunk_worker_handle(data)); };
