/**
 * Apply the machine HTTP proxy to main-process `fetch` and to the Chromium session.
 *
 * Chromium's `mode: "system"` can pick a SOCKS entry on the same port as the HTTP proxy. That
 * breaks `wss`. This reads the OS proxy and, when one exists, sets that hop explicitly. No hop
 * means the session is left alone.
 *
 * Algorithm ApplySystemHttpProxy: before `app.whenReady()`, read the OS proxy and, when one
 * exists, set Chromium `--proxy-server`. After ready, set the same hop on the default session.
 * Prefer HTTPS, then HTTP, then SOCKS. Skip a hop whose TCP port does not accept a connection,
 * then try the next distinct host:port. The HTTP form is one `http://host:port` rule so `wss`
 * does not stay DIRECT. A machine proxy also disables HTTP/2 so Signal can send a classic
 * upgrade. No hop does not call `setProxy` and does not set either switch. A process
 * `HTTP_PROXY` is not a machine proxy and is ignored. A listening port does not prove the
 * proxy can reach the origin.
 */

import { execFile, execFileSync, spawnSync } from "node:child_process";
import { promisify } from "node:util";
import { createOpenBotLogger } from "@openbot/logging";
import { app, net, type Session, session } from "electron";

const execFileAsync = promisify(execFile);

const logger = createOpenBotLogger("system-http-proxy");

const LOOPBACK_BYPASS = "localhost,127.0.0.1,::1,<local>";
const LOOPBACK_BYPASS_SWITCH = "localhost;127.0.0.1;::1;<local>";
const PROXY_PROBE_TIMEOUT_MS = 300;

type ProxyHop = { kind: "direct" } | { kind: "http" | "socks"; host: string; port: number };

let fetchPatched = false;

/**
 * Set Chromium's process proxy switch before `app.whenReady()`.
 *
 * Guarantees: an enabled OS HTTP proxy whose port accepts TCP becomes
 * `--proxy-server=http://host:port` and disables HTTP/2, so Signal can send a classic Upgrade.
 * A closed port is skipped. SOCKS is used when earlier HTTP hops are off or closed.
 * No accepting hop leaves both switches unset. Does not read process environment proxy variables.
 */
export function applyStartupChromiumProxy(): void {
  const hop = readDarwinProxySync();
  if (!hop) {
    logger.info("No machine proxy. Chromium proxy switch stays unset.");
    return;
  }
  app.commandLine.appendSwitch("proxy-server", chromiumProxyRules(hop));
  app.commandLine.appendSwitch("proxy-bypass-list", LOOPBACK_BYPASS_SWITCH);
  // Attendant accepts a classic Upgrade. Electron has no per-session HTTP/2 switch. This applies
  // only when a machine proxy exists, so a direct launch keeps HTTP/2.
  app.commandLine.appendSwitch("disable-http2");
  logger.info("Set the Chromium proxy switch.", { kind: hop.kind, host: hop.host, port: hop.port });
}

/**
 * Route later main-process HTTP through the OS proxy.
 *
 * Guarantees: Node `fetch` uses Chromium `net.fetch` when a proxy hop exists. DIRECT is a no-op.
 * Does not guarantee the proxy answers, and does not log credentials.
 */
export async function applySystemHttpProxy(targetUrl: string): Promise<void> {
  const system = await readMachineProxy(targetUrl);
  if (!system) {
    logger.info("No proxy is configured. Account HTTP stays direct.");
    return;
  }
  await applyHop(system, targetUrl);
}

/**
 * Put the same OS hop on one window session before that window opens Signal.
 *
 * Guarantees: when the OS has a proxy, `setProxy` uses one rule for every scheme, including `wss`.
 * No proxy leaves the session unchanged. Does not prove the later socket opens.
 */
export async function ensureChromiumProxy(target: Session): Promise<void> {
  const hop = readDarwinProxySync();
  if (!hop) return;
  await target.setProxy({
    mode: "fixed_servers",
    proxyRules: chromiumProxyRules(hop),
    proxyBypassRules: LOOPBACK_BYPASS,
  });
}

/**
 * Install one hop on the default session and route Node `fetch` through it.
 * Guarantees: `setProxy` uses `fixed_servers`, not `mode: "system"`.
 * Does not guarantee the hop answers. The log names the proxy Chromium reports for https and wss.
 */
async function applyHop(hop: Exclude<ProxyHop, { kind: "direct" }>, targetUrl: string): Promise<void> {
  await session.defaultSession.setProxy({
    mode: "fixed_servers",
    proxyRules: chromiumProxyRules(hop),
    proxyBypassRules: LOOPBACK_BYPASS,
  });
  await session.defaultSession.closeAllConnections();
  patchFetchThroughChromium();
  const signalUrl = signalProbeUrl(targetUrl);
  logger.info("Applied a proxy to account HTTP.", {
    kind: hop.kind,
    host: hop.host,
    port: hop.port,
    httpsProxy: await session.defaultSession.resolveProxy(targetUrl),
    signalProxy: signalUrl === null ? "skipped" : await session.defaultSession.resolveProxy(signalUrl),
  });
}

/**
 * The proxy the machine has enabled. HTTP wins over SOCKS when both are on.
 * A missing proxy is null. This does not add a proxy that the OS did not enable.
 */
async function readMachineProxy(targetUrl: string): Promise<Exclude<ProxyHop, { kind: "direct" }> | null> {
  const fromOs = await readDarwinProxy();
  if (fromOs) return fromOs;
  const resolved = await readSystemProxyHop(targetUrl);
  return resolved && resolved.kind !== "direct" ? resolved : null;
}

/**
 * macOS system proxy from `scutil --proxy`. Other platforms return null and use `resolveProxy`.
 */
async function readDarwinProxy(): Promise<Exclude<ProxyHop, { kind: "direct" }> | null> {
  if (process.platform !== "darwin") return null;
  try {
    const { stdout } = await execFileAsync("scutil", ["--proxy"], { timeout: 2000 });
    return preferScutilHop(stdout);
  } catch (error) {
    logger.warn("The macOS proxy list could not be read.", {
      message: error instanceof Error ? error.message : String(error),
    });
    return null;
  }
}

/**
 * The same OS proxy read, for the startup switch. A read failure is no proxy.
 * Does not use the process environment.
 */
function readDarwinProxySync(): Exclude<ProxyHop, { kind: "direct" }> | null {
  if (process.platform !== "darwin") return null;
  try {
    return preferScutilHop(execFileSync("scutil", ["--proxy"], { timeout: 2000, encoding: "utf8" }));
  } catch (error) {
    logger.warn("The macOS proxy list could not be read.", {
      message: error instanceof Error ? error.message : String(error),
    });
    return null;
  }
}

/**
 * HTTPS, then HTTP, then SOCKS. A closed TCP port is skipped. An absent or all-closed list is no
 * proxy.
 */
function preferScutilHop(text: string): Exclude<ProxyHop, { kind: "direct" }> | null {
  const seen = new Set<string>();
  for (const hop of [scutilHop(text, "HTTPS"), scutilHop(text, "HTTP"), scutilHop(text, "SOCKS")]) {
    if (!hop) continue;
    const key = `${hop.host}:${hop.port}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (tcpPortAcceptsSync(hop.host, hop.port)) return hop;
    logger.info("Skipped a machine proxy that did not accept a connection.", {
      kind: hop.kind,
      host: hop.host,
      port: hop.port,
    });
  }
  return null;
}

/**
 * Whether `host:port` accepts a TCP connect.
 *
 * Guarantees: a refused, timed-out, or unreachable port is false. A listening port is true.
 * Does not prove the process is a working HTTP or SOCKS proxy.
 */
function tcpPortAcceptsSync(host: string, port: number): boolean {
  const script = `"use strict";
const net = require("net");
const socket = net.connect(${port}, ${JSON.stringify(host)}, () => process.exit(0));
socket.on("error", () => process.exit(1));
setTimeout(() => process.exit(1), ${PROXY_PROBE_TIMEOUT_MS});
`;
  const result = spawnSync(process.execPath, ["-e", script], {
    timeout: PROXY_PROBE_TIMEOUT_MS + 700,
    env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" },
    stdio: "ignore",
    windowsHide: true,
  });
  return result.status === 0;
}

/** One enabled scutil proxy block, or null when that kind is off. */
function scutilHop(text: string, kind: "HTTP" | "HTTPS" | "SOCKS"): Exclude<ProxyHop, { kind: "direct" }> | null {
  if (!new RegExp(`${kind}Enable\\s*:\\s*1`).test(text)) return null;
  const host = text.match(new RegExp(`${kind}Proxy\\s*:\\s*(\\S+)`))?.[1];
  const port = Number(text.match(new RegExp(`${kind}Port\\s*:\\s*(\\d+)`))?.[1]);
  if (!host || !Number.isInteger(port) || port < 1 || port > 65535) return null;
  return { kind: kind === "SOCKS" ? "socks" : "http", host, port };
}

/**
 * The OS proxy Chromium would use for `targetUrl`. A resolve failure is treated as no system proxy.
 */
async function readSystemProxyHop(targetUrl: string): Promise<ProxyHop | null> {
  try {
    return parseFirstProxyHop(await session.defaultSession.resolveProxy(targetUrl));
  } catch (error) {
    logger.warn("The system proxy for the account service could not be resolved.", {
      message: error instanceof Error ? error.message : String(error),
    });
    return null;
  }
}

/**
 * Take the first usable hop from a PAC result such as `PROXY 127.0.0.1:7890; DIRECT`.
 */
function parseFirstProxyHop(pac: string): ProxyHop {
  for (const part of pac.split(";")) {
    const hop = part.trim();
    if (!hop || hop.toUpperCase() === "DIRECT") continue;
    const match = /^(PROXY|HTTPS|HTTP|SOCKS5?|SOCKS4) (\S+):(\d+)$/iu.exec(hop);
    if (!match) continue;
    const scheme = match[1];
    const host = match[2];
    const portText = match[3];
    if (!scheme || !host || !portText) continue;
    const port = Number(portText);
    if (!Number.isInteger(port) || port < 1 || port > 65535) continue;
    const kind = scheme.toUpperCase().startsWith("SOCKS") ? "socks" : "http";
    return { kind, host, port };
  }
  return { kind: "direct" };
}

/**
 * Chromium `proxyRules` for one hop. Loopback bypass stays in `proxyBypassRules`.
 */
function chromiumProxyRules(hop: Exclude<ProxyHop, { kind: "direct" }>): string {
  const endpoint = `${hop.host}:${hop.port}`;
  // One proxy URL covers http, https, ws, and wss. A scheme rule can leave wss DIRECT.
  return hop.kind === "socks" ? `socks5://${endpoint}` : `http://${endpoint}`;
}

/**
 * A `wss` URL on the account host, used only to ask Chromium which proxy it would use.
 * A URL that is not http(s) returns null. The result is not a request.
 */
function signalProbeUrl(targetUrl: string): string | null {
  try {
    const url = new URL(targetUrl);
    if (url.protocol === "https:") url.protocol = "wss:";
    else if (url.protocol === "http:") url.protocol = "ws:";
    else return null;
    url.pathname = "/signal";
    url.search = "";
    url.hash = "";
    return url.toString();
  } catch {
    return null;
  }
}

/**
 * Replace Node `fetch` with Chromium `net.fetch` once. Non-HTTP URLs keep the original fetch.
 */
function patchFetchThroughChromium(): void {
  if (fetchPatched) return;
  fetchPatched = true;
  const original = globalThis.fetch;
  function proxied(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const url = requestUrl(input);
    if (!url.startsWith("http://") && !url.startsWith("https://")) return original(input, init);
    return net.fetch(netFetchInput(input), init);
  }
  globalThis.fetch = Object.assign(proxied, original);
}

/**
 * `net.fetch` accepts a string or Request. A URL object is converted to its href.
 */
function netFetchInput(input: RequestInfo | URL): string | Request {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input;
}

/**
 * Absolute URL of a fetch input. A relative string cannot go to `net.fetch`.
 */
function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input.url;
}
