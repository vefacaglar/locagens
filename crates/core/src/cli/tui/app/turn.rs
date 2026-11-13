//! Redraw tick and turn lifecycle (driver callbacks).

use super::*;

impl App {
    // ---- redraw tick -----------------------------------------------------

    /// Called on every redraw tick; animates only while busy.
    pub fn tick(&mut self) {
        if self.busy {
            self.spinner_ticks += 1;
            if self.spinner_ticks >= SPINNER_TICKS {
                self.spinner_ticks = 0;
                self.spinner = self.spinner.wrapping_add(1);
                self.dirty = true;
            }
        }
        if self.quit_armed_at.is_some() && !self.quit_armed() {
            self.quit_armed_at = None;
            self.dirty = true;
        }
    }

    // ---- turn lifecycle (driver callbacks) ---------------------------------

    pub fn begin_turn(&mut self) {
        self.busy = true;
        self.busy_since = Some(Instant::now());
        self.turn_diff = None;
        self.viewport.follow = true;
        self.dirty = true;
    }

    /// Records a mid-run message the driver queued on the active run.
    pub fn note_queued(&mut self, display: &str, queued_content: &str) {
        self.queued_texts.push_back(queued_content.to_string());
        self.push(CellKind::User {
            text: display.to_string(),
            queued: true,
        });
    }

    /// After a clean finish: the oldest message that was queued but never
    /// folded into the run, ready to be sent as a new turn.
    pub fn take_leftover_queued(&mut self) -> Option<String> {
        let text = self.queued_texts.pop_front()?;
        // The new turn's user event re-creates the cell, so drop the
        // "queued" placeholder.
        if let Some(index) = self
            .cells
            .iter()
            .position(|c| matches!(c.kind(), CellKind::User { queued: true, .. }))
        {
            self.cells.remove(index);
        }
        Some(text)
    }

    pub(super) fn clear_first_queued_flag(&mut self) {
        if let Some(cell) = self
            .cells
            .iter_mut()
            .find(|c| matches!(c.kind(), CellKind::User { queued: true, .. }))
        {
            cell.update(|kind| {
                if let CellKind::User { queued, .. } = kind {
                    *queued = false;
                }
            });
            self.dirty = true;
        }
    }

    pub(super) fn finish(&mut self, outcome: Finish) {
        if !self.busy {
            return;
        }
        let elapsed = self.elapsed().map(format_elapsed).unwrap_or_default();
        self.busy = false;
        self.busy_since = None;
        self.patch_progress = None;
        self.overlay = None;
        self.run_finished = true;
        if outcome != Finish::Completed {
            self.interrupt_pending_tools();
            self.discard_queued();
        }
        if let Some((diff, files)) = self.turn_diff.take() {
            let noun = if files == 1 { "file" } else { "files" };
            self.push(CellKind::Diff {
                title: format!("Changes: {files} {noun}"),
                diff,
            });
        }
        match outcome {
            Finish::Completed => self.push(CellKind::Completed(format!("done · {elapsed}"))),
            Finish::Cancelled => self.push_notice("Run cancelled."),
            Finish::Failed => {}
        }
        self.dirty = true;
    }

    pub(super) fn interrupt_pending_tools(&mut self) {
        for cell in self.cells.iter_mut() {
            if matches!(cell.kind(), CellKind::Tool { result: None, .. }) {
                cell.update(|kind| {
                    if let CellKind::Tool { result, .. } = kind {
                        *result = Some(ToolResult {
                            ok: false,
                            output: "interrupted".to_string(),
                        });
                    }
                });
            }
        }
    }

    pub(super) fn discard_queued(&mut self) {
        let count = self.queued_texts.len();
        if count == 0 {
            return;
        }
        self.queued_texts.clear();
        for cell in self.cells.iter_mut() {
            cell.update(|kind| {
                if let CellKind::User { queued, .. } = kind {
                    *queued = false;
                }
            });
        }
        self.push_notice(format!("{count} queued message(s) were not delivered."));
    }

    /// The event channel closed without a terminal event.
    pub fn on_run_closed(&mut self) {
        self.finish(Finish::Failed);
    }

}
