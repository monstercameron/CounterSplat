// CPU-only SOG fetch and decode worker. Raw decoded streams are transferred to the GPU owner.
import init, { decode_texture } from '/pkg/countersplat_web.js';

const names = ['means_l.webp', 'means_u.webp', 'quats.webp', 'scales.webp', 'sh0.webp', 'shN_labels.webp', 'shN_centroids.webp'];
const ready = init();
let queue = Promise.resolve();

function levelPath(base, folder, name, single) {
  const relative = (single ? name : `${folder}/${name}`).split('/').map(encodeURIComponent).join('/');
  return new URL(relative, base).href;
}
async function bytes(path) {
  const response = await fetch(path, {cache: 'no-store'});
  if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}
async function decode({index, folder, metadata, base, single}) {
  await ready;
  const parsed = JSON.parse(new TextDecoder().decode(metadata));
  const hasSh = Boolean(parsed.shN && parsed.shN.count);
  const encoded = await Promise.all(names.map((name, part) =>
    part >= 5 && !hasSh ? new Uint8Array() : bytes(levelPath(base, folder, name, single))));
  const dimensions = new Uint32Array(names.length * 2);
  const pixels = encoded.map((stream, part) => {
    const texture = decode_texture(part, stream);
    dimensions[part * 2] = texture.width();
    dimensions[part * 2 + 1] = texture.height();
    // wasm-bindgen already returns an owned copy; transfer it without copying again.
    const pixels = texture.take_pixels();
    texture.free();
    return pixels;
  });
  self.postMessage({ok: true, index, metadata, dimensions, pixels},
    [metadata.buffer, dimensions.buffer, ...pixels.map(pixel => pixel.buffer)]);
}
self.onmessage = ({data}) => {
  queue = queue.then(() => decode(data)).catch(error => {
    self.postMessage({ok: false, index: data.index, error: String(error)});
  });
};
