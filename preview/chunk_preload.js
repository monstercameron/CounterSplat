// Preload work is parallel but bounded by the same worker count used for streaming.
// Results remain in file order even when workers complete out of order.
import { ChunkDecoderPool } from './chunk_decoder_pool.js';

export function preloadChunks({metas, filenames, single, base, signal, size, createWorker, maxBytes, onProgress = () => {}}) {
  return new Promise((resolve, reject) => {
    if (!Array.isArray(metas) || !metas.length || metas.length !== filenames.length ||
        !Number.isSafeInteger(maxBytes) || maxBytes < 1) {
      reject(new Error('Invalid splat preload request.')); return;
    }
    let pool, next = 0, done = 0, retainedBytes = 0, settled = false;
    const chunks = new Array(metas.length);
    const finish = (error) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', onAbort);
      pool?.stop();
      if (error) reject(error); else resolve(chunks);
    };
    const onAbort = () => finish(new DOMException('Loading cancelled', 'AbortError'));
    const submitAvailable = () => {
      while (!settled && pool.available && next < metas.length) {
        const index = next++;
        const folder = single ? 'single' : filenames[index].split('/')[0];
        pool.submit({index, folder, metadata:metas[index], base, single});
      }
    };
    if (signal.aborted) { onAbort(); return; }
    signal.addEventListener('abort', onAbort, {once:true});
    try {
      pool = new ChunkDecoderPool({size, createWorker,
        onResult(data) {
          try {
            if (!(data.metadata instanceof Uint8Array) || !(data.dimensions instanceof Uint32Array) ||
                !Array.isArray(data.pixels) || !data.pixels.every(bytes => bytes instanceof Uint8Array)) {
              throw new Error('Decoder returned invalid chunk data.');
            }
            retainedBytes += data.metadata.byteLength + data.dimensions.byteLength +
              data.pixels.reduce((sum, pixels) => sum + pixels.byteLength, 0);
            if (retainedBytes > maxBytes) throw new Error('Decoded splat preload exceeds 512 MiB. Mark splat chunks streamable.');
            chunks[data.index] = data;
            onProgress(++done, metas.length);
            if (done === metas.length) finish(); else submitAvailable();
          } catch (error) { finish(error); }
        },
        onFailure(index, error) { finish(new Error(`Chunk ${index}: ${error}`)); },
        onFatal(error) { finish(new Error(`Chunk workers failed: ${error}`)); },
      });
      submitAvailable();
    } catch (error) { finish(error); }
  });
}
