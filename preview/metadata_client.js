// Owns the metadata worker for exactly one load, including cancellation and errors.
export function loadMetadataInWorker(worker, request, signal, onProgress = () => {}) {
  return new Promise((resolve, reject) => {
    let settled = false, completed = 0;
    const cleanup = () => {
      worker.removeEventListener('message', onMessage);
      worker.removeEventListener('error', onError);
      worker.removeEventListener('messageerror', onError);
      signal.removeEventListener('abort', onAbort);
      worker.terminate();
    };
    const finish = (error, result) => {
      if (settled) return;
      settled = true; cleanup();
      if (error) reject(error); else resolve(result);
    };
    const onMessage = ({data}) => {
      try {
        if (data?.type === 'progress') {
          if (!Number.isInteger(data.completed) || data.completed < completed ||
              data.total !== request.filenames.length || data.completed > data.total) {
            throw new Error('Metadata worker returned invalid progress.');
          }
          completed = data.completed;
          onProgress(data.completed, data.total);
        } else if (data?.type === 'done') {
          if (!Array.isArray(data.result?.metas) || data.result.metas.length !== request.filenames.length ||
              !data.result.metas.every(bytes => bytes instanceof Uint8Array) || !data.result.levelAssets) {
            throw new Error('Metadata worker returned invalid results.');
          }
          finish(null, data.result);
        } else if (data?.type === 'error') finish(new Error(data.error || 'Metadata worker failed.'));
        else throw new Error('Metadata worker returned an unexpected message.');
      } catch (error) { finish(error); }
    };
    const onError = event => finish(new Error(event.message || 'Metadata worker failed.'));
    const onAbort = () => finish(new DOMException('Loading cancelled', 'AbortError'));
    if (signal.aborted) { onAbort(); return; }
    worker.addEventListener('message', onMessage);
    worker.addEventListener('error', onError);
    worker.addEventListener('messageerror', onError);
    signal.addEventListener('abort', onAbort, {once:true});
    const transfer = [request.singleMetadata, request.inventoryBytes]
      .filter(bytes => bytes instanceof Uint8Array).map(bytes => bytes.buffer);
    try { worker.postMessage(request, transfer); } catch (error) { finish(error); }
  });
}
