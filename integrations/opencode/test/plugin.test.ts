import assert from "node:assert/strict"
import { readFile, mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises"
import { execFile } from "node:child_process"
import { createRequire } from "node:module"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { promisify } from "node:util"
import test from "node:test"
import { parse as parseJsonc } from "jsonc-parser/lib/esm/main.js"
import { AeonMemoryPlugin } from "../src/aeon-memory.ts"

interface TestPart {
  id?: string
  sessionID?: string
  messageID?: string
  type: string
  text?: string
  ignored?: boolean
  synthetic?: boolean
  metadata?: Record<string, unknown>
}

interface TestInfo {
  id: string
  sessionID: string
  role: string
  parentID?: string
  error?: unknown
  finish?: string
  agent?: string
  mode?: string
  summary?: boolean
  time?: { created?: number; completed?: number }
}

interface TestMessage {
  info: TestInfo
  parts: TestPart[]
}

interface TestOutput {
  messages: TestMessage[]
}

interface TestHooks {
  "chat.message": (input: { sessionID: string; messageID?: string }, output: { message?: { id: string }; parts: TestPart[] }) => Promise<void>
  "experimental.chat.messages.transform": (input: object, output: TestOutput) => Promise<void>
  "experimental.chat.system.transform"?: (input: object, output: { system: string[] }) => Promise<void>
  "experimental.session.compacting": (input: { sessionID: string }, output: { context: string[] }) => Promise<void>
  tool: Record<string, {
    execute: (args: Record<string, unknown>, context: { sessionID: string }) => Promise<string | { output: string }>
  }>
  event: (input: { event: { type: string; properties: Record<string, unknown> } }) => Promise<void>
  "tool.execute.after": (
    input: { tool: string; sessionID: string; callID: string; args: unknown },
    output: { output: string; title?: string; metadata?: unknown },
  ) => Promise<void>
}

interface RequestRecord {
  path: string
  body: Record<string, unknown> & {
    user_content?: string
    assistant_content?: string
    session_key?: string
    tool?: { toolName?: string; toolCallId?: string; params?: Record<string, unknown> }
    agent_id?: string
    messages?: unknown[]
    assistant_message?: unknown
    user_prompt?: string
    system_prompt?: string
  }
  headers: HeadersInit | undefined
}

interface HarnessOptions {
  messages?: TestMessage[]
  failPath?: string
  failCounts?: Record<string, number>
  recallContexts?: string[]
  recallResponses?: unknown[]
  beforePromptMessages?: TestMessage[]
  recallDelays?: number[]
  delayPath?: string
  delayMs?: number
  config?: Record<string, unknown>
  timers?: {
    setTimer?: (callback: () => void, milliseconds: number) => unknown
    clearTimer?: (handle: unknown) => void
  }
}

const execFileAsync = promisify(execFile)
const here = dirname(fileURLToPath(import.meta.url))
const fixture = JSON.parse(await readFile(join(here, "fixtures/session-messages.json"), "utf8")) as TestMessage[]
const runExitEvents = JSON.parse(await readFile(join(here, "fixtures/opencode-run-completed-exit.json"), "utf8")) as Array<{ type: string; properties: Record<string, unknown> }>

function harness({ messages = fixture, failPath, failCounts = {}, recallContexts, recallResponses, beforePromptMessages, recallDelays, delayPath, delayMs = 1000, config = {}, timers = {} }: HarnessOptions = {}) {
  const requests: RequestRecord[] = []
  const logs: Array<{ level: string; message: string }> = []
  const attempts = new Map<string, number>()
  const fetchImpl = async (url: string, options: RequestInit) => {
    const path = new URL(url).pathname
    if (typeof options.body !== "string") throw new Error("expected JSON request body")
    requests.push({ path, body: JSON.parse(options.body) as RequestRecord["body"], headers: options.headers })
    const attempt = (attempts.get(path) || 0) + 1
    attempts.set(path, attempt)
    if (delayPath === path) {
      await new Promise((resolve, reject) => {
        const timer = setTimeout(resolve, delayMs)
        options.signal?.addEventListener("abort", () => { clearTimeout(timer); reject(Object.assign(new Error("aborted"), { name: "AbortError" })) })
      })
    }
    if (failPath === path || attempt <= (failCounts[path] || 0)) {
      return { ok: false, status: 503, json: async () => ({}) }
    }
    if (path === "/recall") {
      if (recallDelays?.[attempt - 1]) await new Promise((resolve) => setTimeout(resolve, recallDelays[attempt - 1]))
      if (recallResponses && attempt <= recallResponses.length) {
        return { ok: true, status: 200, json: async () => recallResponses[attempt - 1] }
      }
      const context = recallContexts?.[attempt - 1] ?? "User prefers concise answers."
      return { ok: true, status: 200, json: async () => ({ context }) }
    }
    if (path === "/search/memories") return { ok: true, status: 200, json: async () => ({ results: "memory-results", total: 1, strategy: "fts" }) }
    if (path === "/search/conversations") return { ok: true, status: 200, json: async () => ({ results: "conversation-results", total: 1 }) }
    if (path === "/offload/before-prompt" && beforePromptMessages) {
      return { ok: true, status: 200, json: async () => ({ messages: structuredClone(beforePromptMessages) }) }
    }
    return { ok: true, status: 200, json: async () => ({ ok: true }) }
  }
  const client = {
    app: { log: async ({ body }: { body: { level: string; message: string } }) => { logs.push(body) } },
    session: { messages: async () => ({ data: messages }) },
  }
  const sourceFactory = AeonMemoryPlugin.create({
    fetchImpl,
    ...timers,
  })
  const factory = async (input: { client: typeof client; directory: string }) =>
    await sourceFactory(input, {
      gatewayUrl: "http://memory.test",
      recallTimeoutMs: 100,
      captureTimeoutMs: 100,
      sessionEndTimeoutMs: 100,
      offloadTimeoutMs: 100,
      ...config,
    }) as unknown as TestHooks
  return { factory, client, requests, logs }
}

function messageHistory(userID = "u1", sessionID = "s1", created = 1): TestOutput {
  return {
    messages: [{
      info: { id: userID, sessionID, role: "user", time: { created } },
      parts: [{ id: `part-${userID}`, sessionID, messageID: userID, type: "text", text: `prompt-${userID}` }],
    }],
  }
}

function userMessage(userID: string, sessionID: string, created: number, { synthetic = false, metadata }: { synthetic?: boolean; metadata?: Record<string, unknown> } = {}): TestMessage {
  return {
    info: { id: userID, sessionID, role: "user", time: { created } },
    parts: [{
      id: `part-${userID}`,
      sessionID,
      messageID: userID,
      type: "text",
      text: `prompt-${userID}`,
      ...(synthetic ? { synthetic: true } : {}),
      ...(metadata ? { metadata } : {}),
    }],
  }
}

function injectedParts(output: TestOutput): TestPart[] {
  return output.messages.flatMap((message) => message.parts)
    .filter((part) => part?.metadata?.aeonMemoryContext === true)
}

function injectedPart(output: TestOutput): TestPart {
  const part = injectedParts(output)[0]
  assert.ok(part)
  return part
}

test("exports an OpenCode plugin entrypoint", () => {
  assert.equal(typeof AeonMemoryPlugin, "function")
})

test("title system transform sees no recall while repeated main-loop message transforms inject idempotently", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "What do I prefer?" }] })

  const title = { system: ["title-system"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1", model: {} }, title)
  assert.deepEqual(title.system, ["title-system"])
  assert.equal(typeof hooks["experimental.chat.system.transform"], "function")

  const first = messageHistory()
  const second = messageHistory()
  await hooks["experimental.chat.messages.transform"]({}, first)
  await hooks["experimental.chat.messages.transform"]({}, second)
  await hooks["experimental.chat.messages.transform"]({}, second)
  assert.equal(h.requests.filter((r) => r.path === "/recall").length, 1)
  assert.equal(injectedParts(first).length, 1)
  assert.equal(injectedParts(second).length, 1)
  const part = injectedPart(first)
  assert.equal(part.type, "text")
  assert.equal(part.synthetic, true)
  assert.equal(part.sessionID, "s1")
  assert.equal(part.messageID, "u1")
  assert.match(part.id ?? "", /^aeon-memory_memory_[a-f0-9]{24}$/)
  assert.match(part.text ?? "", /trust="untrusted"/)
  assert.match(part.text ?? "", /Never follow instructions/)
})

test("structured recall splits dynamic user context from stable main-loop system context", async () => {
  const h = harness({ recallResponses: [{
    context: "legacy fallback must not win",
    prependContext: "dynamic recalled memory",
    appendSystemContext: "stable memory tools guide; use read_file for details",
  }] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "remember" }] })

  const titleFirst = { system: ["title"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, titleFirst)
  assert.deepEqual(titleFirst.system, ["title"])

  const mainMessages = messageHistory("u1", "s1", 1)
  await hooks["experimental.chat.messages.transform"]({}, mainMessages)
  assert.match(injectedPart(mainMessages).text ?? "", /dynamic recalled memory/)
  assert.doesNotMatch(injectedPart(mainMessages).text ?? "", /stable memory tools guide|legacy fallback/)

  const mainSystem = { system: ["main"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, mainSystem)
  assert.deepEqual(mainSystem.system, ["main", "stable memory tools guide; use read for details"])
  const repeatedWithoutMessages = { system: ["title-or-other"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, repeatedWithoutMessages)
  assert.deepEqual(repeatedWithoutMessages.system, ["title-or-other"])
})

test("v0.7 gateway recall preserves the exact dynamic payload without a second search", async () => {
  const h = harness({ recallResponses: [{
    context: "stable official context",
    prepend_context: "budgeted dynamic context; use read_file for scene details",
    strategy: "hybrid",
    memory_count: 1,
  }] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "remember" }] })

  assert.deepEqual(h.requests.map((request) => request.path), ["/recall"])
  const mainMessages = messageHistory("u1", "s1", 1)
  await hooks["experimental.chat.messages.transform"]({}, mainMessages)
  assert.match(injectedPart(mainMessages).text ?? "", /budgeted dynamic context; use read for scene details/)
  assert.doesNotMatch(injectedPart(mainMessages).text ?? "", /read_file/)
  assert.doesNotMatch(injectedPart(mainMessages).text ?? "", /stable official context/)

  const mainSystem = { system: ["main"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, mainSystem)
  assert.deepEqual(mainSystem.system, ["main", "stable official context"])
})

test("pre-v0.7 gateway recall keeps the compatibility search fallback", async () => {
  const h = harness({ recallResponses: [{ context: "stable official context", strategy: "hybrid", memory_count: 1 }] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "remember" }] })
  assert.deepEqual(h.requests.map((request) => request.path), ["/recall", "/search/memories"])
})

test("recall binds to the canonical output message ID rather than optional hook input ID", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "caller-supplied-id" }, {
    message: { id: "canonical-user-id" },
    parts: [{ id: "p1", sessionID: "s1", messageID: "canonical-user-id", type: "text", text: "hello" }],
  })

  const output = messageHistory("canonical-user-id")
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(injectedParts(output).length, 1)
  assert.equal(injectedPart(output).messageID, "canonical-user-id")
})

test("assistant completion clears only the context belonging to that completed user turn", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "remember me" }] })
  const before = messageHistory()
  await hooks["experimental.chat.messages.transform"]({}, before)
  assert.equal(injectedParts(before).length, 1)

  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  const after = messageHistory()
  await hooks["experimental.chat.messages.transform"]({}, after)
  assert.deepEqual(injectedParts(after), [])
})

test("a new chat message clears the prior query context even when the new recall is empty", async () => {
  const h = harness({ recallContexts: ["old turn context", ""] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "first" }] })
  const first = messageHistory("u1")
  await hooks["experimental.chat.messages.transform"]({}, first)
  assert.match(injectedPart(first).text ?? "", /old turn context/)

  await hooks["chat.message"]({ sessionID: "s1", messageID: "u2" }, { message: { id: "u2" }, parts: [{ type: "text", text: "second" }] })
  const second = messageHistory("u2", "s1", 2)
  await hooks["experimental.chat.messages.transform"]({}, second)
  assert.deepEqual(injectedParts(second), [])
})

test("a late completion from the previous turn does not clear the newer turn context", async () => {
  const h = harness({ recallContexts: ["old turn context", "new turn context"] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "first" }] })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u2" }, { message: { id: "u2" }, parts: [{ type: "text", text: "second" }] })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })

  const output = messageHistory("u2", "s1", 2)
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(injectedParts(output).length, 1)
  assert.match(injectedPart(output).text ?? "", /new turn context/)
  assert.doesNotMatch(injectedPart(output).text ?? "", /old turn context/)
})

test("a late recall response cannot overwrite the newer turn generation", async () => {
  const h = harness({
    recallContexts: ["late old context", "current new context"],
    recallDelays: [80, 0],
    config: { recallTimeoutMs: 500 },
  })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  const oldRecall = hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "first" }] })
  await new Promise((resolve) => setTimeout(resolve, 5))
  const newRecall = hooks["chat.message"]({ sessionID: "s1", messageID: "u2" }, { message: { id: "u2" }, parts: [{ type: "text", text: "second" }] })
  await Promise.all([oldRecall, newRecall])

  const output = messageHistory("u2", "s1", 2)
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(injectedParts(output).length, 1)
  assert.match(injectedPart(output).text ?? "", /current new context/)
  assert.doesNotMatch(injectedPart(output).text ?? "", /late old context/)
})

test("synthetic recalled context is excluded from capture text extraction", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  const output = messageHistory()
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(injectedParts(output).length, 1)
  assert.equal(AeonMemoryPlugin.test.textFromParts(output.messages[0].parts, 1000), "prompt-u1")
})

test("recall never leaks into another session or a newer unmatched user turn", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })

  const otherSession = messageHistory("u1", "s2", 1)
  await hooks["experimental.chat.messages.transform"]({}, otherSession)
  assert.deepEqual(injectedParts(otherSession), [])

  const newerTurn = messageHistory("u1", "s1", 1)
  newerTurn.messages.push(messageHistory("u2", "s1", 2).messages[0])
  await hooks["experimental.chat.messages.transform"]({}, newerTurn)
  assert.deepEqual(injectedParts(newerTurn), [])
})

test("compaction gate removes prior injection and skips exactly the compaction transform", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  const output = messageHistory("u1", "s1", 1)
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(injectedParts(output).length, 1)

  await hooks["experimental.session.compacting"]({ sessionID: "s1" }, { context: [] })
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.deepEqual(injectedParts(output), [])
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.deepEqual(injectedParts(output), [])
})

test("auto-continue rebinds unconsumed recall after compacted and stays idempotent", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  await hooks["experimental.session.compacting"]({ sessionID: "s1" }, { context: [] })
  await hooks["experimental.chat.messages.transform"]({}, messageHistory("u1", "s1", 1))
  await hooks.event({ event: { type: "session.compacted", properties: { sessionID: "s1" } } })

  const output = messageHistory("u1", "s1", 1)
  output.messages.push(userMessage("u2", "s1", 2, { synthetic: true, metadata: { compaction_continue: true } }))
  await hooks["experimental.chat.messages.transform"]({}, output)
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(injectedParts(output).length, 1)
  assert.equal(injectedPart(output).messageID, "u2")
})

test("compaction excludes both recall channels and post-compaction main loop receives both", async () => {
  const h = harness({ recallResponses: [{ prependContext: "dynamic-after-compact", appendSystemContext: "stable-after-compact" }] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  await hooks["experimental.session.compacting"]({ sessionID: "s1" }, { context: [] })
  const compacting = messageHistory("u1", "s1", 1)
  await hooks["experimental.chat.messages.transform"]({}, compacting)
  assert.deepEqual(injectedParts(compacting), [])
  const compactingSystem = { system: ["compact"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, compactingSystem)
  assert.deepEqual(compactingSystem.system, ["compact"])

  await hooks.event({ event: { type: "session.compacted", properties: { sessionID: "s1" } } })
  const continued = { messages: [userMessage("u1", "s1", 1), userMessage("u2", "s1", 2, { synthetic: true })] }
  await hooks["experimental.chat.messages.transform"]({}, continued)
  assert.equal(injectedPart(continued).messageID, "u2")
  const mainSystem = { system: ["main"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, mainSystem)
  assert.deepEqual(mainSystem.system, ["main", "stable-after-compact"])
})

test("overflow replay rebinds when compacted history no longer contains the original user", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "old-user" }, parts: [{ type: "text", text: "large request" }] })
  await hooks["experimental.session.compacting"]({ sessionID: "s1" }, { context: [] })
  await hooks["experimental.chat.messages.transform"]({}, messageHistory("old-user", "s1", 1))
  await hooks.event({ event: { type: "session.compacted", properties: { sessionID: "s1" } } })

  const replay = messageHistory("replayed-user", "s1", 2)
  await hooks["experimental.chat.messages.transform"]({}, replay)
  assert.equal(injectedParts(replay).length, 1)
  assert.equal(injectedPart(replay).messageID, "replayed-user")
})

test("compaction gates are isolated per session", async () => {
  const h = harness({ recallContexts: ["context one", "context two"] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "one" }] })
  await hooks["chat.message"]({ sessionID: "s2" }, { message: { id: "v1" }, parts: [{ type: "text", text: "two" }] })
  await hooks["experimental.session.compacting"]({ sessionID: "s1" }, { context: [] })

  const other = messageHistory("v1", "s2", 1)
  await hooks["experimental.chat.messages.transform"]({}, other)
  assert.equal(injectedParts(other).length, 1)
  const compacting = messageHistory("u1", "s1", 1)
  await hooks["experimental.chat.messages.transform"]({}, compacting)
  assert.deepEqual(injectedParts(compacting), [])
})

test("stale compacted events cannot rebind an old or superseded recall", async () => {
  const h = harness({ recallContexts: ["old context", "new context"] })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "one" }] })

  await hooks.event({ event: { type: "session.compacted", properties: { sessionID: "s1" } } })
  const bareStale = { messages: [userMessage("u1", "s1", 1), userMessage("u2", "s1", 2)] }
  await hooks["experimental.chat.messages.transform"]({}, bareStale)
  assert.deepEqual(injectedParts(bareStale), [])

  await hooks["experimental.session.compacting"]({ sessionID: "s1" }, { context: [] })
  await hooks["experimental.chat.messages.transform"]({}, messageHistory("u1", "s1", 1))
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u2" }, parts: [{ type: "text", text: "two" }] })
  await hooks.event({ event: { type: "session.compacted", properties: { sessionID: "s1" } } })
  const superseded = { messages: [userMessage("u1", "s1", 1), userMessage("u2", "s1", 2)] }
  await hooks["experimental.chat.messages.transform"]({}, superseded)
  assert.equal(injectedParts(superseded).length, 1)
  assert.equal(injectedPart(superseded).messageID, "u2")
  assert.match(injectedPart(superseded).text ?? "", /new context/)
})

test("completed is the primary capture trigger and duplicate idle is capture-only fallback", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  await Promise.all([
    hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } }),
    hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } }),
  ])
  const captures = h.requests.filter((r) => r.path === "/capture")
  assert.equal(captures.length, 1)
  const capture = captures[0]
  assert.ok(capture)
  assert.equal(capture.body.user_content, "Remember blue bicycles. API_KEY=[REDACTED]")
  assert.equal(capture.body.assistant_content, "I will remember that preference.")
  assert.match(capture.body.session_key ?? "", /^opencode:[a-f0-9]{16}:s1$/)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 0)
})

test("completed events capture their exact assistant instead of the newest visible turn", async () => {
  const messages = structuredClone(fixture)
  messages.push(
    { info: { id: "u2", sessionID: "s1", role: "user", time: { created: 4 } }, parts: [{ type: "text", text: "second preference" }] },
    { info: { id: "a2", sessionID: "s1", role: "assistant", parentID: "u2", time: { created: 5, completed: 6 }, finish: "stop" }, parts: [{ type: "text", text: "second response" }] },
  )
  const h = harness({ messages })
  const hooks = await h.factory({ client: h.client, directory: "/repo/exact-event" })

  await hooks.event({ event: { type: "message.updated", properties: { info: messages[1].info } } })
  await hooks.event({ event: { type: "message.updated", properties: { info: messages[3].info } } })

  const captures = h.requests.filter((request) => request.path === "/capture")
  assert.equal(captures.length, 2)
  assert.equal(captures[0]?.body.assistant_content, "I will remember that preference.")
  assert.equal(captures[1]?.body.assistant_content, "second response")
})

test("latest completed pair ignores title and compaction summary assistants", () => {
  const messages = structuredClone(fixture)
  messages.push(
    userMessage("internal-u", "s1", 10),
    {
      info: { id: "title-a", sessionID: "s1", role: "assistant", parentID: "internal-u", agent: "title", time: { created: 11, completed: 12 }, finish: "stop" },
      parts: [{ type: "text", text: "generated title" }],
    },
    {
      info: { id: "compact-a", sessionID: "s1", role: "assistant", parentID: "internal-u", mode: "compaction", summary: true, time: { created: 13, completed: 14 }, finish: "stop" },
      parts: [{ type: "text", text: "summary" }],
    },
  )
  const pair = AeonMemoryPlugin.test.latestCompletedPair(messages, 1000)
  assert.equal(pair?.assistantMessageID, fixture[1].info.id)
})

test("failed completed-event capture is retried by idle and failures never escape hooks", async () => {
  const h = harness({ failCounts: { "/capture": 1 } })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 2)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 0)
  assert.equal(h.logs.filter((l) => l.level === "warn").length, 1)
})

test("failed lifecycle flush is retryable without recapturing the pair", async () => {
  const h = harness({ failCounts: { "/session/end": 1 } })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  const deleted = { type: "session.deleted", properties: { info: { id: "s1" } } }
  await hooks.event({ event: deleted })
  await hooks.event({ event: deleted })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 1)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 2)
})

test("server disposal retries a session.deleted flush that failed once", async () => {
  const h = harness({ failCounts: { "/session/end": 1 } })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks.event({ event: { type: "session.deleted", properties: { info: { id: "s1" } } } })
  await hooks.event({ event: { type: "server.instance.disposed", properties: { directory: "/repo/a" } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 1)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 2)
})

test("timeout is fail-soft", async () => {
  const h = harness({ delayPath: "/recall" })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["chat.message"]({ sessionID: "s1", messageID: "u1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  const output = messageHistory()
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.deepEqual(injectedParts(output), [])
  assert.match(h.logs[0]?.message ?? "", /timeout/)
})

test("slow capture completes under its dedicated timeout without a per-turn flush", async () => {
  const h = harness({
    delayPath: "/capture",
    delayMs: 150,
    config: { captureTimeoutMs: 500, sessionEndTimeoutMs: 500 },
  })
  const hooks = await h.factory({ client: h.client, directory: "/repo/slow" })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 1)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 0)
  assert.equal(h.logs.length, 0)
})

test("each gateway path uses its dedicated timeout", async () => {
  const scheduled: number[] = []
  const h = harness({
    config: {
      recallTimeoutMs: 110,
      captureTimeoutMs: 120,
      sessionEndTimeoutMs: 130,
      offloadTimeoutMs: 140,
      offloadEnabled: true,
    },
    timers: {
      setTimer: (_callback, milliseconds) => { scheduled.push(milliseconds); return scheduled.length },
      clearTimer: () => {},
    },
  })
  const hooks = await h.factory({ client: h.client, directory: "/repo/timeouts" })
  await hooks["chat.message"]({ sessionID: "s1" }, { parts: [{ type: "text", text: "recall" }] })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  await hooks.event({ event: { type: "session.deleted", properties: { info: { id: "s1" } } } })
  await hooks["tool.execute.after"]({ tool: "read", sessionID: "s2", callID: "c1", args: {} }, { output: "ok" })
  assert.deepEqual(scheduled, [110, 120, 130, 140])
})

test("plugin options override individual defaults", () => {
  const config = AeonMemoryPlugin.test.configFromOptions({
    recallTimeoutMs: 123,
    captureTimeoutMs: 777,
  })
  assert.equal(config.recallTimeoutMs, 123)
  assert.equal(config.captureTimeoutMs, 777)
  assert.equal(config.sessionEndTimeoutMs, 120000)
  assert.equal(config.offloadTimeoutMs, 30000)
})

test("recall, capture, and tools can be disabled independently", async () => {
  const recallOff = harness({ config: { recallEnabled: false } })
  const recallOffHooks = await recallOff.factory({ client: recallOff.client, directory: "/repo/no-recall" })
  await recallOffHooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  await recallOffHooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  assert.deepEqual(recallOff.requests.map((request) => request.path), ["/capture"])
  assert.equal(Object.keys(recallOffHooks.tool).length, 2)

  const captureOff = harness({ config: { captureEnabled: false } })
  const captureOffHooks = await captureOff.factory({ client: captureOff.client, directory: "/repo/no-capture" })
  await captureOffHooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  await captureOffHooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  await captureOffHooks.event({ event: { type: "session.deleted", properties: { info: { id: "s1" } } } })
  assert.deepEqual(captureOff.requests.map((request) => request.path), ["/recall"])
  assert.equal(Object.keys(captureOffHooks.tool).length, 2)

  const toolsOff = harness({ config: { toolsEnabled: false } })
  const toolsOffHooks = await toolsOff.factory({ client: toolsOff.client, directory: "/repo/no-tools" })
  await toolsOffHooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  await toolsOffHooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  assert.deepEqual(toolsOff.requests.map((request) => request.path), ["/recall", "/capture"])
  assert.deepEqual(Object.keys(toolsOffHooks.tool), [])

  const disabled = harness({ config: { enabled: false, offloadEnabled: true } })
  const disabledHooks = await disabled.factory({ client: disabled.client, directory: "/repo/disabled" })
  await disabledHooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "hello" }] })
  await disabledHooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  await disabledHooks["tool.execute.after"]({ tool: "read", sessionID: "s1", callID: "c1", args: {} }, { output: "x" })
  assert.deepEqual(disabled.requests, [])
  assert.deepEqual(Object.keys(disabledHooks.tool), [])
})

test("default recall, capture, and lifecycle timeouts match host semantics", () => {
  const config = AeonMemoryPlugin.test.configFromOptions({})
  assert.equal(config.recallTimeoutMs, 5000)
  assert.equal(config.captureTimeoutMs, 10000)
  assert.equal(config.sessionEndTimeoutMs, 120000)
})

test("plugin options reject environment-shaped, unknown, and out-of-range values", () => {
  assert.throws(() => AeonMemoryPlugin.test.configFromOptions({ AEON_MEMORY_GATEWAY_URL: "http://memory.test" }), /Unknown aeon-memory option/)
  assert.throws(() => AeonMemoryPlugin.test.configFromOptions({ recallTimeoutMs: "5000" }), /must be an integer/)
  assert.throws(() => AeonMemoryPlugin.test.configFromOptions({ recallTimeoutMs: 50 }), /between 100 and 600000/)
  assert.throws(() => AeonMemoryPlugin.test.configFromOptions({ gatewayUrl: "memory.test" }), /HTTP\(S\) URL/)
  assert.throws(() => AeonMemoryPlugin.test.configFromOptions({ captureEnabled: "yes" }), /must be a boolean/)
})

test("idle captures when completed event arrived before messages became visible", async () => {
  const messages: TestMessage[] = []
  const h = harness({ messages })
  const hooks = await h.factory({ client: h.client, directory: "/repo/race" })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 0)

  messages.push(...structuredClone(fixture))
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 1)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 0)
})

test("session deletion ends once and after-tool offload is disabled by default", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["tool.execute.after"]({ tool: "read", sessionID: "s1", callID: "c1", args: {} }, { output: "x" })
  const deleted = { type: "session.deleted", properties: { info: { id: "s1" } } }
  await hooks.event({ event: deleted })
  await hooks.event({ event: deleted })
  assert.equal(h.requests.filter((r) => r.path === "/offload/after-tool").length, 0)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 1)
})

test("server disposal finalizes active sessions but ignores other directories", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  await hooks.event({ event: { type: "server.instance.disposed", properties: { directory: "/repo/other" } } })
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 0)

  await hooks.event({ event: { type: "server.instance.disposed", properties: { directory: "/repo/a" } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 1)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 1)
})

test("lifecycle finalization retries a failed last capture before flushing", async () => {
  const h = harness({ failCounts: { "/capture": 1 } })
  const hooks = await h.factory({ client: h.client, directory: "/repo/final" })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })
  await hooks.event({ event: { type: "session.deleted", properties: { info: { id: "s1" } } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 2)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 1)
})

test("opencode run completed event persists capture before immediate process exit without idle", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/run" })
  for (const event of runExitEvents) await hooks.event({ event })
  assert.deepEqual(h.requests.map((request) => request.path), ["/capture"])
})

test("repeated completed streaming events, idle, and deleted submit one turn exactly once", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/streaming" })
  const completed = { type: "message.updated", properties: { info: fixture[1].info } }
  await hooks.event({ event: completed })
  await hooks.event({ event: completed })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  await hooks.event({ event: { type: "session.deleted", properties: { info: { id: "s1" } } } })
  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 1)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 1)
})

test("one OpenCode session captures consecutive turns while late idle stays harmless", async () => {
  const messages = structuredClone(fixture)
  const h = harness({ messages })
  const hooks = await h.factory({ client: h.client, directory: "/repo/multiturn" })

  await hooks["chat.message"]({ sessionID: "s1" }, { parts: [{ type: "text", text: "turn one" }] })
  await hooks.event({ event: { type: "message.updated", properties: { info: messages[1].info } } })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })

  messages.push(
    { info: { id: "u2", sessionID: "s1", role: "user", time: { created: 4 } }, parts: [{ type: "text", text: "second preference" }] },
    { info: { id: "a2", sessionID: "s1", role: "assistant", parentID: "u2", time: { created: 5, completed: 6 }, finish: "stop" }, parts: [{ type: "text", text: "second response" }] },
  )
  await hooks["chat.message"]({ sessionID: "s1" }, { parts: [{ type: "text", text: "turn two" }] })
  // A stale idle sees the already-flushed first pair if the new response is not visible yet.
  const secondAssistant = messages.pop()
  const secondUser = messages.pop()
  assert.ok(secondAssistant)
  assert.ok(secondUser)
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  messages.push(secondUser, secondAssistant)
  await hooks.event({ event: { type: "message.updated", properties: { info: secondAssistant.info } } })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s1" } } })

  assert.equal(h.requests.filter((r) => r.path === "/capture").length, 2)
  assert.equal(h.requests.filter((r) => r.path === "/session/end").length, 0)
})

test("optional after-tool mapping uses the host-neutral DTO", async () => {
  const h = harness({ config: { offloadEnabled: true } })
  const hooks = await h.factory({ client: h.client, directory: "/repo/a" })
  await hooks["tool.execute.after"]({ tool: "read", sessionID: "s1", callID: "c1", args: { file: "README", apiKey: "do-not-store" } }, { output: "ok", title: "read" })
  const request = h.requests.find((r) => r.path === "/offload/after-tool")
  assert.ok(request)
  assert.ok(request.body.tool)
  assert.equal(request.body.tool.toolName, "read")
  assert.equal(request.body.tool.toolCallId, "c1")
  assert.equal(request.body.agent_id, "opencode")
  assert.equal(request.body.tool.params?.apiKey, "[REDACTED]")
  assert.ok(request.body.messages)
  assert.equal(request.body.messages.length, fixture.length)
})

test("memory search tools map HTTP DTOs and enforce a combined three-call turn limit", async () => {
  const h = harness()
  const hooks = await h.factory({ client: h.client, directory: "/repo/search" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "search" }] })
  const context = { sessionID: "s1" }
  assert.equal(await hooks.tool.aeon_memory_search.execute({ query: "preference", limit: 2, type: "persona" }, context), "memory-results")
  assert.equal(await hooks.tool.aeon_conversation_search.execute({ query: "exact words", session_key: "wanted" }, context), "conversation-results")
  assert.equal(await hooks.tool.aeon_memory_search.execute({ query: "event", scene: "work" }, context), "memory-results")
  assert.match(String(await hooks.tool.aeon_conversation_search.execute({ query: "fourth" }, context)), /limit reached/)
  assert.equal(h.requests.filter((request) => request.path.startsWith("/search/")).length, 3)
  const memory = h.requests.find((request) => request.path === "/search/memories")
  assert.ok(memory)
  assert.equal(memory.body.type, "persona")
  const conversation = h.requests.find((request) => request.path === "/search/conversations")
  assert.ok(conversation)
  assert.equal(conversation.body.session_key, "wanted")

  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u2" }, parts: [{ type: "text", text: "new turn" }] })
  assert.equal(await hooks.tool.aeon_memory_search.execute({ query: "reset" }, context), "memory-results")
})

test("offload lifecycle sends real prompt, tool, and assistant messages while excluding internal assistants", async () => {
  const h = harness({ config: { offloadEnabled: true } })
  const hooks = await h.factory({ client: h.client, directory: "/repo/offload" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "prompt-u1" }] })
  await hooks["experimental.chat.messages.transform"]({}, messageHistory("u1", "s1", 1))
  const system = { system: ["real OpenCode system"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, system)
  await hooks["tool.execute.after"]({ tool: "read", sessionID: "s1", callID: "call-1", args: { path: "README" } }, { output: "tool-result" })
  await hooks.event({ event: { type: "message.updated", properties: { info: fixture[1].info } } })

  const before = h.requests.find((request) => request.path === "/offload/before-prompt")
  assert.ok(before)
  assert.equal(before.body.system_prompt, "real OpenCode system")
  assert.equal(before.body.user_prompt, "prompt-u1")
  assert.ok(before.body.messages?.length)
  const after = h.requests.find((request) => request.path === "/offload/after-tool")
  assert.ok(after)
  assert.equal(after.body.messages?.length, fixture.length)
  const llm = h.requests.find((request) => request.path === "/offload/llm-output")
  assert.ok(llm)
  assert.ok(llm.body.assistant_message)

  const internal = structuredClone(fixture[1].info)
  internal.id = "compaction-a"
  internal.summary = true
  await hooks.event({ event: { type: "message.updated", properties: { info: internal } } })
  assert.equal(h.requests.filter((request) => request.path === "/offload/llm-output").length, 1)
  assert.equal(h.requests.filter((request) => request.path === "/capture").length, 1)
})

test("before-prompt waits for real main system, works without recall, rewrites by reference, and excludes title and compaction", async () => {
  const replacement = [userMessage("rewritten", "s1", 3)]
  const h = harness({
    config: { offloadEnabled: true },
    recallResponses: [{}],
    beforePromptMessages: replacement,
  })
  const hooks = await h.factory({ client: h.client, directory: "/repo/offload-system" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "without recall" }] })

  const title = { system: ["title-system"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, title)
  assert.equal(h.requests.filter((request) => request.path === "/offload/before-prompt").length, 0)

  const output = messageHistory("u1", "s1", 1)
  await hooks["experimental.chat.messages.transform"]({}, output)
  await hooks["experimental.chat.messages.transform"]({}, output)
  assert.equal(h.requests.filter((request) => request.path === "/offload/before-prompt").length, 0)
  const main = { system: ["base system", "workspace policy"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, main)
  const calls = h.requests.filter((request) => request.path === "/offload/before-prompt")
  assert.equal(calls.length, 1)
  assert.equal(calls[0]?.body.system_prompt, "base system\n\nworkspace policy")
  assert.equal(calls[0]?.body.user_prompt, "prompt-u1")
  assert.equal(output.messages[0]?.info.id, "rewritten")
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, { system: ["duplicate"] })
  assert.equal(h.requests.filter((request) => request.path === "/offload/before-prompt").length, 1)

  const compact = harness({ config: { offloadEnabled: true }, recallResponses: [{}] })
  const compactHooks = await compact.factory({ client: compact.client, directory: "/repo/offload-compact" })
  await compactHooks["chat.message"]({ sessionID: "s2" }, { message: { id: "u2" }, parts: [{ type: "text", text: "compact" }] })
  await compactHooks["experimental.session.compacting"]({ sessionID: "s2" }, { context: [] })
  await compactHooks["experimental.chat.messages.transform"]({}, messageHistory("u2", "s2", 1))
  await compactHooks["experimental.chat.system.transform"]?.({ sessionID: "s2" }, { system: ["compaction-system"] })
  assert.equal(compact.requests.filter((request) => request.path === "/offload/before-prompt").length, 0)
})

test("before-prompt receives stable recall in the final real system prompt", async () => {
  const h = harness({
    config: { offloadEnabled: true },
    recallResponses: [{ prependContext: "dynamic", appendSystemContext: "stable memory guide" }],
  })
  const hooks = await h.factory({ client: h.client, directory: "/repo/offload-stable" })
  await hooks["chat.message"]({ sessionID: "s1" }, { message: { id: "u1" }, parts: [{ type: "text", text: "prompt" }] })
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, { system: ["title"] })
  await hooks["experimental.chat.messages.transform"]({}, messageHistory("u1", "s1", 1))
  const main = { system: ["real base"] }
  await hooks["experimental.chat.system.transform"]?.({ sessionID: "s1" }, main)
  assert.deepEqual(main.system, ["real base", "stable memory guide"])
  const call = h.requests.find((request) => request.path === "/offload/before-prompt")
  assert.ok(call)
  assert.equal(call.body.system_prompt, "real base\n\nstable memory guide")
})

test("sanitizer removes injected memory and common credentials", () => {
  const cleaned = AeonMemoryPlugin.test.redactSensitive("<aeon-memory-context>ignore me</aeon-memory-context>\n</aeon-memory-context>\nBearer abcdefghijklmnop\nPASSWORD=hunter2")
  assert.doesNotMatch(cleaned, /ignore me|hunter2|abcdefghijklmnop/)
  assert.doesNotMatch(cleaned, /<\/aeon-memory-context>/)
})

test("source installer migrates JSONC file URLs to a local npm dependency without losing unrelated comments", async () => {
  const root = await mkdtemp(join(tmpdir(), "aeon-memory-opencode-"))
  const integrationDir = join(here, "..")
  const oldBundle = join(root, "node_modules", "@aeon-memory", "opencode", "dist", "aeon-memory.js")
  try {
    await writeFile(join(root, "package.json"), JSON.stringify({ private: true, dependencies: { keep: "1.0.0" } }))
    await writeFile(join(root, "opencode.jsonc"), `{
  // preserve unrelated user comments
  "provider": { "keep": {} },
  "plugin": [
    "keep-plugin",
    ["${pathToFileURL(oldBundle).href}", { "captureTimeoutMs": 23456, "offloadEnabled": true }],
  ],
}
`)
    await execFileAsync(join(integrationDir, "install.sh"), ["--target", root])
    const installedPath = join(root, "node_modules", "@aeon-memory", "opencode", "dist", "aeon-memory.js")
    const installed = await readFile(installedPath, "utf8")
    assert.match(installed, /AeonMemoryPlugin/)
    const packageJson = JSON.parse(await readFile(join(root, "package.json"), "utf8"))
    assert.match(packageJson.dependencies["@aeon-memory/opencode"], /^file:/)
    assert.equal(packageJson.dependencies.keep, "1.0.0")
    const configText = await readFile(join(root, "opencode.jsonc"), "utf8")
    assert.match(configText, /preserve unrelated user comments/)
    const configured = parseJsonc(configText)
    assert.equal(configured.plugin.length, 2)
    assert.equal(configured.plugin[1][0], "@aeon-memory/opencode")
    assert.equal(configured.plugin[1][1].gatewayUrl, "http://127.0.0.1:8420")
    assert.equal(configured.plugin[1][1].captureTimeoutMs, 23456)
    assert.equal(configured.plugin[1][1].offloadEnabled, true)
    await execFileAsync(join(integrationDir, "uninstall.sh"), ["--target", root])
    await assert.rejects(readFile(installedPath))
    const uninstalledPackageJson = JSON.parse(await readFile(join(root, "package.json"), "utf8"))
    assert.equal(uninstalledPackageJson.dependencies["@aeon-memory/opencode"], undefined)
    assert.equal(uninstalledPackageJson.dependencies.keep, "1.0.0")
    const afterText = await readFile(join(root, "opencode.jsonc"), "utf8")
    assert.match(afterText, /preserve unrelated user comments/)
    assert.deepEqual(parseJsonc(afterText), { provider: { keep: {} }, plugin: ["keep-plugin"] })
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("installer defaults to the published registry package and requires an explicit choice for duplicate config files", async () => {
  const root = await mkdtemp(join(tmpdir(), "aeon-memory-opencode-config-"))
  const cli = join(here, "..", "dist", "cli.js")
  try {
    const dryRun = await execFileAsync(cli, ["install", "--target", root, "--dry-run"])
    assert.match(dryRun.stdout, /Would run npm install: @aeon-memory\/opencode@0\.7\.0/)
    assert.match(dryRun.stdout, /Would configure: .*opencode\.jsonc/)
    await writeFile(join(root, "opencode.json"), "{}\n")
    await writeFile(join(root, "opencode.jsonc"), "{}\n")
    await assert.rejects(
      execFileAsync(cli, ["status", "--target", root]),
      (error: unknown) => Boolean(error && typeof error === "object" && "stderr" in error && typeof error.stderr === "string" && /Both OpenCode config files exist/.test(error.stderr)),
    )
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("installer rejects an incompatible OpenCode version before writing", async () => {
  const root = await mkdtemp(join(tmpdir(), "aeon-memory-opencode-old-"))
  const cli = join(here, "..", "dist", "cli.js")
  const oldOpenCode = join(here, "fixtures", "opencode-old.sh")
  try {
    const binDir = join(root, "bin")
    await mkdir(binDir)
    await writeFile(join(binDir, "opencode"), await readFile(oldOpenCode), { mode: 0o755 })
    await assert.rejects(
      execFileAsync(cli, ["install", "--target", root], {
        env: { ...process.env, PATH: `${binDir}:${process.env.PATH ?? ""}` },
      }),
      (error: unknown) => Boolean(
        error && typeof error === "object" &&
        "code" in error && error.code === 1 &&
        "stderr" in error && typeof error.stderr === "string" &&
        /requires >= 1\.17\.18/.test(error.stderr),
      ),
    )
    await assert.rejects(readFile(join(root, "node_modules", "@aeon-memory", "opencode", "dist", "aeon-memory.js")))
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("installer reports when OpenCode is newer than the tested experimental-hook range", async () => {
  const root = await mkdtemp(join(tmpdir(), "aeon-memory-opencode-newer-"))
  const cli = join(here, "..", "dist", "cli.js")
  try {
    const binDir = join(root, "bin")
    await mkdir(binDir)
    await writeFile(join(binDir, "opencode"), "#!/bin/sh\necho 1.18.0\n", { mode: 0o755 })
    const result = await execFileAsync(cli, ["status", "--target", root], {
      env: { ...process.env, PATH: `${binDir}:${process.env.PATH ?? ""}` },
    })
    assert.match(result.stdout, /newer than tested 1\.17\.20; experimental hooks require validation/)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("npm pack installs cleanly and exposes an OpenCode-loadable plugin", async () => {
  const root = await mkdtemp(join(tmpdir(), "aeon-memory-opencode-pack-"))
  const integrationDir = join(here, "..")
  const appDir = join(root, "clean-app")
  try {
    const packed = await execFileAsync("npm", ["pack", "--json", "--pack-destination", root], { cwd: integrationDir })
    const packResult: unknown = JSON.parse(packed.stdout)
    const packEntry = Array.isArray(packResult)
      ? packResult[0]
      : packResult && typeof packResult === "object"
        ? Object.values(packResult)[0]
        : undefined
    assert.ok(packEntry && typeof packEntry === "object" && "filename" in packEntry)
    const { filename } = packEntry as { filename: string }
    await execFileAsync("npm", ["init", "-y"], { cwd: root })
    await execFileAsync("npm", ["install", "--ignore-scripts", join(root, filename), "--prefix", appDir], { cwd: root })
    const modulePath = join(appDir, "node_modules", "@aeon-memory", "opencode", "dist", "aeon-memory.js")
    const installed = await import(`${pathToFileURL(modulePath).href}?clean=${Date.now()}`)
    assert.deepEqual(Object.keys(installed), ["AeonMemoryPlugin"])
    assert.equal(typeof installed.AeonMemoryPlugin, "function")

    const resolveFromInstalledApp = createRequire(join(appDir, "package.json"))
    const serverPath = resolveFromInstalledApp.resolve("@aeon-memory/opencode/server")
    assert.equal(await realpath(serverPath), await realpath(modulePath))
    const serverEntry = await import(`${pathToFileURL(serverPath).href}?server=${Date.now()}`)
    assert.equal(typeof serverEntry.AeonMemoryPlugin, "function")

    const hooks = await installed.AeonMemoryPlugin({
      directory: "/clean/repo",
      client: {
        app: { log: async () => {} },
        session: { messages: async () => ({ data: [] }) },
      },
    })
    assert.equal(typeof hooks["chat.message"], "function")
    assert.equal(typeof hooks["experimental.chat.system.transform"], "function")
    assert.equal(typeof hooks["experimental.chat.messages.transform"], "function")
    assert.equal(typeof hooks.event, "function")
    assert.equal(typeof hooks.tool.aeon_memory_search.execute, "function")
    assert.equal(typeof hooks.tool.aeon_conversation_search.execute, "function")
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
