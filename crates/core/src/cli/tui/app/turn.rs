//! Redraw tick and turn lifecycle (driver callbacks).

use super::*;
use crate::cli::tui::diff::file_stats;

impl App {
    // ---- redraw tick -----------------------------------------------------

    /// Called on every redraw tick; animates only while busy. With reduced
    /// motion the status line is static and only the elapsed time moves,
    /// about every five seconds.
    pub fn tick(&mut self) {
        if self.busy {
            self.spinner_ticks += 1;
            let period = if self.reduced_motion { REDUCED_MOTION_TICKS } else { SPINNER_TICKS };
            if self.spinner_ticks >= period {
                self.spinner_ticks = 0;
                self.spinner = self.spinner.wrapping_add(1);
                self.dirty = true;
            }
        }
        if self.quit_armed_at.is_some() && !self.quit_armed() {
            self.quit_armed_at = None;
            self.dirty = true;
        }
        if self.flash.is_some() && self.flash_text().is_none() {
            self.flash = None;
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
        self.queued.push_back(Queued {
            display: display.to_string(),
            content: queued_content.to_string(),
        });
        self.dirty = true;
    }

    /// What the user typed for each queued message, oldest first.
    pub fn queued_displays(&self) -> Vec<&str> {
        self.queued.iter().map(|q| q.display.as_str()).collect()
    }

    /// The run-side text of the newest queued message.
    pub fn last_queued_content(&self) -> Option<&str> {
        self.queued.back().map(|q| q.content.as_str())
    }

    /// The newest queued message was withdrawn from the run: put it back in
    /// the composer for editing.
    pub fn unqueue_last(&mut self) {
        if let Some(queued) = self.queued.pop_back() {
            self.composer.set_text(&queued.display);
            self.flash("Editing the queued message.");
        }
    }

    /// After a clean finish: the oldest message that was queued but never
    /// folded into the run, ready to be sent as a new turn.
    pub fn take_leftover_queued(&mut self) -> Option<String> {
        let queued = self.queued.pop_front()?;
        self.dirty = true;
        Some(queued.content)
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
        self.history_search = None;
        self.run_finished = true;
        if outcome != Finish::Completed {
            self.interrupt_pending_tools();
            self.restore_queued();
        }
        let diff = self.turn_diff.take().map(|(diff, _)| diff);
        let stats = diff.as_deref().map(file_stats).unwrap_or_default();
        match outcome {
            Finish::Completed => self.push(CellKind::TurnSummary {
                headline: elapsed,
                ok: true,
                stats,
                diff,
            }),
            Finish::Cancelled | Finish::Failed => {
                if !stats.is_empty() {
                    self.push(CellKind::TurnSummary {
                        headline: format!("stopped after {elapsed}"),
                        ok: false,
                        stats,
                        diff,
                    });
                }
                if outcome == Finish::Cancelled {
                    self.push_notice("Run cancelled.");
                }
            }
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

    /// Undelivered queued messages go back into the composer instead of
    /// being thrown away.
    pub(super) fn restore_queued(&mut self) {
        if self.queued.is_empty() {
            return;
        }
        let block = self.queued.drain(..).map(|q| q.display).collect::<Vec<_>>().join("\n\n");
        self.composer.append_block(&block);
        self.flash_for("Queued message restored to the composer.", Duration::from_secs(4));
    }

    /// The event channel closed without a terminal event.
    pub fn on_run_closed(&mut self) {
        self.finish(Finish::Failed);
    }
}
