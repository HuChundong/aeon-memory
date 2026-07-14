#!/usr/bin/env node
import { execFileSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, rmSync, rmdirSync, statSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { applyEdits, modify, parse as parseJsonc, type ParseError } from "jsonc-parser/lib/esm/main.js"

const MIN_OPENCODE = "1.17.18"
const PACKAGE_NAME = "@aeon-memory/opencode"
const LEGACY_PACKAGE_NAME = "@tencentdb-agent-memory/opencode"
const MARKER = 'PLUGIN_NAME = "aeon-memory"'
const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const sourcePlugin = join(packageRoot, "src", "aeon-memory.ts")
const sourceCli = join(packageRoot, "src", "cli.ts")
const builtPlugin = join(packageRoot, "dist", "aeon-memory.js")

type Command = "install" | "uninstall" | "status" | "config" | "help" | "--help"

interface Options {
  command?: Command | string
  force: boolean
  dryRun: boolean
  local: boolean
  target?: string
  config?: string
}

interface Detection {
  binary: string
  version?: string
  compatible?: boolean
}

function usage(): void {
  console.log(`Usage: aeon-memory-opencode <install|uninstall|status|config> [options]

Commands:
  install      Install the plugin and register its OpenCode configuration
  uninstall    Remove the recognized bundle and its configuration entry
  status       Show bundle, configuration, and OpenCode compatibility
  config       Print the default OpenCode plugin configuration

Options:
  --target DIR Override the OpenCode config directory
  --config FILE Override the exact OpenCode JSON or JSONC config file
  --local      Install the package from this source checkout (development only)
  --force      Install even when the detected OpenCode version is too old
  --dry-run    Print actions without writing files
`)
}

function parse(argv: string[]): Options {
  const result: Options = { command: argv[0], force: false, dryRun: false, local: false }
  for (let i = 1; i < argv.length; i += 1) {
    if (argv[i] === "--force") result.force = true
    else if (argv[i] === "--dry-run") result.dryRun = true
    else if (argv[i] === "--local") result.local = true
    else if (argv[i] === "--target" && argv[i + 1]) result.target = argv[++i]
    else if (argv[i] === "--config" && argv[i + 1]) result.config = argv[++i]
    else throw new Error(`Unknown or incomplete option: ${argv[i]}`)
  }
  if (result.target && result.config) throw new Error("Use either --target DIR or --config FILE, not both")
  return result
}

function configDir(option?: string): string {
  if (option) return resolve(option)
  const configHome = process.env.XDG_CONFIG_HOME || join(homedir(), ".config")
  return join(configHome, "opencode")
}

function configPath(dir: string, option?: string): string {
  if (option) return resolve(option)
  const candidates = [join(dir, "opencode.jsonc"), join(dir, "opencode.json")].filter(existsSync)
  if (candidates.length > 1) {
    throw new Error(`Both OpenCode config files exist; rerun with --config FILE: ${candidates.join(", ")}`)
  }
  return candidates[0] ?? join(dir, "opencode.jsonc")
}

const DEFAULT_OPTIONS = {
  enabled: true,
  gatewayUrl: "http://127.0.0.1:8420",
  recallTimeoutMs: 5000,
  captureTimeoutMs: 10000,
  sessionEndTimeoutMs: 120000,
  offloadTimeoutMs: 30000,
  recallMaxChars: 12000,
  captureMaxChars: 40000,
  offloadEnabled: false,
  contextWindow: 200000,
}

interface ConfigDocument {
  text: string
  value: Record<string, unknown>
}

function readConfig(path: string): ConfigDocument {
  const text = existsSync(path)
    ? readFileSync(path, "utf8")
    : '{\n  "$schema": "https://opencode.ai/config.json"\n}\n'
  const errors: ParseError[] = []
  const parsed = parseJsonc(text, errors, { allowTrailingComma: true, disallowComments: false })
  if (errors.length > 0) {
    throw new Error(`OpenCode config is not valid JSON/JSONC: ${path}`)
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`OpenCode config must contain a JSON object: ${path}`)
  }
  return { text, value: parsed as Record<string, unknown> }
}

function pluginSpec(entry: unknown): string | undefined {
  if (typeof entry === "string") return entry
  if (Array.isArray(entry) && typeof entry[0] === "string") return entry[0]
  return undefined
}

function isKnownPluginSpec(spec: string | undefined, oldBundle: string, legacy: string): boolean {
  if (!spec) return false
  if (spec === PACKAGE_NAME || spec === LEGACY_PACKAGE_NAME) return true
  try {
    const file = resolve(fileURLToPath(spec))
    return file === resolve(oldBundle) || file === resolve(legacy) || file.endsWith(join("node_modules", "@aeon-memory", "opencode", "dist", "aeon-memory.js"))
  } catch {
    return false
  }
}

function configuredOptions(entries: unknown[], oldBundle: string, legacy: string): Record<string, unknown> | undefined {
  for (const entry of entries) {
    if (!Array.isArray(entry) || !isKnownPluginSpec(pluginSpec(entry), oldBundle, legacy)) continue
    const options = entry[1]
    if (options && typeof options === "object" && !Array.isArray(options)) return options as Record<string, unknown>
  }
  return undefined
}

function writePluginConfig(configPath: string, oldBundle: string, legacy: string, remove: boolean): void {
  const document = readConfig(configPath)
  const entries = Array.isArray(document.value.plugin) ? document.value.plugin : []
  const retained = entries.filter((entry) => !isKnownPluginSpec(pluginSpec(entry), oldBundle, legacy))
  if (!remove) retained.push([PACKAGE_NAME, { ...DEFAULT_OPTIONS, ...configuredOptions(entries, oldBundle, legacy) }])
  const edits = modify(document.text, ["plugin"], retained.length > 0 ? retained : undefined, {
    formattingOptions: { insertSpaces: true, tabSize: 2, eol: "\n" },
  })
  mkdirSync(dirname(configPath), { recursive: true })
  writeFileSync(configPath, applyEdits(document.text, edits), { mode: 0o600 })
}

function packageJsonPath(dir: string): string {
  return join(dir, "package.json")
}

function installedPackagePath(dir: string): string {
  return join(dir, "node_modules", "@aeon-memory", "opencode")
}

function packageVersion(): string {
  const parsed = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8")) as { version?: unknown }
  if (typeof parsed.version !== "string") throw new Error(`Package version is missing: ${packageRoot}`)
  return parsed.version
}

function installPackage(dir: string, local: boolean, dryRun: boolean): void {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm"
  const spec = local ? packageRoot : `${PACKAGE_NAME}@${packageVersion()}`
  if (dryRun) {
    console.log(`Would run npm install: ${spec}`)
    return
  }
  mkdirSync(dir, { recursive: true })
  execFileSync(npm, ["install", "--save-exact", "--ignore-scripts", spec], { cwd: dir, stdio: "inherit" })
}

function localDependency(dir: string): string | undefined {
  const path = packageJsonPath(dir)
  if (!existsSync(path)) return undefined
  const pkg = JSON.parse(readFileSync(path, "utf8")) as { dependencies?: unknown }
  const dependencies = pkg.dependencies
  if (!dependencies || typeof dependencies !== "object" || Array.isArray(dependencies)) return undefined
  const value = (dependencies as Record<string, unknown>)[PACKAGE_NAME]
  return typeof value === "string" ? value : undefined
}

function uninstallPackage(dir: string, dryRun: boolean): void {
  const dependency = localDependency(dir)
  if (!dependency) return
  if (dryRun) {
    console.log(`Would run npm uninstall: ${PACKAGE_NAME}`)
    return
  }
  const npm = process.platform === "win32" ? "npm.cmd" : "npm"
  execFileSync(npm, ["uninstall", PACKAGE_NAME, "--ignore-scripts"], { cwd: dir, stdio: "inherit" })
}

function versionParts(version: string): number[] {
  return version.replace(/^v/, "").split(".").slice(0, 3).map((part) => Number.parseInt(part, 10) || 0)
}

function versionAtLeast(actual: string, minimum: string): boolean {
  const a = versionParts(actual)
  const b = versionParts(minimum)
  for (let i = 0; i < 3; i += 1) {
    if (a[i] !== b[i]) return (a[i] ?? 0) > (b[i] ?? 0)
  }
  return true
}

function detectOpenCode(): Detection {
  const binary = "opencode"
  try {
    const output = execFileSync(binary, ["--version"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim()
    const version = output.match(/\d+\.\d+\.\d+/)?.[0]
    return version ? { binary, version, compatible: versionAtLeast(version, MIN_OPENCODE) } : { binary }
  } catch {
    return { binary }
  }
}

function recognized(path: string): boolean {
  return existsSync(path) && readFileSync(path, "utf8").includes(MARKER)
}

function removeLegacyBundle(path: string, pruneParent = false): void {
  if (existsSync(path)) rmSync(path)
  if (!pruneParent) return
  try {
    rmdirSync(dirname(path))
  } catch {
    // Preserve a non-empty directory owned by the user.
  }
}

function compatibilityText(detected: Detection): string {
  if (!detected.version) return `OpenCode not detected via ${detected.binary}; requires >= ${MIN_OPENCODE}`
  return `OpenCode ${detected.version}: ${detected.compatible ? "compatible" : `requires >= ${MIN_OPENCODE}`}`
}

function buildNeeded(): boolean {
  if (!existsSync(sourcePlugin)) return false
  if (!existsSync(builtPlugin)) return true
  const builtAt = statSync(builtPlugin).mtimeMs
  return [sourcePlugin, sourceCli, join(packageRoot, "package.json"), join(packageRoot, "tsconfig.json")]
    .some((path) => existsSync(path) && statSync(path).mtimeMs > builtAt)
}

function ensureBuilt(dryRun: boolean): string {
  if (buildNeeded()) {
    if (dryRun) {
      console.log(`Would build TypeScript source: ${sourcePlugin}`)
      return builtPlugin
    }
    const npm = process.platform === "win32" ? "npm.cmd" : "npm"
    execFileSync(npm, ["run", "build"], { cwd: packageRoot, stdio: "inherit" })
  }
  if (!existsSync(builtPlugin)) throw new Error(`Built plugin not found: ${builtPlugin}`)
  return builtPlugin
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function main(): void {
  let options: Options
  try {
    options = parse(process.argv.slice(2))
  } catch (error) {
    console.error(errorMessage(error))
    usage()
    process.exitCode = 2
    return
  }
  if (!options.command || options.command === "help" || options.command === "--help") {
    usage()
    return
  }

  if (options.command === "config") {
    console.log(JSON.stringify([PACKAGE_NAME, DEFAULT_OPTIONS], null, 2))
    return
  }

  const dir = options.config ? dirname(resolve(options.config)) : configDir(options.target)
  const oldBundle = join(dir, "aeon-memory", "aeon-memory.js")
  const legacy = join(dir, "plugins", "aeon-memory.js")
  const selectedConfigPath = configPath(dir, options.config)
  const installedPackage = installedPackagePath(dir)
  const detected = detectOpenCode()

  if (options.command === "status") {
    console.log(compatibilityText(detected))
    const config = readConfig(selectedConfigPath)
    const entries = Array.isArray(config.value.plugin) ? config.value.plugin : []
    const configured = entries.some((entry) => isKnownPluginSpec(pluginSpec(entry), oldBundle, legacy))
    console.log(`${existsSync(installedPackage) && configured ? "Installed and configured" : "Not fully installed"}: ${PACKAGE_NAME}`)
    console.log(`Package: ${installedPackage}`)
    console.log(`Config: ${selectedConfigPath}`)
    return
  }

  if (options.command === "install") {
    if (detected.version && !detected.compatible && !options.force) {
      throw new Error(`${compatibilityText(detected)}. Use --force only after validating hook compatibility.`)
    }
    console.log(compatibilityText(detected))
    ensureBuilt(options.dryRun)
    if (options.dryRun) {
      installPackage(dir, options.local, true)
      console.log(`Would configure: ${selectedConfigPath}`)
      return
    }
    installPackage(dir, options.local, false)
    writePluginConfig(selectedConfigPath, oldBundle, legacy, false)
    if (recognized(oldBundle) || !existsSync(oldBundle)) removeLegacyBundle(oldBundle, true)
    if (recognized(legacy)) removeLegacyBundle(legacy)
    console.log(`Installed ${options.local ? "local" : "registry"} npm package: ${PACKAGE_NAME}`)
    console.log(`Configured: ${selectedConfigPath}`)
    console.log("Restart OpenCode to load the configured plugin instance.")
    return
  }

  if (options.command === "uninstall") {
    for (const path of [oldBundle, legacy]) {
      if (existsSync(path) && !recognized(path)) throw new Error(`Refusing to remove an unrecognized file: ${path}`)
    }
    if (options.dryRun) {
      uninstallPackage(dir, true)
      console.log(`Would remove legacy bundles: ${oldBundle}, ${legacy}`)
      console.log(`Would update: ${selectedConfigPath}`)
      return
    }
    uninstallPackage(dir, false)
    removeLegacyBundle(oldBundle, true)
    removeLegacyBundle(legacy)
    writePluginConfig(selectedConfigPath, oldBundle, legacy, true)
    console.log(`Removed npm package: ${PACKAGE_NAME}`)
    console.log(`Updated: ${selectedConfigPath}`)
    return
  }

  throw new Error(`Unknown command: ${options.command}`)
}

try {
  main()
} catch (error) {
  console.error(errorMessage(error))
  process.exitCode = 1
}
