const width=64,height=64,pitch=256;
const target=device.createTexture({size:[width,height],format:'rgba8unorm',usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.COPY_SRC});
const output=device.createBuffer({size:pitch*height,usage:GPUBufferUsage.MAP_READ|GPUBufferUsage.COPY_DST});
const module=device.createShaderModule({code:SHADER});
const vertexBuffers=[];let layouts=[];
if(CASE.sprite){
 layouts=[{arrayStride:16,stepMode:'instance',attributes:[{shaderLocation:0,offset:0,format:'float32x2'},{shaderLocation:1,offset:8,format:'float32x2'}]},{arrayStride:8,attributes:[{shaderLocation:2,offset:0,format:'float32x2'}]}];
 // Zero X velocity and positive Y velocity give angle zero, so positions
 // have a simple analytical rasterization reference while using upstream WGSL.
 for(const data of [new Float32Array([0,0,0,.05]),new Float32Array([-.75,-.75,.75,-.75,0,.75])]){
  const b=device.createBuffer({size:data.byteLength,usage:GPUBufferUsage.VERTEX|GPUBufferUsage.COPY_DST});device.queue.writeBuffer(b,0,data);vertexBuffers.push(b);
 }
}
const pipeline=device.createRenderPipeline({layout:'auto',vertex:{module,entryPoint:CASE.sprite?'vert_main':'vs',buffers:layouts},fragment:{module,entryPoint:CASE.sprite?'frag_main':'fs',targets:[{format:'rgba8unorm'}]},primitive:{topology:'triangle-list'}});
const encoder=device.createCommandEncoder();
const pass=encoder.beginRenderPass({colorAttachments:[{view:target.createView(),clearValue:[0,0,0,1],loadOp:'clear',storeOp:'store'}]});
pass.setPipeline(pipeline);vertexBuffers.forEach((b,i)=>pass.setVertexBuffer(i,b));pass.draw(3,1);pass.end();
encoder.copyTextureToBuffer({texture:target},{buffer:output,bytesPerRow:pitch,rowsPerImage:height},[width,height]);
device.queue.submit([encoder.finish()]);await output.mapAsync(GPUMapMode.READ);
const pixels=new Uint8Array(output.getMappedRange());let checked=0,colored=0;
for(let y=0;y<height;y++)for(let x=0;x<width;x++){
 const px=(x+.5)/width*2-1,py=1-(y+.5)/height*2;
 const edges=[py+.75,.375-.5*py-px,.375-.5*py+px];
 const i=(y*width+x)*4;
 const inside=edges.every(v=>v>.04),outside=edges.some(v=>v<-.04);
 if(pixels[i]||pixels[i+1]||pixels[i+2])colored++;
 if(inside){
  if(CASE.sprite){
   const expected=[1-Math.sin(1)-.05,px*100-.05+.1,Math.cos(.5),1].map(v=>Math.round(Math.max(0,Math.min(1,v))*255));
   for(let k=0;k<4;k++)assert(Math.abs(pixels[i+k]-expected[k])<=1,`sprite pixel ${x},${y} channel${k} got${pixels[i+k]} expected${expected[k]}`);
  }else assert(pixels[i]===255&&pixels[i+1]===0&&pixels[i+2]===0&&pixels[i+3]===255,'triangle pixel mismatch');
  checked++;
 }else if(outside){assert(pixels[i]===0&&pixels[i+1]===0&&pixels[i+2]===0&&pixels[i+3]===255,'clear pixel mismatch');checked++;}
}
assert(colored>900&&colored<1400,'triangle coverage implausible');
console.log(JSON.stringify({sample:CASE.name,gpuRendered:true,checkedPixels:checked,coloredPixels:colored}));
console.log('GPU_FRAME_'+CASE.name+':'+btoa(String.fromCharCode(...pixels)));
output.unmap();device.destroy();
