//! Background work with progress reporting.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::{Context, Window};

use crate::app::QuadernoApp;

#[derive(Clone, Default)]
pub struct ProgressState {
    pub label: String,
    pub detail: String,
    pub done: u64,
    pub total: u64,
}

/// Shared progress of the running background task.
#[derive(Clone)]
pub struct TaskProgress(Arc<Mutex<ProgressState>>);

impl TaskProgress {
    pub fn new(label: impl Into<String>) -> Self {
        Self(Arc::new(Mutex::new(ProgressState { label: label.into(), ..Default::default() })))
    }

    pub fn snapshot(&self) -> ProgressState {
        self.0.lock().unwrap().clone()
    }

    /// Sets the item currently being processed and resets the byte counter.
    pub fn set_detail(&self, detail: impl Into<String>) {
        let mut s = self.0.lock().unwrap();
        s.detail = detail.into();
        s.done = 0;
        s.total = 0;
    }

    /// Byte-level progress callback for core transfer functions.
    pub fn callback(&self) -> quaderno_core::Progress {
        let inner = self.0.clone();
        Arc::new(move |done, total| {
            let mut s = inner.lock().unwrap();
            s.done = done;
            s.total = total;
        })
    }

    /// Fraction (0–100) of the current transfer, if known.
    pub fn percent(&self) -> Option<f32> {
        let s = self.0.lock().unwrap();
        (s.total > 0).then(|| (s.done as f32 / s.total as f32 * 100.0).min(100.0))
    }
}

impl QuadernoApp {
    /// Runs `work` on a background thread while the UI shows a busy indicator,
    /// then calls `done` on the UI thread with the result.
    pub fn run_task<R: Send + 'static>(
        &mut self,
        label: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(TaskProgress) -> anyhow::Result<R> + Send + 'static,
        done: impl FnOnce(&mut Self, anyhow::Result<R>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.busy.is_some() {
            self.notify_warning("別の処理を実行中です。完了までお待ちください", window, cx);
            return;
        }
        let progress = TaskProgress::new(label);
        self.busy = Some(progress.clone());
        cx.notify();

        let background = cx.background_executor().spawn({
            let progress = progress.clone();
            async move { work(progress) }
        });

        // Repaint periodically so progress is visible.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let running = this
                .update(cx, |this, cx| {
                    cx.notify();
                    this.busy.is_some()
                })
                .unwrap_or(false);
            if !running {
                break;
            }
        })
        .detach();

        cx.spawn_in(window, async move |this, cx| {
            let result = background.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                done(this, result, window, cx);
                cx.notify();
            });
        })
        .detach();
    }
}
