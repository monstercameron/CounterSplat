// Metadata worker bootstrap: loading and describing level metadata is Rust (`metadata_worker_handle`).
import init, { metadata_worker_handle } from './countersplat_web.js';
const ready = init();
self.onmessage = ({data}) => { ready.then(() => metadata_worker_handle(data)); };
