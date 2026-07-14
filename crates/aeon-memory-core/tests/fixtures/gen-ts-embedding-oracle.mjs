#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
const root=process.env.AEON_MEMORY_TS_BASELINE;if(!root)throw new Error("AEON_MEMORY_TS_BASELINE is required");
const sha=execFileSync("git",["-C",root,"rev-parse","HEAD"],{encoding:"utf8"}).trim();if(sha!=="4339e63650920871eb0e8888083a1779d114e3ae")throw new Error(`wrong baseline ${sha}`);
const requests=[];const responses={success:{data:[{index:1,embedding:[0,5]},{index:0,embedding:[3,0,4]}]},empty:{data:[]},missing:{},malformed:{data:[{index:0,embedding:[1,"invalid"]}]}};
const server=createServer(async(req,res)=>{let body="";for await(const chunk of req)body+=chunk;const name=req.url.split("/").at(-2);requests.push({name,authorization:req.headers.authorization,body:JSON.parse(body)});res.writeHead(200,{"content-type":"application/json"});res.end(JSON.stringify(responses[name]));});
await new Promise(r=>server.listen(0,"127.0.0.1",r));const port=server.address().port;
const {OpenAIEmbeddingService}=await import(join(root,"src/core/store/embedding.ts"));const output=[];
for(const name of ["success","empty","missing","malformed"]){const svc=new OpenAIEmbeddingService({provider:"openai",baseUrl:`http://127.0.0.1:${port}/${name}`,apiKey:"secret",model:"fixture-model",dimensions:3,maxInputChars:3,timeoutMs:1000});try{const value=await svc.embedBatch(name==="success"?["A😀B","xy"]:["x"]);output.push({name,ok:true,value:value.map(v=>Array.from(v))});}catch(error){output.push({name,ok:false,error:error instanceof Error?error.message:String(error)});}}
const emptySvc=new OpenAIEmbeddingService({provider:"openai",baseUrl:`http://127.0.0.1:${port}/unused`,apiKey:"secret",model:"fixture-model",dimensions:3});output.push({name:"empty_batch",ok:true,value:(await emptySvc.embedBatch([])).map(v=>Array.from(v))});
await new Promise(r=>server.close(r));writeFileSync(join(new URL(".",import.meta.url).pathname,"embedding_runtime_oracle.json"),JSON.stringify({requests,output},null,2)+"\n");
