import assert from 'node:assert/strict';
import fs from 'node:fs';
import {createAgentSession,DefaultResourceLoader,SessionManager,SettingsManager,ModelRuntime,createMcpExtension} from '@earendil-works/pi-coding-agent';
const cwd=process.cwd(),agentDir=cwd+'/.evidence/catalog-test';fs.mkdirSync(agentDir,{recursive:true});
const expected=['mcp__dashboards__run_js','mcp__scratch__run_js','mcp__dashboards__get_artifact','mcp__dashboards__list_artifacts'];
const settings=SettingsManager.inMemory({});
const loader=new DefaultResourceLoader({cwd,agentDir,settingsManager:settings,extensionFactories:[{name:"mcp",factory:createMcpExtension(),builtin:true}],noExtensions:true,additionalExtensionPaths:['builtin:mcp',cwd+'/agent.ts'],noSkills:true,noPromptTemplates:true,noThemes:true,noContextFiles:true});
await loader.reload();assert.deepEqual(loader.getExtensions().errors,[]);
const modelRuntime=await ModelRuntime.create({authPath:agentDir+'/unused-auth.json',modelsPath:null,modelsStorePath:agentDir+'/models.json',allowModelNetwork:false,refreshOnCreate:false});
const {session}=await createAgentSession({cwd,agentDir,modelRuntime,model:modelRuntime.getModel('openai','gpt-6.1-sol'),noTools:'builtin',tools:expected,resourceLoader:loader,settingsManager:settings,sessionManager:SessionManager.inMemory()});
try {
 await session.bindExtensions({mode:"rpc"});
 for(let i=0;i<100;i++){if(session.getActiveToolNames().length===expected.length)break;await new Promise(r=>setTimeout(r,100));}
 const active=session.getActiveToolNames().sort();
 console.log('Model-visible tools:',JSON.stringify(active));assert.deepEqual(active,[...expected].sort());
 const wire=session.agent.state.tools.map(t=>t.name).sort();assert.deepEqual(wire,active);
 assert(active.every(n=>n.startsWith('mcp__')));
 assert(!active.some(n=>/^(read|write|edit|bash|powershell|find|grep|ls|codemode)$/.test(n)));
 fs.writeFileSync('.evidence/model-visible-tools.json',JSON.stringify({active,wire,noModelCall:true,auth:'isolated empty test directory'},null,2));
 console.log('Full Pi agent wire catalog contains only the four intended MCP tools; no built-in shell/filesystem aliases.');
}finally{session.dispose();}

// The isolated test owns its transports; terminate remaining keepalive handles.
process.exit(0);
