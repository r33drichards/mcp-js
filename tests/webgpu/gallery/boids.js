// Independent scalar reference: double intermediates, f32 input/state boundaries.
// Tolerance accounts for GPU f32 sums, normalize/sqrt and fused arithmetic.
function reference(input,p) {
  const n=input.length/4, out=new Float32Array(input.length);
  for(let i=0;i<n;i++) {
    let x=input[4*i], y=input[4*i+1], vx=input[4*i+2], vy=input[4*i+3];
    let mx=0,my=0,cx=0,cy=0,sx=0,sy=0,mc=0,cc=0;
    for(let j=0;j<n;j++) if(i!==j) {
      const dx=input[4*j]-x,dy=input[4*j+1]-y,dist=Math.hypot(dx,dy);
      if(dist<p[1]) {mx+=input[4*j];my+=input[4*j+1];mc++;}
      if(dist<p[2]) {sx-=dx;sy-=dy;}
      if(dist<p[3]) {cx+=input[4*j+2];cy+=input[4*j+3];cc++;}
    }
    if(mc) {mx=mx/mc-x;my=my/mc-y;}
    if(cc) {cx/=cc;cy/=cc;}
    vx+=mx*p[4]+sx*p[5]+cx*p[6];vy+=my*p[4]+sy*p[5]+cy*p[6];
    const length=Math.hypot(vx,vy);assert(length>1e-8,'undefined zero normalize excluded');
    const scale=Math.min(length,.1)/length;vx*=scale;vy*=scale;
    x+=vx*p[0];y+=vy*p[0];
    if(x< -1)x=1;if(x>1)x=-1;if(y< -1)y=1;if(y>1)y=-1;
    out.set([x,y,vx,vy],4*i);
  }
  return out;
}
const random=rng(42), n=CASE.n;
let gpu=new Float32Array(n*4);
for(let i=0;i<n;i++) gpu.set([random()*.4-.2,random()*.4-.2,random()*.08+.005,random()*.08-.04],4*i);
const params=new Float32Array([.04,.1,.025,.025,.02,.05,.005]);
if(CASE.boundary) {
  params[1]=params[2]=params[3]=0;
  for(let i=0;i<n;i++) gpu.set([i%2 ? -.999 : .999,i%4<2 ? -.999 : .999,i%2 ? -.3 : .3,i%4<2 ? -.2 : .2],4*i);
}
let cpu=gpu.slice(), maxError=0, comparisons=0;
const compute=program(SHADER,'main',[28,gpu.byteLength,gpu.byteLength],[2],CASE.uniform?[0]:[]);
for(let step=0;step<CASE.steps;step++) {
  cpu=reference(cpu,params);
  gpu=new Float32Array((await compute([params,gpu,new Float32Array(n*4)],[n/64,1,1]))[0]);
  for(let i=0;i<cpu.length;i++) {
    const error=Math.abs(gpu[i]-cpu[i]);maxError=Math.max(maxError,error);comparisons++;
    assert(Number.isFinite(gpu[i]) && error<=2e-5+2e-4*Math.abs(cpu[i]),`boids ${CASE.name} step=${step} i=${i} got=${gpu[i]} expected=${cpu[i]} err=${error}`);
  }
}
console.log(JSON.stringify({sample:'boids',case:CASE.name,particles:n,steps:CASE.steps,comparisons,maxAbsoluteError:maxError}));
device.destroy();
