import test from "node:test";
import assert from "node:assert/strict";
import fastify from "fastify";
import { registerSymbolRoutes } from "./symbols.js";
import { SymbolIndexer } from "../orchestrator/symbols/SymbolIndexer.js";

test("Symbol routes search symbols", async () => {
  const server = fastify();
  const symbolIndexer = new SymbolIndexer();
  const mockRepo: any = {
    list: () => [{ path: process.cwd(), name: "Workspace" }],
    get: (p: string) => ({ path: p, name: "Workspace" }),
    create: () => {},
    delete: () => true
  };

  const mockCtx: any = {
    symbolIndexer,
    projectRepo: mockRepo,
    defaultProjectPath: process.cwd()
  };

  registerSymbolRoutes(server, mockCtx);

  const res = await server.inject({
    method: "GET",
    url: `/api/projects/symbols?path=${encodeURIComponent(process.cwd())}&query=SymbolExtractor`
  });

  assert.equal(res.statusCode, 200);
  const body = JSON.parse(res.body);
  assert.ok(Array.isArray(body.symbols));
  assert.ok(body.symbols.some((s: any) => s.name === "SymbolExtractor"));
});
