// Resolution selection is independent of the browser viewport and renderer.
export const MAX_RENDER_PIXELS = 16_777_216;
export const MAX_RENDER_DIMENSION = 8192;
export const DEFAULT_DISPLAY = Object.freeze({mode:'fixed', width:1280, height:720, scaling:'fill'});
export const RESOLUTION_PRESETS = Object.freeze([
  {id:'viewport', label:'Viewport (native)'},
  {id:'640x360', label:'640 × 360  ·  16:9', width:640, height:360},
  {id:'1280x720', label:'1280 × 720  ·  16:9', width:1280, height:720},
  {id:'1920x1080', label:'1920 × 1080  ·  16:9', width:1920, height:1080},
  {id:'2560x1440', label:'2560 × 1440  ·  16:9', width:2560, height:1440},
  {id:'3840x2160', label:'3840 × 2160  ·  16:9', width:3840, height:2160},
  {id:'1024x768', label:'1024 × 768  ·  4:3', width:1024, height:768},
  {id:'2560x1080', label:'2560 × 1080  ·  21:9', width:2560, height:1080},
  {id:'3440x1440', label:'3440 × 1440  ·  ultrawide', width:3440, height:1440},
  {id:'custom', label:'Custom…'},
]);

export function validateResolution(width, height, deviceLimit = MAX_RENDER_DIMENSION) {
  const max = Math.min(MAX_RENDER_DIMENSION, Number.isFinite(deviceLimit) ? Math.floor(deviceLimit) : MAX_RENDER_DIMENSION);
  if (!Number.isInteger(width) || !Number.isInteger(height) || width < 64 || height < 64)
    return 'Width and height must be whole numbers of at least 64 pixels.';
  if (width > max || height > max) return `The maximum dimension on this device is ${max} pixels.`;
  if (width * height > MAX_RENDER_PIXELS) return 'The resolution exceeds the 16.8 megapixel render limit.';
  return null;
}
export function normalizeDisplay(value) {
  if (!value || typeof value !== 'object' || !['fixed','viewport'].includes(value.mode) ||
      !['fill','fit'].includes(value.scaling) || validateResolution(value.width,value.height))
    return {...DEFAULT_DISPLAY};
  return {mode:value.mode, width:value.width, height:value.height, scaling:value.scaling};
}
export function aspectLabel(width, height) {
  let a=width, b=height;
  while (b) [a,b]=[b,a%b];
  return `${width/a}:${height/a}`;
}
// Bound the presentation surface proportionally, never by independently
// clamping axes; output pixels still map uniformly to CSS pointer coordinates.
export function viewportSize(cssWidth, cssHeight, devicePixelRatio, deviceLimit = MAX_RENDER_DIMENSION) {
  if (!(Number.isFinite(cssWidth) && cssWidth>0 && Number.isFinite(cssHeight) && cssHeight>0))
    return {width:1,height:1};
  const limit = Math.max(1, Math.min(MAX_RENDER_DIMENSION, Number.isFinite(deviceLimit) ? deviceLimit : MAX_RENDER_DIMENSION));
  const dpr = Number.isFinite(devicePixelRatio) && devicePixelRatio>0 ? devicePixelRatio : 1;
  const scale = Math.min(dpr, limit/cssWidth, limit/cssHeight, Math.sqrt(MAX_RENDER_PIXELS/(cssWidth*cssHeight)));
  return {width:Math.max(1,Math.floor(cssWidth*scale)),height:Math.max(1,Math.floor(cssHeight*scale))};
}
export function sceneResolution(preferences, viewport) {
  return preferences.mode === 'viewport' ? viewport : {width:preferences.width,height:preferences.height};
}

// The display panel is host UI because it also applies before a game scene is
// loaded. Only the selected settings cross into the renderer; game UI stays ECS.
export function attachDisplayPanel(document, storage, getDeviceLimit) {
  const key='countersplat.display.v1';
  const preset=document.getElementById('render-preset');
  const scaling=document.getElementById('viewport-scaling');
  const custom=document.getElementById('custom-resolution');
  const width=document.getElementById('render-width');
  const height=document.getElementById('render-height');
  const form=document.getElementById('resolution-form');
  const feedback=document.getElementById('display-feedback');
  const summary=document.getElementById('display-current');
  let preferences;
  try { preferences=normalizeDisplay(JSON.parse(storage.getItem(key))); }
  catch { preferences={...DEFAULT_DISPLAY}; }
  let viewport={width:1,height:1};
  for (const item of RESOLUTION_PRESETS) {
    const option=document.createElement('option'); option.value=item.id; option.textContent=item.label; preset.append(option);
  }
  const describe=() => {
    const size=sceneResolution(preferences,viewport);
    const label=preferences.mode==='viewport' ? 'Viewport' : `${size.width} × ${size.height}`;
    summary.textContent=label;
    if (size.width>1 && size.height>1)
      feedback.textContent=`${size.width} × ${size.height} · ${aspectLabel(size.width,size.height)} · ${preferences.scaling==='fill' ? 'Fill viewport' : 'Fit entire image'}`;
  };
  const sync=() => {
    const match=RESOLUTION_PRESETS.find(item=>item.width===preferences.width && item.height===preferences.height);
    preset.value=preferences.mode==='viewport' ? 'viewport' : match?.id || 'custom';
    scaling.value=preferences.scaling;
    custom.hidden=preset.value!=='custom';
    width.value=String(preferences.width); height.value=String(preferences.height);
    describe();
  };
  const save=next => {
    const limit=getDeviceLimit();
    const error=next.mode==='fixed' ? validateResolution(next.width,next.height,limit) : null;
    if (error) { feedback.textContent=error; return false; }
    preferences=next;
    try { storage.setItem(key,JSON.stringify(preferences)); } catch { /* The live setting still applies. */ }
    sync(); return true;
  };
  preset.addEventListener('change', () => {
    if (preset.value==='custom') { custom.hidden=false; width.focus(); return; }
    const chosen=RESOLUTION_PRESETS.find(item=>item.id===preset.value);
    if (!chosen) return;
    const next=chosen.id==='viewport' ? {...preferences,mode:'viewport'} :
      {...preferences,mode:'fixed',width:chosen.width,height:chosen.height};
    if (!save(next)) preset.value=preferences.mode==='viewport' ? 'viewport' :
      (RESOLUTION_PRESETS.find(item=>item.width===preferences.width && item.height===preferences.height)?.id || 'custom');
  });
  scaling.addEventListener('change', () => save({...preferences,scaling:scaling.value}));
  form.addEventListener('submit', event => {
    event.preventDefault();
    save({...preferences,mode:'fixed',width:Number(width.value),height:Number(height.value)});
  });
  sync();
  return {
    get preferences() { return preferences; },
    updateViewport(size) {
      if (size.width!==viewport.width || size.height!==viewport.height) { viewport=size; if (preferences.mode==='viewport') describe(); }
    },
  };
}
