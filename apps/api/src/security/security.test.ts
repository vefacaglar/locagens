import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import Fastify from "fastify";
import { registerApiAuthentication } from "./apiAuth.js";
import { registerOriginPolicy } from "./originPolicy.js";
import { isPrivateAddress, safeFetchText, selectPublicAddress, validateAgentUrl } from "./SafeHttpClient.js";
import { commandSandbox, normalizeNetworkDomains } from "./CommandSandbox.js";
import { canonicalProjectPath, requireRegisteredProject } from "./projectPaths.js";
import { resolveInsideForMutation, resolveInsideForRead } from "../orchestrator/workspace/pathGuards.js";

test("API authentication protects /api while leaving /ping public", async () => {
  const server = Fastify();
  const token = "a".repeat(32);
  registerApiAuthentication(server, token);
  server.get("/ping", async () => ({ ok: true }));
  server.get("/api/private", async () => ({ ok: true }));

  assert.equal((await server.inject({ method: "GET", url: "/ping" })).statusCode, 200);
  assert.equal((await server.inject({ method: "GET", url: "/api/private" })).statusCode, 401);
  assert.equal((await server.inject({ method: "GET", url: "/api/private", headers: { authorization: `Bearer ${token}` } })).statusCode, 200);
  await server.close();
});

test("control-plane requests reject untrusted browser origins", async () => {
  const server = Fastify();
  const token = "a".repeat(32);
  registerOriginPolicy(server);
  registerApiAuthentication(server, token);
  server.post("/api/private", async () => ({ ok: true }));

  const headers = { authorization: `Bearer ${token}` };
  assert.equal((await server.inject({ method: "POST", url: "/api/private", headers: { ...headers, origin: "https://attacker.example" } })).statusCode, 403);
  assert.equal((await server.inject({ method: "POST", url: "/api/private", headers: { ...headers, origin: "http://localhost:5173" } })).statusCode, 200);
  assert.equal((await server.inject({ method: "POST", url: "/api/private", headers })).statusCode, 200);
  await server.close();
});

test("private and reserved IP ranges are rejected", async () => {
  for (const address of ["127.0.0.1", "10.0.0.1", "169.254.169.254", "192.168.1.1", "::1", "::ffff:7f00:1", "fc00::1", "fe80::1", "fec0::1", "2001:0db8::1"]) {
    assert.equal(isPrivateAddress(address), true, address);
  }
  assert.equal(isPrivateAddress("8.8.8.8"), false);
  assert.equal(isPrivateAddress("2001:4860:4860::8888"), false);
  await assert.rejects(() => safeFetchText("http://127.0.0.1:4321/"), /private network/i);
  await assert.rejects(() => safeFetchText("http://[::1]/"), /private network/i);
  await assert.rejects(() => safeFetchText("http://user:password@example.com/"), /credentials/i);
  assert.throws(() => validateAgentUrl(new URL("http://127.0.0.1/private", "https://example.com/start").toString()), /private network/i);
  assert.throws(() => selectPublicAddress([
    { address: "93.184.216.34", family: 4 },
    { address: "127.0.0.1", family: 4 }
  ]), /resolves to a local/i);
});

test("project paths must be canonical registered directories below home/root", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-project-test-"));
  const file = path.join(root, "file.txt");
  fs.writeFileSync(file, "x");
  try {
    const canonical = canonicalProjectPath(root);
    assert.equal(canonical, fs.realpathSync.native(root));
    assert.throws(() => canonicalProjectPath(path.parse(root).root), /filesystem root/i);
    assert.throws(() => canonicalProjectPath(os.homedir()), /home directory/i);
    assert.throws(() => canonicalProjectPath(file), /must be a directory/i);
    assert.equal(requireRegisteredProject({ get: (value: string) => value === canonical ? ({ path: canonical } as any) : null, list: () => [] } as any, root), canonical);
    assert.throws(() => requireRegisteredProject({ get: () => null, list: () => [] } as any, root), /not registered/i);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("network domains are normalized and local/IP targets are refused", () => {
  assert.deepEqual(normalizeNetworkDomains(["API.Example.com", "api.example.com", "*.NPMJS.org"]), ["*.npmjs.org", "api.example.com"]);
  assert.throws(() => normalizeNetworkDomains(["localhost"]));
  assert.throws(() => normalizeNetworkDomains(["127.0.0.1"]));
  assert.throws(() => normalizeNetworkDomains(["2130706433"]));
  assert.throws(() => normalizeNetworkDomains(["example.com:99999"]));
});

test("sandbox adapter probes the current platform without assuming readiness", async () => {
  const status = await commandSandbox.status();
  assert.equal(status.platform, process.platform);
  assert.ok(["ready", "unavailable", "setup_required"].includes(status.status));
  assert.ok(Array.isArray(status.errors));
  assert.ok(Array.isArray(status.warnings));
});

test("workspace guards block symlink escapes and mutation through links", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-path-test-"));
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-path-outside-"));
  try {
    fs.writeFileSync(path.join(outside, "secret.txt"), "secret");
    fs.symlinkSync(outside, path.join(root, "escape"));
    assert.throws(() => resolveInsideForRead(root, "escape/secret.txt"), /outside/);
    assert.throws(() => resolveInsideForMutation(root, "escape/new.txt"), /symbolic link/);
    assert.equal(resolveInsideForMutation(root, "safe/new.txt"), path.join(fs.realpathSync.native(root), "safe", "new.txt"));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  }
});
