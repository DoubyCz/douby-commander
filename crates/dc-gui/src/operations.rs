//! GUI of the operations manager: progress windows, the row of operations
//! running in the background (`PanelAllProgress` of the original) and the
//! questions operations ask.
//!
//! Everything is refreshed by a timer while any operation exists, as the
//! original does (`AllProgressOnUpdateTimer`); worker threads never touch GTK.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use dc_i18n::{fill, pgettext};
use dc_ops::{Operation, OperationId, OperationInfo, OperationsManager, QueueId};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dialogs;
use crate::progress_window::{self, ProgressWindow};

/// What a progress window or a row item follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Follow {
    /// An operation of the free queue.
    Operation(OperationId),
    /// Whichever operation of a numbered queue runs.
    Queue(QueueId),
}

impl Follow {
    fn of(info: &OperationInfo) -> Self {
        match info.queue {
            QueueId::Free => Self::Operation(info.id),
            queue @ QueueId::Numbered(_) => Self::Queue(queue),
        }
    }
}

/// Dialogs that remember their last queue each on their own, as in the
/// original (the button form for deleting, the copy and move dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMemory {
    Delete = 0,
    Transfer = 1,
}

struct RowItem {
    button: gtk::Button,
    label: gtk::Label,
    bar: gtk::ProgressBar,
}

pub struct OperationsUi {
    manager: OperationsManager,
    row: gtk::Box,
    row_items: RefCell<Vec<(Follow, RowItem)>>,
    windows: RefCell<HashMap<Follow, Rc<ProgressWindow>>>,
    /// Question dialogs on screen, closed when the question goes away.
    questions: RefCell<HashMap<OperationId, gio::Cancellable>>,
    timer: RefCell<Option<glib::SourceId>>,
    /// Queue of "Add To Queue #N", the last one chosen, per kind of dialog.
    last_queue: [Cell<u32>; 2],
    on_finished: RefCell<Option<Box<dyn Fn()>>>,
    this: Weak<Self>,
}

impl OperationsUi {
    pub fn new() -> Rc<Self> {
        let row = gtk::Box::builder().spacing(4).margin_start(2).margin_end(2).visible(false).build();
        Rc::new_cyclic(|this| Self {
            manager: OperationsManager::new(),
            row,
            row_items: RefCell::default(),
            windows: RefCell::default(),
            questions: RefCell::default(),
            timer: RefCell::default(),
            last_queue: [Cell::new(1), Cell::new(1)],
            on_finished: RefCell::default(),
            this: this.clone(),
        })
    }

    /// The row of background operations, placed above the function keys.
    pub fn row(&self) -> &gtk::Box {
        &self.row
    }

    /// Called after operations end, e.g. to list the panels again.
    pub fn connect_finished(&self, callback: impl Fn() + 'static) {
        *self.on_finished.borrow_mut() = Some(Box::new(callback));
    }

    pub fn last_queue(&self, dialog: QueueMemory) -> u32 {
        self.last_queue[dialog as usize].get()
    }

    pub fn new_queue(&self) -> QueueId {
        self.manager.new_queue_id()
    }

    /// Runs `operation` in `queue`. Its progress window opens unless it has
    /// to wait in a queue (`ShowOperation` of the original).
    /// `dialog` remembers a numbered queue for its next "Add To Queue #N".
    pub fn start(&self, operation: Box<dyn Operation>, queue: QueueId, dialog: QueueMemory) {
        if let QueueId::Numbered(number) = queue {
            self.last_queue[dialog as usize].set(number);
        }
        let id = self.manager.add(operation, queue);
        let queue_length = self.manager.operations().iter().filter(|info| info.queue == queue).count();
        match queue {
            QueueId::Free => self.open_window(Follow::Operation(id)),
            QueueId::Numbered(_) if queue_length == 1 => self.open_window(Follow::Queue(queue)),
            QueueId::Numbered(_) => {}
        }
        self.ensure_timer();
        self.refresh();
    }

    fn open_window(&self, follow: Follow) {
        if let Some(window) = self.windows.borrow().get(&follow) {
            window.present();
            return;
        }
        let this = self.this.clone();
        let window = ProgressWindow::new(self.main_window().as_ref(), self.manager.clone(), move || {
            if let Some(this) = this.upgrade() {
                this.windows.borrow_mut().remove(&follow);
                // The row may have to show the operation now.
                glib::idle_add_local_once(move || this.refresh());
            }
        });
        self.windows.borrow_mut().insert(follow, Rc::clone(&window));
        window.present();
    }

    fn ensure_timer(&self) {
        if self.timer.borrow().is_some() {
            return;
        }
        let this = self.this.clone();
        let source = glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            this.refresh();
            if this.manager.is_empty() {
                this.timer.borrow_mut().take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        *self.timer.borrow_mut() = Some(source);
    }

    fn refresh(&self) {
        let infos = self.manager.operations();
        self.update_windows(&infos);
        self.update_questions(&infos);
        self.update_row(&infos);

        if !self.manager.take_finished().is_empty()
            && let Some(on_finished) = self.on_finished.borrow().as_ref()
        {
            on_finished();
        }
    }

    fn update_windows(&self, infos: &[OperationInfo]) {
        let windows: Vec<(Follow, Rc<ProgressWindow>)> =
            self.windows.borrow().iter().map(|(follow, window)| (*follow, Rc::clone(window))).collect();
        for (follow, window) in windows {
            let shown = infos.iter().find(|info| Follow::of(info) == follow);
            match shown {
                Some(info) => {
                    let queue_length = infos.iter().filter(|other| other.queue == info.queue).count();
                    window.update(info, queue_length, self.manager.is_paused(info.id));
                }
                // Finished (a queue: all of it). Closing removes it from `windows`.
                None => window.close_keeping_operation(),
            }
        }
    }

    fn update_questions(&self, infos: &[OperationInfo]) {
        let mut questions = self.questions.borrow_mut();
        // Questions that went away unanswered (operation stopped).
        questions.retain(|id, cancellable| {
            let still_asked = infos.iter().any(|info| info.id == *id && info.question.is_some());
            if !still_asked {
                cancellable.cancel();
            }
            still_asked
        });

        for info in infos {
            let Some(question) = &info.question else { continue };
            if questions.contains_key(&info.id) {
                continue;
            }
            let cancellable = gio::Cancellable::new();
            questions.insert(info.id, cancellable.clone());

            let parent = self.windows.borrow().get(&Follow::of(info)).map(|window| window.window().clone());
            let parent = parent.or_else(|| self.main_window());
            let (this, id) = (self.this.clone(), info.id);
            dialogs::ask(parent.as_ref(), question, &cancellable, move |answer, text| {
                if let Some(this) = this.upgrade() {
                    this.questions.borrow_mut().remove(&id);
                    this.manager.answer_with_text(id, answer, text);
                }
            });
        }
    }

    /// One item per background operation or queue, i.e. without its window.
    /// The row is hidden when there is none (`UpdateVisibility` of the original).
    fn update_row(&self, infos: &[OperationInfo]) {
        let windows = self.windows.borrow();
        let mut wanted: Vec<(Follow, String, Option<f64>)> = Vec::new();
        for info in infos {
            let follow = Follow::of(info);
            if windows.contains_key(&follow) || wanted.iter().any(|(other, ..)| *other == follow) {
                continue;
            }
            let text = match info.queue {
                QueueId::Free => fill(
                    &pgettext("operations row", "{number}: {operation} - {percent}"),
                    &[("number", &info.id), ("operation", &info.description), ("percent", &progress_window::percent(info))],
                ),
                queue @ QueueId::Numbered(_) => {
                    let length = infos.iter().filter(|other| other.queue == queue).count();
                    fill(
                        &pgettext("operations row", "{queue} ({count})\n{operation} - {percent}"),
                        &[
                            ("queue", &queue),
                            ("count", &length),
                            ("operation", &info.description),
                            ("percent", &progress_window::percent(info)),
                        ],
                    )
                }
            };
            wanted.push((follow, text, info.progress.fraction()));
        }
        drop(windows);

        let same_items = {
            let items = self.row_items.borrow();
            items.len() == wanted.len() && items.iter().zip(&wanted).all(|((have, _), (want, ..))| have == want)
        };
        if !same_items {
            self.rebuild_row(wanted.iter().map(|(follow, ..)| *follow));
        }
        for ((_, item), (_, text, fraction)) in self.row_items.borrow().iter().zip(&wanted) {
            item.label.set_text(text);
            match fraction {
                Some(fraction) => item.bar.set_fraction(*fraction),
                None => item.bar.pulse(),
            }
        }
        self.row.set_visible(!wanted.is_empty());
    }

    fn rebuild_row(&self, follows: impl Iterator<Item = Follow>) {
        for (_, item) in self.row_items.borrow_mut().drain(..) {
            self.row.remove(&item.button);
        }
        let mut items = self.row_items.borrow_mut();
        for follow in follows {
            let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            let bar = gtk::ProgressBar::new();
            let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
            content.append(&label);
            content.append(&bar);
            let button = gtk::Button::builder().child(&content).focusable(false).focus_on_click(false).build();
            button.add_css_class("flat");
            let this = self.this.clone();
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.open_window(follow);
                    this.refresh();
                }
            });
            self.row.append(&button);
            items.push((follow, RowItem { button, label, bar }));
        }
    }

    fn main_window(&self) -> Option<gtk::Window> {
        self.row.root().and_downcast::<gtk::Window>()
    }
}
