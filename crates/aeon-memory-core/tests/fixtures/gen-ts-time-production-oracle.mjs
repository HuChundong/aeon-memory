import {execFileSync} from "node:child_process";
import {writeFileSync} from "node:fs";
import {join} from "node:path";
const root=process.env.AEON_MEMORY_TS_BASELINE;
if(!root)throw new Error("AEON_MEMORY_TS_BASELINE required");
const sha=execFileSync("git",["-C",root,"rev-parse","HEAD"],{encoding:"utf8"}).trim();
if(sha!=="4339e63650920871eb0e8888083a1779d114e3ae")throw new Error(`wrong baseline ${sha}`);
if(!process.env.AEON_MEMORY_TIME_ORACLE_CHILD){
  execFileSync("npx",["tsx",new URL(import.meta.url).pathname],{cwd:root,stdio:"inherit",env:{...process.env,AEON_MEMORY_TIME_ORACLE_CHILD:"1",TZ:"America/New_York"}});
  process.exit(0);
}
const time=await import(join(root,"src/utils/time.ts"));
const instant=new Date("2026-01-01T00:30:00.000Z");
time.initTimeModule({timezone:"Asia/Shanghai"});
const shanghai={date:time.formatLocalDate(instant),formatted:time.formatForLLM(instant),dayStart:time.startOfLocalDay(instant)};
time.initTimeModule({timezone:"system"});
const system={timezone:time.getActiveTimeZone(),date:time.formatLocalDate(instant),formatted:time.formatForLLM(instant)};
function nextRunAt(cleanTime,nowMs){const [h,m]=cleanTime.split(":").map(Number);const now=new Date(nowMs);const next=new Date(nowMs);next.setHours(h,m,0,0);if(next<=now)next.setDate(next.getDate()+1);return next.getTime()}
const dst={spring:nextRunAt("03:00",Date.parse("2026-03-07T09:00:00Z")),fall:nextRunAt("03:00",Date.parse("2026-10-31T08:00:00Z"))};
writeFileSync(new URL("time_production_oracle.json",import.meta.url),JSON.stringify({shanghai,system,dst},null,2)+"\n");
