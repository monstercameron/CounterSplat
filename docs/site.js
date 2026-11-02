// An original, procedural point-cloud study. No game media is needed by the site.
const canvas = document.getElementById('scan-study');
if (canvas) {
  const context = canvas.getContext('2d');
  const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');
  let seed = 18763;
  const random = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 4294967296; };
  const points = [];
  function point(x, y, z, kind) {
    points.push({x, y, z, kind, light: .55 + random() * .45, size: .45 + random() * 1.15});
  }
  // Floor, curved shell, and repeated structural ribs suggest a captured hall.
  for (let i = 0; i < 5100; i++) point((random() - .5) * 13, random() * .04, random() * 22, 'floor');
  for (let i = 0; i < 7800; i++) {
    const a = random() * Math.PI, z = random() * 22;
    point(Math.cos(a) * 6.1, Math.sin(a) * 5.4 + .1, z, 'shell');
  }
  for (let rib = 0; rib < 6; rib++) {
    for (let i = 0; i < 950; i++) {
      const a = random() * Math.PI, z = 1 + rib * 3.9 + (random() - .5) * .16;
      point(Math.cos(a) * 6.03, Math.sin(a) * 5.33 + .12, z, 'rib');
    }
  }
  for (let i = 0; i < 2300; i++) {
    const side = random() < .5 ? -1 : 1;
    const z = 1 + Math.floor(random() * 6) * 3.9 + (random() - .5) * .55;
    point(side * (5.65 + random() * .6), random() * 5, z, 'rib');
  }
  points.sort((a, b) => b.z - a.z);
  let pointer = 0, scheduled = false;
  function draw() {
    scheduled = false;
    const box = canvas.getBoundingClientRect();
    if (!box.width || !box.height) return;
    const scale = Math.min(devicePixelRatio || 1, 1.5);
    canvas.width = Math.round(box.width * scale); canvas.height = Math.round(box.height * scale);
    context.setTransform(scale, 0, 0, scale, 0, 0);
    const w = box.width, h = box.height;
    const background = context.createLinearGradient(0, 0, 0, h);
    background.addColorStop(0, '#293f37'); background.addColorStop(.58, '#182f2b'); background.addColorStop(1, '#19221e');
    context.fillStyle = background; context.fillRect(0, 0, w, h);
    const glow = context.createRadialGradient(w * .52, h * .45, 0, w * .52, h * .45, w * .55);
    glow.addColorStop(0, 'rgba(198,185,126,.24)'); glow.addColorStop(1, 'rgba(15,32,26,0)');
    context.fillStyle = glow; context.fillRect(0, 0, w, h);
    const focal = Math.min(w * .89, h * .84), horizon = h * .54, yaw = pointer * .025;
    const c = Math.cos(yaw), s = Math.sin(yaw);
    for (const p of points) {
      const z = p.z + 5.8, x = p.x * c - z * s, depth = p.x * s + z * c;
      if (depth < 1) continue;
      const sx = w * .5 + x * focal / depth, sy = horizon - (p.y - 1.7) * focal / depth;
      if (sx < -4 || sx > w + 4 || sy < -4 || sy > h + 4) continue;
      const fog = Math.max(.17, 1 - depth / 34), size = Math.min(3, p.size * (1 + 9 / depth));
      const tint = p.kind === 'rib' ? [220,195,142] : p.kind === 'floor' ? [172,176,143] : [137,177,155];
      context.fillStyle = 'rgba(' + tint[0] + ',' + tint[1] + ',' + tint[2] + ',' + (fog * p.light * .68).toFixed(2) + ')';
      context.fillRect(sx, sy, size, size * .8);
    }
  }
  function requestDraw() {
    if (!scheduled) { scheduled = true; requestAnimationFrame(draw); }
  }
  canvas.addEventListener('pointermove', event => {
    if (reduceMotion.matches) return;
    const box = canvas.getBoundingClientRect();
    pointer = (event.clientX - box.left) / box.width - .5;
    requestDraw();
  }, {passive:true});
  canvas.addEventListener('pointerleave', () => { pointer = 0; requestDraw(); });
  window.addEventListener('resize', requestDraw, {passive:true});
  requestDraw();
}
