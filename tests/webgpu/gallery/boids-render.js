// Both official shaders unchanged; headless host and readback replace browser UI.
const n=64,width=256,height=256,pitch=1024;
const initial=new Float32Array(n*4);
for(let i=0;i<n;i++)initial.set([-.875+(i%8)*.25,-.875+Math.floor(i/8)*.25,0,.05],i*4);
const params=new Float32Array([.04,0,0,0,.02,.05,.005]);
const compute=program(SHADER,'main',[28,initial.byteLength,initial.byteLength],[2],[0]);
const positions=new Float32Array((await compute([params,initial,new Float32Array(n*4)],[1,1,1]))[0]);
let maxError=0;
for(let i=0;i<n;i++)for(let k=0;k<4;k++){
 const expected=k===1?Math.fround(initial[4*i+1]+params[0]*initial[4*i+3]):initial[4*i+k];
 const error=Math.abs(positions[4*i+k]-expected);maxError=Math.max(maxError,error);assert(error<2e-5,'Boids compute position mismatch');
}
const target=device.createTexture({size:[width,height],format:'rgba8unorm',usage:17});
const output=device.createBuffer({size:pitch*height,usage:9});
const vertexData=[positions,new Float32Array([-.05,-.05,.05,-.05,0,.05])];
const vertices=vertexData.map(data=>{const b=device.createBuffer({size:data.byteLength,usage:GPUBufferUsage.VERTEX|GPUBufferUsage.COPY_DST});device.queue.writeBuffer(b,0,data);return b;});
const module=device.createShaderModule({code:SPRITE});
const pipeline=device.createRenderPipeline({layout:'auto',vertex:{module,entryPoint:'vert_main',buffers:[{arrayStride:16,stepMode:'instance',attributes:[{shaderLocation:0,offset:0,format:'float32x2'},{shaderLocation:1,offset:8,format:'float32x2'}]},{arrayStride:8,attributes:[{shaderLocation:2,offset:0,format:'float32x2'}]}]},fragment:{module,entryPoint:'frag_main',targets:[{format:'rgba8unorm'}]}});
const encoder=device.createCommandEncoder();const pass=encoder.beginRenderPass({colorAttachments:[{view:target.createView(),clearValue:[0,0,0,1],loadOp:'clear',storeOp:'store'}]});
pass.setPipeline(pipeline);vertices.forEach((b,i)=>pass.setVertexBuffer(i,b));pass.draw(3,n);pass.end();encoder.copyTextureToBuffer({texture:target},{buffer:output,bytesPerRow:pitch},[width,height]);device.queue.submit([encoder.finish()]);
await output.mapAsync(GPUMapMode.READ);const pixels=new Uint8Array(output.getMappedRange());
let checked=0,colored=0;
for(let y=0;y<height;y++)for(let x=0;x<width;x++){
 const px=(x+.5)/width*2-1,py=1-(y+.5)/height*2,i=(y*width+x)*4;
 if(pixels[i]||pixels[i+1]||pixels[i+2])colored++;
 let boundary=false,inside=-1;
 for(let b=0;b<n;b++){
  const dx=px-positions[4*b],dy=py-positions[4*b+1];
  const e=[dy+.05,.025-.5*dy-dx,.025-.5*dy+dx];
  if(e.every(v=>v>.01)){inside=b;break;}
  if(e.every(v=>v>-.01))boundary=true;
 }
 if(inside>=0){const dx=px-positions[4*inside];const expected=[1-Math.sin(1)-.05,dx*100-.05+.1,Math.cos(.5),1].map(v=>Math.round(Math.max(0,Math.min(1,v))*255));for(let k=0;k<4;k++)assert(Math.abs(pixels[i+k]-expected[k])<=1,`Boids raster ${x},${y} channel ${k}`);checked++;}
 else if(!boundary){assert(pixels[i]===0&&pixels[i+1]===0&&pixels[i+2]===0&&pixels[i+3]===255,'Boids background mismatch');checked++;}
}
assert(colored>4000&&colored<6500,'Boids frame coverage implausible');
console.log(JSON.stringify({sample:'boids-compute-render',particles:n,unchangedShaders:true,maxPositionError:maxError,checkedPixels:checked,coloredPixels:colored}));
let binary='';for(let i=0;i<pixels.length;i+=16384)binary+=String.fromCharCode(...pixels.subarray(i,i+16384));
console.log('GPU_FRAME_boids:'+btoa(binary));
output.unmap();device.destroy();
