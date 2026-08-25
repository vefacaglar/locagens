import type { ProcessInfo, ProcessLogEntry, ProcessStatus } from "@locagens/shared";

export type { ProcessInfo, ProcessLogEntry, ProcessStatus };

export interface SpawnOptions {
  command: string;
  projectPath: string;
  env?: Record<string, string>;
}
