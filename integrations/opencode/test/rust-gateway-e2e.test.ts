import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { createServer } from "node:http"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import test from "node:test"
import { AeonMemoryPlugin } from "../src/aeon-memory.ts"

async function freePort(): Promise<number> {
  const server = createServer()
  await new Promise<void>((resolveReady) => server.listen(0, "127.0.0.1", resolveReady))
  const address = server.address()
  if (!address || typeof address === "string") throw new Error("no TCP address")
  const port = address.port
  await new Promise<void>((resolveClose) => server.close(() => resolveClose()))
  return port
}

async function waitForHealth(url: string, child: ReturnType<typeof spawn>): Promise<void> {
  const deadline = Date.now() + 20_000
  while (Date.now() < deadline) {
    if (child.exitCode != null) throw new Error(`aeon-memory-server exited with ${child.exitCode}`)
    try {
      if ((await fetch(`${url}/health`)).ok) return
    } catch {}
    await new Promise((resolveWait) => setTimeout(resolveWait, 50))
  }
  throw new Error("timed out waiting for aeon-memory-server")
}

test("real Rust gateway rewrites OpenCode messages for mild, aggressive deletion, and MMD injection", { timeout: 180_000 }, async () => {
  const repo = resolve(import.meta.dirname, "../../..")
  const binary = join(repo, "target", "debug", process.platform === "win32" ? "aeon-memory-server.exe" : "aeon-memory-server")
  const build = spawnSync("cargo", ["build", "-p", "aeon-memory-gateway", "--bin", "aeon-memory-server"], { cwd: repo, encoding: "utf8" })
  assert.equal(build.status, 0, build.stderr)

  const root = await mkdtemp(join(tmpdir(), "aeon-memory-opencode-rust-e2e-"))
  const gatewayPort = await freePort()
  const llmPort = await freePort()
  const llm = createServer(async (request, response) => {
    const chunks: Buffer[] = []
    for await (const chunk of request) chunks.push(Buffer.from(chunk))
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8")) as { messages?: Array<{ role?: string; content?: string }> }
    const system = body.messages?.find((message) => message.role === "system")?.content || ""
    const user = body.messages?.filter((message) => message.role === "user").at(-1)?.content || ""
    let content: string
    if (system.includes("工具结果摘要器")) {
      const callID = user.match(/tool_call_id: ([^\n]+)/)?.[1] || "call-1"
      content = JSON.stringify([{ tool_call_id: callID, tool_call: "read({})", summary: "mild gateway summary", timestamp: "2026-07-13T00:00:00Z", score: 9 }])
    } else if (system.includes("任务生命周期门神")) {
      content = JSON.stringify({ taskCompleted: false, isContinuation: user.includes("001-e2e.mmd"), isLongTask: true, continuationMmdFile: user.includes("001-e2e.mmd") ? "001-e2e.mmd" : null, newTaskLabel: "e2e" })
    } else if (system.includes("任务拓扑架构师")) {
      content = JSON.stringify({ file_action: "write", mmd_content: "%%{\"taskGoal\":\"schema bridge\"}%%\nflowchart TD\n001-N1[\"doing bridge\"]", replace_blocks: [], node_mapping: { "call-1": "001-N1" } })
    } else {
      content = "[]"
    }
    const payload = JSON.stringify({ choices: [{ message: { role: "assistant", content } }] })
    response.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(payload) })
    response.end(payload)
  })
  await new Promise<void>((resolveReady) => llm.listen(llmPort, "127.0.0.1", resolveReady))

  const config = join(root, "aeon-memory.yaml")
  const offloadRoot = join(root, "context-offload")
  await writeFile(config, `server:\n  host: 127.0.0.1\n  port: ${gatewayPort}\ndata:\n  baseDir: ${JSON.stringify(join(root, "memory"))}\nllm:\n  baseUrl: http://127.0.0.1:${llmPort}/v1\n  apiKey: test\n  model: mock\nmemory:\n  extraction:\n    enabled: false\n  embedding:\n    provider: none\n  offload:\n    enabled: true\n    mode: local\n    dataDir: ${JSON.stringify(offloadRoot)}\n    forceTriggerThreshold: 1\n    l2NullThreshold: 1\n    mildOffloadRatio: 0\n    aggressiveCompressRatio: 0.5\n    mmdMaxTokenRatio: 0.8\n`)
  const gateway = spawn(binary, ["--config", config], { cwd: repo, stdio: ["ignore", "ignore", "pipe"] })
  let stderr = ""
  gateway.stderr?.on("data", (chunk) => { stderr += String(chunk) })

  try {
    const gatewayUrl = `http://127.0.0.1:${gatewayPort}`
    await waitForHealth(gatewayUrl, gateway)
    const sessionID = "s1"
    const user = { info: { id: "u1", sessionID, role: "user", time: { created: 1 } }, parts: [{ id: "p-u1", sessionID, messageID: "u1", type: "text", text: "build the schema bridge" }] }
    const originalToolPayload = "original tool payload ".repeat(100)
    const assistant = { info: { id: "a1", sessionID, role: "assistant", parentID: "u1", time: { created: 2, completed: 3 }, finish: "tool-calls" }, parts: [{ id: "p-tool", sessionID, messageID: "a1", type: "tool", callID: "call-1", tool: "read", state: { status: "completed", input: { path: "README" }, output: originalToolPayload, title: "read", metadata: {}, time: { start: 2, end: 3 } } }] }
    const messages: any[] = [user, assistant]
    const client = { app: { log: async () => {} }, session: { messages: async () => ({ data: messages }) } }
    const offloadResponses: any[] = []
    const fetchImpl = async (input: string | URL | Request, init?: RequestInit) => {
      const response = await fetch(input, init)
      if (String(input).includes("/offload/")) offloadResponses.push(await response.clone().json())
      return response
    }
    const factory = AeonMemoryPlugin.create({ fetchImpl })
    const hooks: any = await factory({ client, directory: root }, {
      gatewayUrl,
      offloadEnabled: true,
      contextWindow: 1024,
      recallTimeoutMs: 5000,
      offloadTimeoutMs: 10000,
    })

    await hooks["chat.message"]({ sessionID }, { message: { id: "u1" }, parts: user.parts })
    const first = { messages: structuredClone([user]) }
    await hooks["experimental.chat.messages.transform"]({}, first)
    await hooks["experimental.chat.system.transform"]({ sessionID }, { system: ["system"] })
    await hooks["tool.execute.after"]({ tool: "read", sessionID, callID: "call-1", args: { path: "README" } }, { output: originalToolPayload, title: "read", metadata: {} })

    const jsonl = join(offloadRoot, "opencode", "offload-s1.jsonl")
    // A subsequent prompt makes the completed pair old enough for the real
    // L3 mild scan. This exercises the plugin's canonical response merge, not
    // merely L1 persistence.
    const progress = { info: { id: "a2", sessionID, role: "assistant", parentID: "u1", time: { created: 4, completed: 5 }, finish: "stop" }, parts: [{ id: "p-a2", sessionID, messageID: "a2", type: "text", text: "tool result reviewed" }] }
    const continuation = { info: { id: "u2", sessionID, role: "user", time: { created: 6 } }, parts: [{ id: "p-u2", sessionID, messageID: "u2", type: "text", text: "continue the schema bridge" }] }
    messages.push(progress, continuation)
    await hooks["chat.message"]({ sessionID }, { message: { id: "u2" }, parts: continuation.parts })
    const mild = { messages: structuredClone(messages) }
    await hooks["experimental.chat.messages.transform"]({}, mild)
    await hooks["experimental.chat.system.transform"]({ sessionID }, { system: ["system"] })
    assert.ok(mild.messages.every((message: any) => message.info?.id && Array.isArray(message.parts)))
    const mildAssistant = mild.messages.find((message: any) => message.info.id === "a1")
    assert.ok(mildAssistant, "mild rewrite must preserve the original OpenCode message identity")
    const mildTool = mildAssistant.parts.find((part: any) => part.type === "tool" && part.callID === "call-1")
    assert.ok(mildTool, "mild rewrite must keep the OpenCode tool call/result pair legal")
    assert.match(mildTool.state.output, /mild gateway summary/, JSON.stringify(offloadResponses.at(-1)))
    assert.notEqual(mildTool.state.output, originalToolPayload)
    assert.match(await readFile(jsonl, "utf8"), /"offloaded":true/)

    const hugeUser = { info: { id: "u3", sessionID, role: "user", time: { created: 7 } }, parts: [{ id: "p-u3", sessionID, messageID: "u3", type: "text", text: `verify ${"large context ".repeat(900)}` }] }
    messages.push(hugeUser)
    await hooks["chat.message"]({ sessionID }, { message: { id: "u3" }, parts: hugeUser.parts })
    const transformed = { messages: structuredClone(messages) }
    await hooks["experimental.chat.messages.transform"]({}, transformed)
    await hooks["experimental.chat.system.transform"]({ sessionID }, { system: ["system"] })

    assert.ok(transformed.messages.every((message: any) => message.info?.id && Array.isArray(message.parts)))
    assert.equal(transformed.messages.some((message: any) => message.info.id === "a1"), false, "aggressive rewrite must delete the old tool message")
    const latest = transformed.messages.find((message: any) => message.info.id === "u3")
    assert.ok(latest)
    assert.ok(latest.parts.some((part: any) => part.synthetic === true && part.metadata?.aeonOffloadContext === true && String(part.text).includes("current_task_context")), "MMD must be expressed as a legal synthetic OpenCode text part")
    assert.match(await readFile(jsonl, "utf8"), /"offloaded":"deleted"/)
  } finally {
    gateway.kill("SIGTERM")
    await new Promise<void>((resolveExit) => gateway.once("exit", () => resolveExit()))
    await new Promise<void>((resolveClose) => llm.close(() => resolveClose()))
    await rm(root, { recursive: true, force: true })
    assert.equal(gateway.exitCode, 0, stderr)
    assert.equal(gateway.signalCode, null, stderr)
  }
})
