const w=CASE.w,h=CASE.h,random=rng(73);
let gpu=new Uint32Array(w*h);
const put=(x,y)=>{gpu[((y+h)%h)*w+(x+w)%w]=1;};
if(CASE.name==='block') [[3,3],[3,4],[4,3],[4,4]].forEach(([x,y])=>put(x,y));
if(CASE.name==='blinker') [[3,4],[4,4],[5,4]].forEach(([x,y])=>put(x,y));
if(CASE.name==='wrap-glider') [[w-2,h-3],[w-1,h-2],[w-3,h-1],[w-2,h-1],[w-1,h-1]].forEach(([x,y])=>put(x,y));
if(CASE.name==='seeded') gpu=gpu.map(()=>random()<.4?1:0);
if(CASE.name==='full') gpu.fill(1);
let cpu=gpu.slice(),comparisons=0;
function next(input) {
 const out=new Uint32Array(w*h);
 for(let y=0;y<h;y++)for(let x=0;x<w;x++) {
  let count=0;
  for(let dy=-1;dy<=1;dy++)for(let dx=-1;dx<=1;dx++)if(dx||dy)count+=input[((y+dy+h)%h)*w+(x+dx+w)%w];
  out[y*w+x]=count===3||(input[y*w+x]===1&&count===2)?1:0;
 }
 return out;
}
const compute=program(SHADER,'main',[8,gpu.byteLength,gpu.byteLength],[2]);
for(let step=0;step<CASE.steps;step++) {
 cpu=next(cpu);gpu=new Uint32Array((await compute([new Uint32Array([w,h]),gpu,new Uint32Array(w*h)],[w/8,h/8,1]))[0]);
 for(let i=0;i<cpu.length;i++){comparisons++;assert(gpu[i]===cpu[i],`life ${CASE.name} step=${step} i=${i}`);}
}
console.log(JSON.stringify({sample:'life',case:CASE.name,width:w,height:h,steps:CASE.steps,comparisons,exact:true}));
device.destroy();
