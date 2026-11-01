// One short-lived worker prepares metadata for the selected level and transfers
// the original bytes back; terminating this worker cancels its pending fetches.
import { prepareMetadata } from './metadata_loader.js';

self.onmessage = async ({data}) => {
  let lastUpdate = -Infinity;
  try {
    const result = await prepareMetadata(data, {onProgress(completed, total) {
      const now = performance.now();
      if (completed === total || now - lastUpdate >= 50) {
        lastUpdate = now;
        self.postMessage({type:'progress', completed, total});
      }
    }});
    self.postMessage({type:'done', result}, result.metas.map(bytes => bytes.buffer));
  } catch (error) {
    self.postMessage({type:'error', error:String(error)});
  }
};
