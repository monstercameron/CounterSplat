// Decode and project HDR panoramas away from the GPU/canvas owner.
import init, { decode_skybox_hdr } from '/pkg/countersplat_web.js';
const ready = init();
self.onmessage = async ({data}) => {
  try {
    await ready;
    const cube = decode_skybox_hdr(data.bytes, 256);
    const size = cube.size();
    const pixels = new Uint8Array(cube.take_pixels());
    cube.free();
    self.postMessage({ok: true, request: data.request, asset: data.asset, select: data.select, size, pixels}, [pixels.buffer]);
  } catch (error) {
    self.postMessage({ok: false, request: data.request, select: data.select, error: String(error)});
  }
};
