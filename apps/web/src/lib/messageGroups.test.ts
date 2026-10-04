import { describe, expect, it } from 'vitest';
import type { RunMessage } from '@locagens/shared';
import { collectAgentSummaries, groupMessages } from './messageGroups';

function message(overrides: Partial<RunMessage>): RunMessage {
  return {
    id: overrides.id ?? Math.random().toString(36).slice(2),
    runId: 'run-1',
    role: 'assistant',
    content: '',
    createdAt: '2026-10-04T10:00:00.000Z',
    ...overrides
  };
}

describe('spawn_agents sub-agent grouping', () => {
  it('renders each explore/general sub-agent in its own labelled window', () => {
    const groups = groupMessages([
      message({ id: 'main', content: 'Splitting the work.' }),
      message({ id: 'e1', agentRole: 'explorer', agentName: 'Count lines', content: '812 entries.', model: 'model-1' }),
      message({ id: 'w1', agentRole: 'worker', agentName: 'Translate part 1', content: 'Part 1 done.', model: 'model-1' }),
      message({ id: 'w2', agentRole: 'worker', agentName: 'Translate part 2', content: 'Part 2 done.', model: 'model-1' })
    ]);

    const summaries = collectAgentSummaries(groups, false);
    expect(summaries.map(s => [s.role, s.roleLabel, s.title])).toEqual([
      ['explorer', 'Explore', 'Count lines'],
      ['worker', 'Agent', 'Translate part 1'],
      ['worker', 'Agent', 'Translate part 2']
    ]);
  });
});
