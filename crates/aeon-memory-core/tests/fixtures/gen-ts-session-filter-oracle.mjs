#!/usr/bin/env node
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const root=process.env.AEON_MEMORY_TS_BASELINE;if(!root)throw Error("AEON_MEMORY_TS_BASELINE required");
const {SessionFilter,isNonInteractiveTrigger}=await import(pathToFileURL(join(root,"src/utils/session-filter.ts")));
const patterns=[" bench-* ","literal.+","x*y*z",""];
const keys=["agent:a:normal","agent:a:memory-scene-extract-1","agent:a:subagent:x","temp:slug","agent:a:bench-judge-3","preliteral.+post","x12y34z","xzy",""];
const triggers=[null,"cron","CRON","heartbeat","automation","schedule","manual"];
const filter=new SessionFilter(patterns);
const contexts=[{}, {sessionKey:"agent:a:normal"},{sessionKey:"agent:a:normal",sessionId:"memory-x"},{sessionKey:"agent:a:normal",trigger:"cron"},{sessionKey:"agent:a:bench-judge-3",trigger:"manual"}];
const out={patterns,keys:keys.map(key=>({key,skip:filter.shouldSkip(key)})),triggers:triggers.map(trigger=>({trigger,skip:isNonInteractiveTrigger(trigger??undefined,"agent:a:normal")})),keyTriggers:["agent:a:cron:x","agent:a:HEARTBEAT:x","agent:a:normal"].map(key=>({key,skip:isNonInteractiveTrigger(undefined,key)})),contexts:contexts.map(ctx=>({ctx,skip:filter.shouldSkipCtx(ctx)}))};
writeFileSync(process.env.AEON_MEMORY_ORACLE_OUTPUT||new URL("./session_filter_oracle.json",import.meta.url).pathname,JSON.stringify(out,null,2)+"\n");
