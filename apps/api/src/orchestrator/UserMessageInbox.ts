/**
 * Messages the user sends while a run is still working. The main agent loop
 * drains a run's inbox before each model call, so a message is folded into the
 * conversation at the next step without interrupting the run.
 *
 * A run accepts messages only between open() and close(). close() is called
 * synchronously the moment the main loop has returned, so a message can never be
 * accepted after the loop's last drain and then silently dropped — the caller is
 * told to continue the run normally instead.
 */
export class UserMessageInbox {
  private inboxes = new Map<string, string[]>();

  open(runId: string): void {
    this.inboxes.set(runId, []);
  }

  /** Queues a message; false when the run is not accepting messages. */
  enqueue(runId: string, content: string): boolean {
    const inbox = this.inboxes.get(runId);
    if (!inbox) return false;
    inbox.push(content);
    return true;
  }

  hasPending(runId: string): boolean {
    return (this.inboxes.get(runId)?.length ?? 0) > 0;
  }

  /** Takes every queued message for the run, oldest first. */
  drain(runId: string): string[] {
    const inbox = this.inboxes.get(runId);
    if (!inbox || inbox.length === 0) return [];
    return inbox.splice(0, inbox.length);
  }

  /** Stops accepting messages and returns any that were never delivered. */
  close(runId: string): string[] {
    const leftover = this.inboxes.get(runId) ?? [];
    this.inboxes.delete(runId);
    return leftover;
  }
}
