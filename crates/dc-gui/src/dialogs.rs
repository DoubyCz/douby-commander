//! Small modal dialogs. Results come back through callbacks, the main loop
//! keeps running while a dialog is open.

use std::cell::Cell;
use std::rc::Rc;

use dc_i18n::{fill, gettext, pgettext};
use dc_ops::{Answer, QueueId, Question};

use crate::texts;
use gtk::prelude::*;
use gtk::{gio, glib};

/// F7: asks for the name of a new directory, prefilled with `suggestion`
/// selected as a whole, so typing replaces it. `on_ok` gets the text as
/// typed; an empty text means nothing to do and is up to the caller.
pub fn ask_new_directory(parent: Option<&gtk::Window>, suggestion: &str, on_ok: impl FnOnce(String) + 'static) {
    let dialog = gtk::Window::builder()
        .title(gettext("Create new directory"))
        .modal(true)
        .resizable(false)
        .default_width(420)
        .build();
    dialog.set_transient_for(parent);

    let label =
        gtk::Label::builder().label(gettext("_Input new directory name:")).use_underline(true).xalign(0.0).build();
    let entry = gtk::Entry::builder().text(suggestion).activates_default(true).hexpand(true).build();
    label.set_mnemonic_widget(Some(&entry));

    let cancel = gtk::Button::with_mnemonic(&cancel_label());
    let ok = gtk::Button::with_mnemonic(&ok_label());
    ok.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).margin_top(8).build();
    buttons.append(&cancel);
    buttons.append(&ok);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&label);
    content.append(&entry);
    content.append(&buttons);
    dialog.set_child(Some(&content));
    dialog.set_default_widget(Some(&ok));
    close_on_escape(&dialog);

    let on_ok = Cell::new(Some(on_ok));
    ok.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        entry,
        move |_| {
            let text = entry.text().to_string();
            dialog.close();
            if let Some(on_ok) = on_ok.take() {
                on_ok(text);
            }
        }
    ));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| dialog.close()
    ));

    dialog.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}

/// Confirmation of a file operation (`TfrmButtonForm` of the original):
/// OK runs it at once, "Add To Queue #N" puts it into queue N, the "▾" menu
/// offers queues 1 to 5 and a new queue. `on_ok` gets the chosen queue.
pub fn confirm_operation(
    parent: Option<&gtk::Window>,
    message: &str,
    last_queue: u32,
    new_queue: QueueId,
    on_ok: impl FnOnce(QueueId) + 'static,
) {
    let dialog = gtk::Window::builder().title(texts::APP_NAME).modal(true).resizable(false).build();
    dialog.set_transient_for(parent);
    let label = gtk::Label::builder().label(message).xalign(0.0).wrap(true).max_width_chars(70).build();

    let add_to_queue =
        gtk::Button::with_mnemonic(&fill(&gettext("A_dd To Queue #{number}"), &[("number", &last_queue)]));
    let queue_menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let popover = gtk::Popover::builder().child(&queue_menu).build();
    // Without a label the button shows the theme's own arrow ("▾" of the original).
    let more_queues = gtk::MenuButton::builder().popover(&popover).build();
    let cancel = gtk::Button::with_mnemonic(&cancel_label());
    let ok = gtk::Button::with_mnemonic(&ok_label());
    ok.add_css_class("suggested-action");

    let spacer = gtk::Box::builder().hexpand(true).build();
    let buttons = gtk::Box::builder().spacing(8).margin_top(12).build();
    for widget in [add_to_queue.upcast_ref::<gtk::Widget>(), more_queues.upcast_ref(), spacer.upcast_ref(), cancel.upcast_ref(), ok.upcast_ref()]
    {
        buttons.append(widget);
    }
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .width_request(400)
        .build();
    content.append(&label);
    content.append(&buttons);
    dialog.set_child(Some(&content));
    dialog.set_default_widget(Some(&ok));
    close_on_escape(&dialog);

    // Several buttons, one decision.
    let on_ok = Rc::new(Cell::new(Some(on_ok)));
    let choose = move |dialog: &gtk::Window, queue: QueueId| {
        dialog.close();
        if let Some(on_ok) = on_ok.take() {
            on_ok(queue);
        }
    };
    let choose = Rc::new(choose);

    let connect = |button: &gtk::Button, queue: QueueId| {
        let (choose, dialog) = (Rc::clone(&choose), dialog.downgrade());
        button.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                choose(&dialog, queue);
            }
        });
    };
    connect(&ok, QueueId::Free);
    connect(&add_to_queue, QueueId::Numbered(last_queue));
    let new_queue_button = gtk::Button::builder().label(gettext("New queue")).css_classes(["flat"]).build();
    connect(&new_queue_button, new_queue);
    queue_menu.append(&new_queue_button);
    for number in 1..=5 {
        let queue = QueueId::Numbered(number);
        let button = gtk::Button::builder().label(queue.to_string()).css_classes(["flat"]).build();
        connect(&button, queue);
        queue_menu.append(&button);
    }
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| dialog.close()
    ));

    dialog.present();
    ok.grab_focus();
}

/// Yes / No question, Yes is the default. Dismissing it counts as No.
pub fn yes_no(parent: Option<&gtk::Window>, message: &str, on_answer: impl FnOnce(bool) + 'static) {
    const NO: i32 = 0;
    const YES: i32 = 1;
    let (no, yes) = (pgettext("button", "_No"), pgettext("button", "_Yes"));
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .buttons([no.as_str(), yes.as_str()])
        .cancel_button(NO)
        .default_button(YES)
        .modal(true)
        .build();
    dialog.choose(parent, None::<&gio::Cancellable>, move |answer| on_answer(answer == Ok(YES)));
}

/// Buttons per row of a question (`TfrmMsg` of the original).
const BUTTONS_PER_ROW: usize = 3;
/// With this many answers or more, only the first [`SHOWN_BUTTONS`] are
/// buttons and the rest are under "Other".
const OTHER_FROM: usize = 10;
const SHOWN_BUTTONS: usize = 8;

/// A question of a running operation, laid out as the message box of the
/// original: the text, the detail below, buttons three per row. A question
/// with `input` has an entry for a text (a new name). Enter is the default
/// answer, Esc and closing the cancel answer. `cancellable` closes the
/// dialog when the question goes away without an answer (operation
/// stopped); `on_answer` is then not called.
pub fn ask(
    parent: Option<&gtk::Window>,
    question: &Question,
    cancellable: &gio::Cancellable,
    on_answer: impl FnOnce(Answer, Option<String>) + 'static,
) {
    let dialog = gtk::Window::builder().title(texts::APP_NAME).modal(true).resizable(false).build();
    dialog.set_transient_for(parent);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .width_request(420)
        .build();
    let text = gtk::Label::builder().label(&question.text).xalign(0.0).wrap(true).max_width_chars(80).selectable(true).build();
    text.set_focusable(false);
    content.append(&text);
    if !question.detail.is_empty() {
        let detail = gtk::Label::builder().label(&question.detail).xalign(0.0).wrap(true).max_width_chars(80).build();
        detail.add_css_class("dim-label");
        content.append(&detail);
    }
    let entry = question.input.as_ref().map(|initial| {
        let entry = gtk::Entry::builder().text(initial).activates_default(true).build();
        content.append(&entry);
        entry
    });

    // One answer, whichever button, key or close comes first.
    let on_answer = Rc::new(Cell::new(Some(on_answer)));
    let finish = {
        let (dialog, entry, on_answer) = (dialog.downgrade(), entry.clone(), Rc::clone(&on_answer));
        Rc::new(move |answer: Answer| {
            let text = entry.as_ref().map(|entry| entry.text().to_string());
            if let Some(on_answer) = on_answer.take() {
                on_answer(answer, text);
            }
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        })
    };

    let answers = &question.answers;
    let (shown, other) =
        if answers.len() >= OTHER_FROM { answers.split_at(SHOWN_BUTTONS) } else { (answers.as_slice(), &[][..]) };
    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(6).column_homogeneous(true).margin_top(8).build();
    let mut cells = 0;
    for &answer in shown {
        // A text question: Yes is OK.
        let label = if entry.is_some() && answer == Answer::Yes { ok_label() } else { answer.label() };
        let button = gtk::Button::with_mnemonic(&label);
        let finish = Rc::clone(&finish);
        button.connect_clicked(move |_| finish(answer));
        if answer == question.default {
            button.add_css_class("suggested-action");
            dialog.set_default_widget(Some(&button));
        }
        attach_cell(&grid, &button, cells);
        cells += 1;
    }
    if !other.is_empty() {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let popover = gtk::Popover::builder().child(&list).build();
        for &answer in other {
            let button = gtk::Button::builder().label(answer.label()).use_underline(true).css_classes(["flat"]).build();
            let finish = Rc::clone(&finish);
            button.connect_clicked(move |_| finish(answer));
            list.append(&button);
        }
        let more = gtk::MenuButton::builder().label(pgettext("button", "Ot_her")).use_underline(true).popover(&popover).build();
        attach_cell(&grid, &more, cells);
    }
    content.append(&grid);
    dialog.set_child(Some(&content));

    let cancel = question.cancel;
    let finish_on_close = Rc::clone(&finish);
    dialog.connect_close_request(move |_| {
        finish_on_close(cancel);
        glib::Propagation::Proceed
    });
    close_on_escape(&dialog);
    // The question went away (operation stopped): close without answering.
    let (weak, cancelled) = (dialog.downgrade(), Rc::clone(&on_answer));
    cancellable.connect_cancelled_local(move |_| {
        drop(cancelled.take());
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });

    dialog.present();
    match &entry {
        Some(entry) => {
            entry.grab_focus();
            entry.select_region(0, -1);
        }
        None => {
            if let Some(default) = dialog.default_widget() {
                default.grab_focus();
            }
        }
    }
}

fn attach_cell(grid: &gtk::Grid, widget: &impl IsA<gtk::Widget>, index: usize) {
    let column = i32::try_from(index % BUTTONS_PER_ROW).unwrap_or(0);
    let row = i32::try_from(index / BUTTONS_PER_ROW).unwrap_or(0);
    grid.attach(widget, column, row, 1, 1);
}

pub fn show_error(parent: Option<&gtk::Window>, message: &str, detail: &str) {
    gtk::AlertDialog::builder().message(message).detail(detail).modal(true).build().show(parent);
}

pub fn ok_label() -> String {
    pgettext("button", "_OK")
}

pub fn cancel_label() -> String {
    pgettext("button", "_Cancel")
}

fn close_on_escape(window: &gtk::Window) {
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::CallbackAction::new(|widget, _| {
            if let Some(window) = widget.downcast_ref::<gtk::Window>() {
                window.close();
            }
            glib::Propagation::Stop
        })),
    ));
    window.add_controller(shortcuts);
}
