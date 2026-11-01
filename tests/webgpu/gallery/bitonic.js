const n=CASE.n,random=rng(99);
let gpu=Uint32Array.from({length:n},(_,i)=>CASE.name==='reverse'?n-i:CASE.name==='equal'?7:CASE.name==='sorted'?i:Math.floor(random()*9));
// Include unsigned extremes and duplicates without any floating-point conversion.
if(CASE.name==='seeded'){gpu[0]=4294967295;gpu[1]=0;gpu[2]=4294967295;}
const expected=Array.from(gpu).sort((a,b)=>a-b);
const compute=program(SHADER,'computeMain',[gpu.byteLength,gpu.byteLength,16,4],[1,3]);
let steps=0,comparisons=0,totalSwaps=0;
for(let span=2;span<=n;span*=2)for(let height=span;height>=2;height/=2){
 const flip=height===span,local=height<=16,algo=flip?(local?1:3):(local?2:4);
 const cpu=gpu.slice();let swaps=0;
 for(let base=0;base<n;base+=height)for(let i=0;i<height/2;i++){
  const a=base+i,b=base+(flip?height-i-1:i+height/2);
  if(cpu[b]<cpu[a]){const t=cpu[a];cpu[a]=cpu[b];cpu[b]=t;swaps++;}
 }
 const params=new ArrayBuffer(16);new Float32Array(params,0,2).set([n,1]);new Uint32Array(params,8,2).set([algo,height]);
 // Explicitly seed output with input: upstream global phases write swapped pairs only.
 const result=await compute([gpu,gpu.slice(),params,new Uint32Array([0])],[n/16,1,1]);
 gpu=new Uint32Array(result[0]);const counter=new Uint32Array(result[1])[0];
 assert(counter===(local?swaps:0),`atomic swap count got=${counter} expected=${local?swaps:0}`);
 for(let i=0;i<n;i++){comparisons++;assert(gpu[i]===cpu[i],`bitonic ${CASE.name} span=${span} height=${height} i=${i}`);}
 totalSwaps+=counter;steps++;
}
assert(JSON.stringify(Array.from(gpu))===JSON.stringify(expected),'final sort differs from independent numeric sort');
console.log(JSON.stringify({sample:'bitonic',case:CASE.name,elements:n,steps,comparisons,localAtomicSwaps:totalSwaps,exact:true}));
device.destroy();
