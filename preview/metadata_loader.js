// Worker-side loading and description of level metadata. The renderer receives the
// original bytes; parsing the inventory and chunk descriptors stays off the page thread.
import { describeLevelAssets } from './asset_loader.js';

const MAX_METADATA_BYTES = 512 * 1024 * 1024;
const MAX_FILE_BYTES = 16 * 1024 * 1024;

export function metadataUrl(relative, base) {
  if (typeof relative !== 'string' || !relative || relative.startsWith('/') || relative.includes('\\') ||
      relative.split('/').some(part => !part || part === '.' || part === '..')) {
    throw new Error('The level contains an invalid asset path.');
  }
  return new URL(relative.split('/').map(encodeURIComponent).join('/'), base).href;
}

export async function prepareMetadata(request, {fetcher = fetch, onProgress = () => {}} = {}) {
  const {level, lodCount, filenames, base, singleMetadata, inventoryBytes,
    viewerSettings, hasSource, source = 'level', concurrency = 12} = request;
  if (!level || !Array.isArray(filenames) || filenames.length < 1 || filenames.length > 10_000 ||
      !Number.isInteger(concurrency) || concurrency < 1 || concurrency > 32) {
    throw new Error('Invalid level metadata request.');
  }
  const isSingle = level.format === 'single_sog';
  if (!isSingle && level.format !== 'streamed_sog') throw new Error('This level format is not supported.');
  if (isSingle && (filenames.length !== 1 || !(singleMetadata instanceof Uint8Array))) {
    throw new Error('Invalid single scan metadata.');
  }
  const metas = new Array(filenames.length), parsed = new Array(filenames.length);
  let next = 0, completed = 0, retainedBytes = 0;
  const decoder = new TextDecoder();
  async function readNext() {
    while (next < filenames.length) {
      const index = next++;
      let bytes;
      if (isSingle) bytes = singleMetadata;
      else {
        const url = metadataUrl(filenames[index], base);
        const response = await fetcher(url, {cache:'no-store'});
        if (!response.ok) throw new Error(`Could not load ${url} (HTTP ${response.status}).`);
        const declared = Number(response.headers?.get('content-length'));
        if (declared > MAX_FILE_BYTES) throw new Error('Chunk metadata exceeds the 16 MiB limit.');
        bytes = new Uint8Array(await response.arrayBuffer());
      }
      if (bytes.byteLength > MAX_FILE_BYTES) throw new Error('Chunk metadata exceeds the 16 MiB limit.');
      retainedBytes += bytes.byteLength;
      if (retainedBytes > MAX_METADATA_BYTES) throw new Error('Level metadata exceeds the 512 MiB limit.');
      metas[index] = bytes;
      parsed[index] = JSON.parse(decoder.decode(bytes));
      onProgress(++completed, filenames.length);
    }
  }
  await Promise.all(Array.from({length:Math.min(concurrency, filenames.length)}, readNext));
  const inventory = inventoryBytes?.length ? JSON.parse(decoder.decode(inventoryBytes)) : null;
  const levelAssets = describeLevelAssets(level, lodCount, parsed, filenames,
    {viewerSettings, inventory, source:hasSource}, source);
  return {metas, levelAssets};
}
