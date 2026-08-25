import test from "node:test";
import assert from "node:assert/strict";
import { SymbolExtractor } from "./SymbolExtractor.js";
import { SymbolIndexer } from "./SymbolIndexer.js";

test("SymbolExtractor parses TypeScript functions, classes, interfaces and types", () => {
  const tsCode = `
export function add(a: number, b: number): number {
  return a + b;
}

export const multiply = (a: number, b: number) => a * b;

export class Calculator {
  compute() {}
}

export interface CalcOptions {
  precision: number;
}

export type CalcResult = number;
`;

  const symbols = SymbolExtractor.extract("src/math.ts", tsCode);
  assert.equal(symbols.length, 5);

  assert.equal(symbols[0].name, "add");
  assert.equal(symbols[0].kind, "function");

  assert.equal(symbols[1].name, "multiply");
  assert.equal(symbols[1].kind, "function");

  assert.equal(symbols[2].name, "Calculator");
  assert.equal(symbols[2].kind, "class");

  assert.equal(symbols[3].name, "CalcOptions");
  assert.equal(symbols[3].kind, "interface");

  assert.equal(symbols[4].name, "CalcResult");
  assert.equal(symbols[4].kind, "type");
});

test("SymbolExtractor parses Python functions and classes", () => {
  const pyCode = `
def calculate_tax(amount):
    return amount * 0.2

class OrderService:
    def __init__(self):
        pass
`;

  const symbols = SymbolExtractor.extract("service.py", pyCode);
  assert.equal(symbols.length, 3);
  assert.equal(symbols[0].name, "calculate_tax");
  assert.equal(symbols[0].kind, "function");
  assert.equal(symbols[1].name, "OrderService");
  assert.equal(symbols[1].kind, "class");
  assert.equal(symbols[2].name, "__init__");
  assert.equal(symbols[2].kind, "function");
});

test("SymbolIndexer indexes and searches symbols across workspace", async () => {
  const indexer = new SymbolIndexer();
  const results = await indexer.search(process.cwd(), "ProcessManager");
  assert.ok(results.length > 0);
  assert.ok(results.some((s) => s.name === "ProcessManager"));
});
