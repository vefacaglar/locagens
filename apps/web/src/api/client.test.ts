import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
  delete (globalThis as any).__LOCAGENS_DESKTOP__;
  vi.restoreAllMocks();
});

describe('desktop API transport', () => {
  it('brokers API calls without exposing an authorization token to the renderer', async () => {
    const apiRequest = vi.fn(async () => ({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ port: 4321 })
    }));
    (globalThis as any).__LOCAGENS_DESKTOP__ = {
      apiRequest,
      subscribeRunEvents: vi.fn(),
      unsubscribeRunEvents: vi.fn(),
      onRunEvent: vi.fn(() => () => undefined)
    };
    const { api } = await import('./client');
    await expect(api.getSettings()).resolves.toEqual({ port: 4321 });
    expect(apiRequest).toHaveBeenCalledWith({ path: '/api/settings', method: 'GET' });
    expect(JSON.stringify(apiRequest.mock.calls)).not.toContain('Bearer');
    expect((globalThis as any).__LOCAGENS_API_TOKEN__).toBeUndefined();
  });

  it('delivers desktop-brokered SSE data and unsubscribes cleanly', async () => {
    let listener: ((event: any) => void) | undefined;
    const unsubscribeRunEvents = vi.fn(async () => undefined);
    (globalThis as any).__LOCAGENS_DESKTOP__ = {
      apiRequest: vi.fn(),
      subscribeRunEvents: vi.fn(async () => undefined),
      unsubscribeRunEvents,
      onRunEvent: vi.fn((next: (event: any) => void) => {
        listener = next;
        return () => undefined;
      })
    };
    const { api } = await import('./client');
    const stream = api.openEvents('run-safe');
    const received: string[] = [];
    stream.onmessage = event => received.push(event.data);

    const subscriptionId = (globalThis as any).__LOCAGENS_DESKTOP__.subscribeRunEvents.mock.calls[0][0].subscriptionId;
    expect((globalThis as any).__LOCAGENS_DESKTOP__.subscribeRunEvents.mock.calls[0][0]).toEqual({ subscriptionId, runId: 'run-safe' });
    listener?.({ subscriptionId, type: 'message', data: '{"type":"connected"}' });
    expect(received).toEqual(['{"type":"connected"}']);
    stream.close();
    expect(unsubscribeRunEvents).toHaveBeenCalledWith(subscriptionId);
  });
});
