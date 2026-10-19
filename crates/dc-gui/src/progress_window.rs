//! Progress window of one operation, or of the running operation of a
//! numbered queue (`fFileOpDlg` of the original).
//!
//! Not modal, the panels stay usable. "To panel" closes the window and the
//! operation goes on in the background, shown in the operations row.
//! Cancel and closing the window pause the operation and ask first.

use std::cell::Cell;
use std::rc::Rc;

use dc_core::format;
use dc_i18n::{fill, gettext, pgettext};
use dc_ops::{OperationId, OperationInfo, OperationsManager, QueueId};
use gtk::glib;
use gtk::prelude::*;

use crate::{dialogs, texts};

pub struct ProgressWindow {
    window: gtk::Window,
    from: gtk::Label,
    /// "To:" with the target, for copying and moving.
    to_row: gtk::Box,
    to: gtk::Label,
    /// The current file, for operations that go through contents.
    file_bar: gtk::ProgressBar,
    bar: gtk::ProgressBar,
    /// Files done / total (`lblFileCount`), next to a bar in bytes.
    file_count: gtk::Label,
    estimate: gtk::Label,
    pause: gtk::Button,
    manager: OperationsManager,
    /// Operation shown now; for a queue its running operation.
    current: Cell<Option<(OperationId, QueueId)>>,
    /// Set when closing must not stop the operation ("To panel", finished).
    keep_running: Cell<bool>,
}

impl ProgressWindow {
    /// `on_closed` runs once the window is gone, for whatever reason.
    pub fn new(parent: Option<&gtk::Window>, manager: OperationsManager, on_closed: impl Fn() + 'static) -> Rc<Self> {
        let window = gtk::Window::builder().title(texts::APP_NAME).default_width(520).resizable(false).build();
        window.set_transient_for(parent);

        let path_row = |caption: String| {
            let caption = gtk::Label::builder().label(caption).xalign(0.0).width_chars(5).build();
            let path = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
            let row = gtk::Box::builder().spacing(6).build();
            row.append(&caption);
            row.append(&path);
            (row, path)
        };
        let (from_row, from) = path_row(gettext("From:"));
        let (to_row, to) = path_row(gettext("To:"));
        to_row.set_visible(false);

        let file_bar = gtk::ProgressBar::builder().show_text(true).visible(false).build();
        let bar = gtk::ProgressBar::builder().show_text(true).build();
        let file_count = gtk::Label::builder().xalign(0.0).visible(false).build();
        let estimate = gtk::Label::builder().xalign(0.0).build();

        let pause = gtk::Button::with_mnemonic(&pause_label(false));
        let to_panel = gtk::Button::with_mnemonic(&pgettext("button", "_To panel"));
        let view_all = gtk::Button::builder()
            .label(pgettext("button", "_View all"))
            .use_underline(true)
            .sensitive(false)
            .tooltip_text(texts::not_implemented())
            .build();
        let cancel = gtk::Button::with_mnemonic(&dialogs::cancel_label());
        let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).margin_top(8).build();
        for button in [&pause, &to_panel, &view_all, &cancel] {
            buttons.append(button);
        }

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        for widget in [
            from_row.upcast_ref::<gtk::Widget>(),
            to_row.upcast_ref(),
            file_bar.upcast_ref(),
            bar.upcast_ref(),
            file_count.upcast_ref(),
            estimate.upcast_ref(),
            buttons.upcast_ref(),
        ] {
            content.append(widget);
        }
        window.set_child(Some(&content));

        let this = Rc::new(Self {
            window,
            from,
            to_row,
            to,
            file_bar,
            bar,
            file_count,
            estimate,
            pause,
            manager,
            current: Cell::new(None),
            keep_running: Cell::new(false),
        });

        let weak = Rc::downgrade(&this);
        this.pause.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade()
                && let Some((id, _)) = this.current.get()
            {
                this.manager.toggle_pause(id);
            }
        });
        let weak = Rc::downgrade(&this);
        to_panel.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.close_keeping_operation();
            }
        });
        let weak = Rc::downgrade(&this);
        cancel.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.ask_to_cancel();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if this.keep_running.get() || this.current.get().is_none() {
                on_closed();
                glib::Propagation::Proceed
            } else {
                // Closing works like Cancel; the window closes when the operation is gone.
                this.ask_to_cancel();
                glib::Propagation::Stop
            }
        });

        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// Closes without touching the operation: "To panel", or it has ended.
    pub fn close_keeping_operation(&self) {
        self.keep_running.set(true);
        self.window.close();
    }

    /// Shows `info`. `queue_length` counts all operations of its queue.
    pub fn update(&self, info: &OperationInfo, queue_length: usize, paused: bool) {
        self.current.set(Some((info.id, info.queue)));
        let progress = &info.progress;

        let current = progress.current.as_ref().map(|path| path.display().to_string()).unwrap_or_default();
        self.from.set_text(&current);
        self.from.set_tooltip_text(Some(&current));
        self.to_row.set_visible(progress.target.is_some());
        if let Some(target) = &progress.target {
            let target = target.display().to_string();
            self.to.set_text(&target);
            self.to.set_tooltip_text(Some(&target));
        }

        // Copying: a bar for the current file, the total in bytes and the
        // file count below; deleting: one bar in files.
        self.file_bar.set_visible(progress.by_bytes);
        self.file_count.set_visible(progress.by_bytes);
        if progress.by_bytes {
            if let (Some(fraction), Some(total)) = (progress.file_fraction(), progress.file_total_bytes) {
                self.file_bar.set_fraction(fraction);
                self.file_bar.set_text(Some(&bytes_text(progress.file_done_bytes, total)));
            } else {
                self.file_bar.pulse();
                self.file_bar.set_text(Some(""));
            }
            let count = progress.total_files.filter(|total| *total > 0).map(|total| {
                format!("{} / {}", format::count(progress.done_files), format::count(total))
            });
            self.file_count.set_text(&count.unwrap_or_default());
        }

        match (progress.fraction(), progress.total_files, progress.total_bytes) {
            (Some(fraction), _, Some(bytes)) if progress.by_bytes => {
                self.bar.set_fraction(fraction);
                self.bar.set_text(Some(&bytes_text(progress.done_bytes, bytes)));
            }
            (Some(fraction), Some(total), _) => {
                self.bar.set_fraction(fraction);
                self.bar.set_text(Some(&format!("{}/{}", format::count(progress.done_files), format::count(total))));
            }
            _ => {
                // Still counting what to do.
                self.bar.pulse();
                self.bar.set_text(Some(""));
            }
        }

        self.estimate.set_text(&estimate(info, paused));
        self.pause.set_label(&pause_label(paused));
        self.pause.set_use_underline(true);

        let percent = percent(info);
        let template = match (info.queue, paused) {
            (QueueId::Free, false) => pgettext("progress window title", "{percent} {operation}"),
            (QueueId::Free, true) => pgettext("progress window title", "{percent} {operation} - paused"),
            (QueueId::Numbered(_), false) => {
                pgettext("progress window title", "[{count}] {percent} {operation} - {queue}")
            }
            (QueueId::Numbered(_), true) => {
                pgettext("progress window title", "[{count}] {percent} {operation} - {queue} - paused")
            }
        };
        let title = fill(
            &template,
            &[("percent", &percent), ("operation", &info.description), ("queue", &info.queue), ("count", &queue_length)],
        );
        self.window.set_title(Some(&title));
    }

    /// Cancel of the original (`StopOperationOrQueue`): pause, ask, then
    /// stop the operation (or the whole queue) or let it go on.
    fn ask_to_cancel(self: &Rc<Self>) {
        let Some((id, queue)) = self.current.get() else { return };
        let was_paused = self.manager.is_paused(id);
        if !was_paused {
            self.manager.set_paused(id, true);
        }
        let manager = self.manager.clone();
        // rsMsgCancelOperation of the original.
        let question = gettext("Are you sure that you want to cancel this operation?");
        dialogs::yes_no(Some(&self.window), &question, move |yes| {
            if yes {
                match queue {
                    QueueId::Free => manager.stop(id),
                    QueueId::Numbered(_) => manager.stop_queue(queue),
                }
            } else if !was_paused {
                manager.set_paused(id, false);
            }
        });
    }
}

/// `45%`, or an ellipsis while counting.
pub fn percent(info: &OperationInfo) -> String {
    info.progress.fraction().map_or_else(
        || "…".to_owned(),
        |fraction| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a rounded percentage between 0 and 100"
            )]
            let percent = (fraction * 100.0).round() as u32;
            // Czech writes "45 %", hence translatable.
            fill(&gettext("{percent}%"), &[("percent", &percent)])
        },
    )
}

fn pause_label(paused: bool) -> String {
    if paused { pgettext("button", "_Start") } else { pgettext("button", "_Pause") }
}

/// `12.5 M/40.0 M` as in the bars of the original.
fn bytes_text(done: u64, total: u64) -> String {
    format!("{}/{}", format::size(done.min(total)), format::size(total))
}

/// "Speed 12/s, time remaining 00:01:05": files per second for deleting,
/// bytes per second for copying, as in the original.
fn estimate(info: &OperationInfo, paused: bool) -> String {
    let progress = &info.progress;
    let (done, total) = match (progress.by_bytes, progress.total_bytes, progress.total_files) {
        (true, Some(bytes), _) if bytes > 0 => (progress.done_bytes, bytes),
        (_, _, Some(files)) => (progress.done_files, files),
        _ => return String::new(),
    };
    let Some(elapsed) = info.elapsed else { return String::new() };
    let seconds = elapsed.as_secs_f64();
    if paused || seconds < 0.5 {
        return String::new();
    }
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "counts and rates are far below the precision limits and never negative"
    )]
    let (speed, remaining) = {
        let speed = done as f64 / seconds;
        let left = total.saturating_sub(done) as f64;
        (speed.round() as u64, (speed > 0.0).then(|| std::time::Duration::from_secs_f64(left / speed)))
    };
    // rsDlgSpeedTime / rsDlgSpeed of the original.
    let speed = if progress.by_bytes { format::size(speed) } else { format::count(speed) };
    match remaining {
        Some(remaining) => fill(
            &gettext("Speed {speed}/s, time remaining {time}"),
            &[("speed", &speed), ("time", &format::duration(remaining))],
        ),
        None => fill(&gettext("Speed {speed}/s"), &[("speed", &speed)]),
    }
}
