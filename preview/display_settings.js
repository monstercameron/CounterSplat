// Resolution selection is independent of the browser viewport and renderer.
export const MAX_RENDER_PIXELS = 16_777_216;
export const MAX_RENDER_DIMENSION = 8192;
export const DEFAULT_DISPLAY = Object.freeze({mode:'fixed', width:1280, height:720, scaling:'fill', aspect:'16:9'});
// The 21:9 family uses common marketed ultrawide sizes (which are not all
// mathematically identical ratios, e.g. 3440×1440 and 2560×1080).
export const ASPECT_RATIOS = Object.freeze([
  {id:'16:9', label:'16:9'}, {id:'16:10', label:'16:10'},
  {id:'21:9', label:'21:9'}, {id:'custom', label:'Custom'},
]);
const COMMON_RESOLUTIONS = Object.freeze({
  '16:9': [[640,360],[960,540],[1280,720],[1600,900],[1920,1080],[2560,1440],[3200,1800],[3840,2160],[5120,2880]],
  '16:10': [[640,400],[960,600],[1280,800],[1440,900],[1680,1050],[1920,1200],[2560,1600],[2880,1800],[3840,2400]],
  '21:9': [[1280,540],[1920,810],[2560,1080],[3440,1440],[3840,1600],[5120,2160]],
});
const CUSTOM_HEIGHTS = [180,270,360,432,480,540,600,720,800,900,1080,1200,1440,1600,1800,2160,2400,2880,3200];
const validAspect = id => ASPECT_RATIOS.some(item=>item.id===id);
// Store the user's reference ratio separately, so repeated rounded resolution
// steps cannot progressively change it. This is not a render allocation.
const validRatio = (width,height) => Number.isInteger(width) && Number.isInteger(height) &&
  width > 0 && height > 0 && width <= MAX_RENDER_DIMENSION && height <= MAX_RENDER_DIMENSION;
export function aspectMatches(id, width, height) {
  if (!(Number.isFinite(width) && Number.isFinite(height) && width>0 && height>0)) return false;
  if (id==='custom') return true;
  const ratio=width/height;
  const target=({'16:9':16/9, '16:10':16/10, '21:9':43/18})[id];
  return !!target && Math.abs(ratio/target-1) <= (id==='21:9' ? 0.035 : 0.006);
}
export function inferAspect(width, height) {
  return ASPECT_RATIOS.find(item=>item.id!=='custom' && aspectMatches(item.id,width,height))?.id || 'custom';
}
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
      !['fill','fit','pixel'].includes(value.scaling) || validateResolution(value.width,value.height))
    return {...DEFAULT_DISPLAY};
  const aspect=validAspect(value.aspect) && (value.mode==='viewport' || aspectMatches(value.aspect,value.width,value.height))
    ? value.aspect : inferAspect(value.width,value.height);
  const ratio = aspect==='custom' ? (validRatio(value.ratio_width,value.ratio_height)
    ? {ratio_width:value.ratio_width,ratio_height:value.ratio_height}
    : {ratio_width:value.width,ratio_height:value.height}) : {};
  return {mode:value.mode, width:value.width, height:value.height, scaling:value.scaling, aspect, ...ratio};
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

// The same mapping is passed to scene UI scripts, so callouts follow the GPU
// image through letterboxing, cropping, and integer-aligned odd-sized borders.
export function presentationMapping(source, output, mode) {
  const sw=Math.max(1,source.width), sh=Math.max(1,source.height);
  const ow=Math.max(1,output.width), oh=Math.max(1,output.height);
  if (mode==='pixel') {
    const factor=Math.max(1,Math.min(Math.floor(ow/sw),Math.floor(oh/sh)));
    const iw=sw*factor, ih=sh*factor;
    const dx=ow-iw, dy=oh-ih;
    return {scale:[ow/iw,oh/ih], offset:[(dx/2-Math.floor(dx/2))/iw,(dy/2-Math.floor(dy/2))/ih],
      factor, cropped:iw>ow || ih>oh};
  }
  const ratio=(ow/oh)/(sw/sh);
  const scale=mode==='fill' ? (ratio<1 ? [ratio,1] : [1,1/ratio]) :
    (ratio<1 ? [1,1/ratio] : [ratio,1]);
  return {scale,offset:[0,0],factor:null,cropped:false};
}
// Presets are filtered against both the GPU dimension and total-pixel limits.
// Custom sizes scale from the selected ratio, rounded by at most one pixel.
export function resolutionOptions(aspect, reference, viewport, deviceLimit=MAX_RENDER_DIMENSION, ratioReference=reference) {
  if (!validAspect(aspect)) return [];
  const ref=reference?.width>0 && reference?.height>0 ? reference : {width:1280,height:720};
  const ratio=validRatio(ratioReference?.width,ratioReference?.height) ? ratioReference : ref;
  const pairs=aspect==='custom' ? CUSTOM_HEIGHTS.map(h=>[Math.round(h*ratio.width/ratio.height),h]) : COMMON_RESOLUTIONS[aspect];
  const found=new Map();
  const add=(width,height)=>{
    if (!validateResolution(width,height,deviceLimit) && (aspect==='custom' || aspectMatches(aspect,width,height)))
      found.set(`${width}x${height}`,{id:`${width}x${height}`,label:`${width} × ${height}`,width,height});
  };
  for (const [width,height] of pairs) add(width,height);
  add(ref.width,ref.height);
  return [...found.values()].sort((a,b)=>a.width*a.height-b.width*b.height);
}
// Persistence and validation stay in the browser host. The pause scene owns
// presentation and sends setting requests through ECS events.
export function createDisplaySettings(storage, getDeviceLimit) {
  const key='countersplat.display.v1';
  let preferences;
  try { preferences=normalizeDisplay(JSON.parse(storage?.getItem(key) ?? 'null')); }
  catch { preferences={...DEFAULT_DISPLAY}; }
  const save=next=>{ preferences=next; try { storage?.setItem(key,JSON.stringify(preferences)); } catch { /* Live settings still work. */ } };
  return {
    get preferences() { return preferences; },
    set(next) {
      if (!next || typeof next !== 'object' || !['fixed','viewport'].includes(next.mode) ||
          !['fill','fit','pixel'].includes(next.scaling) || (next.aspect !== undefined && !validAspect(next.aspect)))
        return {ok:false,error:'Invalid display setting.'};
      const error=validateResolution(next.width,next.height,next.mode==='fixed' ? getDeviceLimit() : MAX_RENDER_DIMENSION);
      if (error) return {ok:false,error};
      const aspect=validAspect(next.aspect) && (next.mode==='viewport' || aspectMatches(next.aspect,next.width,next.height))
        ? next.aspect : inferAspect(next.width,next.height);
      let ratio={};
      if (aspect==='custom') {
        const sameSize=preferences.mode==='fixed' && preferences.aspect==='custom' &&
          preferences.width===next.width && preferences.height===next.height;
        const previous=validRatio(preferences.ratio_width,preferences.ratio_height) && sameSize && !next.reset_ratio;
        const ratioWidth=next.ratio_width ?? (previous ? preferences.ratio_width : next.width);
        const ratioHeight=next.ratio_height ?? (previous ? preferences.ratio_height : next.height);
        if (!validRatio(ratioWidth,ratioHeight)) return {ok:false,error:'Invalid custom aspect ratio.'};
        ratio={ratio_width:ratioWidth,ratio_height:ratioHeight};
      }
      save({mode:next.mode,width:next.width,height:next.height,scaling:next.scaling,aspect,...ratio});
      return {ok:true};
    },
    setAspect(aspect,viewport) {
      if (!validAspect(aspect)) return {ok:false,error:'Invalid aspect ratio.'};
      const reference=sceneResolution(preferences,viewport);
      if (aspect==='custom') {
        const source=validateResolution(reference.width,reference.height,getDeviceLimit()) ? preferences : reference;
        save({...preferences,mode:'fixed',width:source.width,height:source.height,aspect,ratio_width:source.width,ratio_height:source.height});
        return {ok:true};
      }
      const options=resolutionOptions(aspect,reference,viewport,getDeviceLimit()).filter(item=>item.id!=='viewport');
      if (!options.length) return {ok:false,error:'No supported resolutions for this aspect ratio.'};
      const pixels=Math.max(1,reference.width*reference.height);
      options.sort((a,b)=>Math.abs(Math.log(a.width*a.height/pixels))-Math.abs(Math.log(b.width*b.height/pixels)));
      save({mode:'fixed',width:options[0].width,height:options[0].height,scaling:preferences.scaling,aspect});
      return {ok:true};
    },
  };
}
export function createHudPreferences(storage) {
  const key='countersplat.hud.show-performance.v1';
  let showPerformance=true;
  try { const value=storage?.getItem(key); if (value==='true' || value==='false') showPerformance=value==='true'; }
  catch { /* Default remains visible. */ }
  return {
    get showPerformance() { return showPerformance; },
    set(value) {
      if (typeof value!=='boolean') return false;
      showPerformance=value;
      try { storage?.setItem(key,String(value)); } catch { /* Live setting still applies. */ }
      return true;
    },
  };
}
