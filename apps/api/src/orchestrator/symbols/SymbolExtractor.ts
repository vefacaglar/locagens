import type { CodeSymbol, SymbolKind } from "./types.js";

export class SymbolExtractor {
  /**
   * Extracts code symbols (functions, classes, interfaces, types) from file content.
   */
  public static extract(filePath: string, content: string): CodeSymbol[] {
    const symbols: CodeSymbol[] = [];
    const ext = filePath.split(".").pop()?.toLowerCase() || "";
    const lines = content.split("\n");

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith("//") || trimmed.startsWith("#") || trimmed.startsWith("/*") || trimmed.startsWith("*")) {
        continue;
      }

      const lineNum = i + 1;

      // --- TypeScript / JavaScript / Vue ---
      if (["ts", "js", "tsx", "jsx", "vue", "mjs", "cjs"].includes(ext)) {
        // Function declaration: function foo(...) or async function foo(...)
        const funcMatch = trimmed.match(/^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([a-zA-Z0-9_$]+)/);
        if (funcMatch) {
          symbols.push({
            name: funcMatch[1],
            kind: "function",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        // Arrow function / function expression: const foo = (...) => or const foo = async (...) =>
        const arrowMatch = trimmed.match(/^(?:export\s+)?(?:const|let|var)\s+([a-zA-Z0-9_$]+)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[a-zA-Z0-9_$]+)\s*=>/);
        if (arrowMatch) {
          symbols.push({
            name: arrowMatch[1],
            kind: "function",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        // Class declaration: class Foo or export class Foo
        const classMatch = trimmed.match(/^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([a-zA-Z0-9_$]+)/);
        if (classMatch) {
          symbols.push({
            name: classMatch[1],
            kind: "class",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        // Interface declaration: interface Foo or export interface Foo
        const interfaceMatch = trimmed.match(/^(?:export\s+)?interface\s+([a-zA-Z0-9_$]+)/);
        if (interfaceMatch) {
          symbols.push({
            name: interfaceMatch[1],
            kind: "interface",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        // Type alias: type Foo = or export type Foo =
        const typeMatch = trimmed.match(/^(?:export\s+)?type\s+([a-zA-Z0-9_$]+)\s*=/);
        if (typeMatch) {
          symbols.push({
            name: typeMatch[1],
            kind: "type",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }
      }

      // --- Python ---
      if (ext === "py") {
        // def foo(...) or async def foo(...)
        const pyFuncMatch = trimmed.match(/^(?:async\s+)?def\s+([a-zA-Z0-9_]+)\s*\(/);
        if (pyFuncMatch) {
          symbols.push({
            name: pyFuncMatch[1],
            kind: "function",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        // class Foo(...) or class Foo:
        const pyClassMatch = trimmed.match(/^class\s+([a-zA-Z0-9_]+)/);
        if (pyClassMatch) {
          symbols.push({
            name: pyClassMatch[1],
            kind: "class",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }
      }

      // --- Go ---
      if (ext === "go") {
        const goFuncMatch = trimmed.match(/^func\s+(?:\([^)]+\)\s+)?([a-zA-Z0-9_]+)\s*\(/);
        if (goFuncMatch) {
          symbols.push({
            name: goFuncMatch[1],
            kind: "function",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        const goTypeMatch = trimmed.match(/^type\s+([a-zA-Z0-9_]+)\s+(struct|interface)/);
        if (goTypeMatch) {
          symbols.push({
            name: goTypeMatch[1],
            kind: goTypeMatch[2] === "interface" ? "interface" : "class",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }
      }

      // --- Rust ---
      if (ext === "rs") {
        const rsFuncMatch = trimmed.match(/^(?:pub\s+)?(?:async\s+)?fn\s+([a-zA-Z0-9_]+)/);
        if (rsFuncMatch) {
          symbols.push({
            name: rsFuncMatch[1],
            kind: "function",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }

        const rsStructMatch = trimmed.match(/^(?:pub\s+)?(?:struct|enum|trait)\s+([a-zA-Z0-9_]+)/);
        if (rsStructMatch) {
          symbols.push({
            name: rsStructMatch[1],
            kind: "class",
            filePath,
            line: lineNum,
            preview: trimmed.slice(0, 100)
          });
          continue;
        }
      }
    }

    return symbols;
  }
}
