// Transfers bounded map requests to an isolated geometry worker. Requests are
// coalesced while it is busy; an old scene can never install a new texture.
export class MinimapRuntime {
  constructor(game, base, isCurrent, onResult, worker = new Worker(new URL('./minimap-worker.js', import.meta.url), {type:'module'}), flipXY = true) {
    this.game=game; this.isCurrent=isCurrent; this.onResult=onResult; this.worker=worker;
    this.state='loading'; this.pending=null; this.inFlight=null; this.sequence=0;
    worker.onmessage=({data})=>this.onMessage(data);
    worker.onerror=event=>this.fail(event.message || 'Map worker failed');
    worker.postMessage({type:'load',base:base.href,flipXY});
  }
  request(value) {
    const number=(n,lo,hi)=>Number.isFinite(n) && n>=lo && n<=hi;
    if (!value || !Array.isArray(value.position) || value.position.length!==3 ||
        !value.position.every(v=>number(v,-10_000_000,10_000_000)) ||
        !number(value.feet,-10_000_000,10_000_000) || !number(value.units,.5,2.5) ||
        !number(value.radius,.1,100) || !number(value.height,0,10) ||
        !number(value.thickness,.01,2) || !Number.isInteger(value.size) || !number(value.size,16,256)) return;
    if (this.state==='unavailable') { this.onResult({status:'unavailable'}); return; }
    this.pending={...value,position:value.position.slice()};
    this.pump();
  }
  pump() {
    if (!this.isCurrent() || this.state!=='ready' || this.inFlight || !this.pending) return;
    const value=this.pending; this.pending=null;
    const id=++this.sequence; this.inFlight=id;
    this.worker.postMessage({...value,type:'slice',id});
  }
  onMessage(data) {
    if (!this.isCurrent()) return;
    if (data?.type==='ready') { this.state='ready'; this.pump(); return; }
    if (data?.type==='unavailable') { this.state='unavailable'; this.pending=null; this.onResult({status:'unavailable'}); return; }
    if (data?.type==='error') {
      if (data.operation==='slice' && data.id!==this.inFlight) return;
      if (data.operation==='load') { this.fail(data.message); return; }
      this.inFlight=null; this.onResult({status:'error'}); this.pump(); return;
    }
    if (data?.type!=='slice' || data.id!==this.inFlight) return;
    this.inFlight=null;
    if (!(data.pixels instanceof Uint8Array) || !Number.isInteger(data.size) || data.size<16 || data.size>256 ||
        data.pixels.length!==data.size*data.size*4 || !Array.isArray(data.center) || data.center.length!==2 ||
        !data.center.every(Number.isFinite) || !Number.isFinite(data.radius) || data.radius<=0 || !Number.isFinite(data.height)) {
      this.onResult({status:'error'}); this.pump(); return;
    }
    try {
      this.game.set_gameplay_ui_image('level-minimap',data.size,data.size,data.pixels);
      this.onResult({status:'ready',center:data.center,radius:data.radius,height:data.height});
    } catch (error) { console.warn('Could not upload minimap:',error); this.onResult({status:'error'}); }
    this.pump();
  }
  fail(message) {
    if (!this.isCurrent()) return;
    this.state='unavailable'; this.pending=null; this.inFlight=null;
    this.onResult({status:'unavailable'});
    console.warn('Level map:',message);
  }
  stop() { this.worker.terminate(); this.pending=null; this.inFlight=null; }
}
