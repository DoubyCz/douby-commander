//! Documents in the quick view: the pages under each other, scrolled by
//! the mouse, with the page shown and buttons to the first, previous,
//! next and last page. The programs of the system draw them
//! (`dc_core::preview::pages`), a page only once it comes into view; pages
//! far from the view are forgotten, so a long document keeps memory low.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dc_core::preview::pages::{PageRenderer, PageSize};
use dc_i18n::{fill, gettext};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::launch;
use crate::pixels::Pixels;
use crate::quick_details;

/// Space around and between the pages, in pixels.
const GAP: i32 = 12;
/// Pages are drawn this many pixels wide at most, a multiple of
/// [`WIDTH_STEP`]: small changes of the view's width draw nothing anew.
const MAX_WIDTH: i32 = 2400;
const WIDTH_STEP: i32 = 200;
/// Pages this far from the view are forgotten.
const KEEP_AROUND: usize = 4;
const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
/// A page larger than this as PNG is not shown.
const MAX_PAGE_BYTES: usize = 64 << 20;
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INFO_BYTES: usize = 4 << 20;

/// The sizes of the pages of `path` from the program of `renderer`; in a
/// worker thread. Empty when it cannot tell (not installed, broken,
/// encrypted): the quick view shows the file as other files then.
pub fn page_sizes(path: &Path, renderer: PageRenderer) -> Vec<PageSize> {
    let Some(program) = launch::find_program(renderer.info_program()) else { return Vec::new() };
    let output =
        quick_details::run(&program, &renderer.info_args(path.as_os_str()), None, INFO_TIMEOUT, MAX_INFO_BYTES, &AtomicBool::new(false));
    output.map(|output| renderer.parse_pages(&String::from_utf8_lossy(&output))).unwrap_or_default()
}

struct Document {
    path: PathBuf,
    renderer: PageRenderer,
    sizes: Vec<PageSize>,
    slots: Vec<gtk::Picture>,
    /// The width each page is drawn in, `None` when it is not.
    drawn: Vec<Option<i32>>,
}

#[derive(Default)]
struct State {
    document: Option<Document>,
    /// The page being drawn now; one at a time.
    drawing: Option<usize>,
    /// Stops the drawing of the document shown.
    cancel: Arc<AtomicBool>,
    layout_pending: bool,
}

pub struct DocumentView {
    root: gtk::Box,
    scrolled: gtk::ScrolledWindow,
    pages: gtk::Box,
    position: gtk::Label,
    state: RefCell<State>,
    this: Weak<Self>,
}

impl DocumentView {
    pub fn new() -> Rc<Self> {
        let pages = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(GAP)
            .margin_top(GAP)
            .margin_bottom(GAP)
            .margin_start(GAP)
            .margin_end(GAP)
            .build();
        let scrolled =
            gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&pages).build();
        let position = gtk::Label::builder().width_chars(12).build();
        let button = |icon: &str, tooltip: String| {
            let button = gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .focusable(false)
                .focus_on_click(false)
                .build();
            button.add_css_class("flat");
            button
        };
        let first = button("go-first-symbolic", gettext("First page"));
        let previous = button("go-previous-symbolic", gettext("Previous page"));
        let next = button("go-next-symbolic", gettext("Next page"));
        let last = button("go-last-symbolic", gettext("Last page"));
        let bar = gtk::Box::builder().halign(gtk::Align::Center).build();
        bar.append(&first);
        bar.append(&previous);
        bar.append(&position);
        bar.append(&next);
        bar.append(&last);
        let bar_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bar_row.add_css_class("dc-bar");
        bar.set_hexpand(true);
        bar_row.append(&bar);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&bar_row);
        root.append(&scrolled);

        let view = Rc::new_cyclic(|this| Self {
            root,
            scrolled,
            pages,
            position,
            state: RefCell::default(),
            this: this.clone(),
        });
        let go = |button: &gtk::Button, target: fn(usize, usize) -> usize| {
            let weak = Rc::downgrade(&view);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    let count = view.count();
                    if count > 0 {
                        view.scroll_to(target(view.current_page(), count).min(count - 1));
                    }
                }
            });
        };
        go(&first, |_, _| 0);
        go(&previous, |page, _| page.saturating_sub(1));
        go(&next, |page, _| page + 1);
        go(&last, |_, count| count - 1);
        let weak = Rc::downgrade(&view);
        view.scrolled.vadjustment().connect_value_changed(move |_| {
            if let Some(view) = weak.upgrade() {
                view.update();
            }
        });
        // The width is known only after the layout: the pages get their
        // heights in an idle, a change during the layout would show later.
        let weak = Rc::downgrade(&view);
        view.scrolled.hadjustment().connect_changed(move |_| {
            if let Some(view) = weak.upgrade() {
                view.layout_later();
            }
        });
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the pages of `path`, of `sizes`, from the first.
    pub fn show(&self, path: &Path, renderer: PageRenderer, sizes: Vec<PageSize>) {
        self.clear();
        let slots: Vec<gtk::Picture> = sizes
            .iter()
            .map(|_| {
                let slot = gtk::Picture::builder().content_fit(gtk::ContentFit::Fill).can_shrink(true).build();
                slot.add_css_class("dc-page");
                self.pages.append(&slot);
                slot
            })
            .collect();
        let drawn = vec![None; slots.len()];
        self.state.borrow_mut().document = Some(Document { path: path.to_owned(), renderer, sizes, slots, drawn });
        self.scrolled.vadjustment().set_value(0.0);
        self.layout_later();
    }

    /// Forgets the document, stops its drawing.
    pub fn clear(&self) {
        let mut state = self.state.borrow_mut();
        state.cancel.store(true, Ordering::Relaxed);
        state.cancel = Arc::default();
        state.drawing = None;
        if state.document.take().is_some() {
            while let Some(child) = self.pages.first_child() {
                self.pages.remove(&child);
            }
        }
        self.position.set_text("");
    }

    fn count(&self) -> usize {
        self.state.borrow().document.as_ref().map_or(0, |document| document.slots.len())
    }

    fn layout_later(&self) {
        if std::mem::replace(&mut self.state.borrow_mut().layout_pending, true) {
            return;
        }
        let weak = self.this.clone();
        glib::idle_add_local_once(move || {
            if let Some(view) = weak.upgrade() {
                view.state.borrow_mut().layout_pending = false;
                view.layout();
            }
        });
    }

    /// Every page as wide as the view, as high as its size says.
    fn layout(&self) {
        let width = self.slot_width();
        if width <= 0 {
            return;
        }
        if let Some(document) = &self.state.borrow().document {
            for (slot, size) in document.slots.iter().zip(&document.sizes) {
                let height = if size.width > 0.0 { f64::from(width) * size.height / size.width } else { f64::from(width) };
                #[expect(clippy::cast_possible_truncation, reason = "pixels of a page, far within i32")]
                slot.set_size_request(width, height.round() as i32);
            }
        }
        self.update();
    }

    fn slot_width(&self) -> i32 {
        self.scrolled.width() - 2 * GAP
    }

    /// The pages in view, and the one at a third of the view's height,
    /// which is the page shown.
    fn in_view(&self) -> (Vec<usize>, usize) {
        let adjustment = self.scrolled.vadjustment();
        let (top, bottom) = (adjustment.value(), adjustment.value() + adjustment.page_size());
        let mark = top + adjustment.page_size() / 3.0;
        let state = self.state.borrow();
        let Some(document) = &state.document else { return (Vec::new(), 0) };
        let mut visible = Vec::new();
        let mut current = 0;
        for (index, slot) in document.slots.iter().enumerate() {
            let Some(bounds) = slot.compute_bounds(&self.pages) else { continue };
            let (start, end) = (f64::from(bounds.y()), f64::from(bounds.y() + bounds.height()));
            if end >= top && start <= bottom {
                visible.push(index);
            }
            if start <= mark {
                current = index;
            }
        }
        (visible, current)
    }

    fn current_page(&self) -> usize {
        self.in_view().1
    }

    fn scroll_to(&self, page: usize) {
        let slot = self.state.borrow().document.as_ref().and_then(|document| document.slots.get(page).cloned());
        if let Some(bounds) = slot.and_then(|slot| slot.compute_bounds(&self.pages)) {
            self.scrolled.vadjustment().set_value(f64::from(bounds.y()));
        }
    }

    /// The page shown in the bar; the next page in view that is not
    /// drawn in the current width is drawn, those far away forgotten.
    fn update(&self) {
        let (visible, current) = self.in_view();
        let count = self.count();
        if count > 0 {
            self.position.set_text(&fill(&gettext("{page} / {count}"), &[("page", &(current + 1)), ("count", &count)]));
        }
        let width = (self.slot_width() * self.scrolled.scale_factor()).clamp(WIDTH_STEP, MAX_WIDTH);
        let width = (width + WIDTH_STEP - 1) / WIDTH_STEP * WIDTH_STEP;
        let mut state = self.state.borrow_mut();
        let State { document: Some(document), drawing, cancel, .. } = &mut *state else { return };
        if let (Some(&first), Some(&last)) = (visible.first(), visible.last()) {
            for (index, slot) in document.slots.iter().enumerate() {
                if (index + KEEP_AROUND < first || index > last + KEEP_AROUND) && document.drawn[index].is_some() {
                    slot.set_paintable(None::<&gtk::gdk::Paintable>);
                    document.drawn[index] = None;
                }
            }
        }
        if drawing.is_some() {
            return;
        }
        // The page after the view too, scrolling down shows it drawn.
        let wanted = visible.iter().copied().chain(visible.last().map(|last| last + 1)).filter(|&page| page < count);
        let Some(page) = wanted.into_iter().find(|&page| document.drawn[page] != Some(width)) else { return };
        *drawing = Some(page);
        let (path, renderer, cancel) = (document.path.clone(), document.renderer, Arc::clone(cancel));
        let weak = self.this.clone();
        glib::spawn_future_local(async move {
            let drawing = Arc::clone(&cancel);
            let pixels = gio::spawn_blocking(move || draw(&path, renderer, page, width, &drawing)).await.ok().flatten();
            let Some(view) = weak.upgrade() else { return };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            {
                let mut state = view.state.borrow_mut();
                state.drawing = None;
                if let Some(document) = &mut state.document {
                    if let (Some(slot), Some(pixels)) = (document.slots.get(page), &pixels) {
                        slot.set_paintable(Some(&pixels.texture()));
                    }
                    // A page that cannot be drawn is not tried again.
                    document.drawn[page] = Some(width);
                }
            }
            view.update();
        });
    }
}

/// Page `page` (from 0) of `path`, `width` pixels wide; in a worker thread.
fn draw(path: &Path, renderer: PageRenderer, page: usize, width: i32, cancel: &AtomicBool) -> Option<Pixels> {
    let program = launch::find_program(renderer.render_program())?;
    let args = renderer.render_args(path.as_os_str(), page + 1, width.unsigned_abs());
    let png = quick_details::run(&program, &args, None, RENDER_TIMEOUT, MAX_PAGE_BYTES, cancel)?;
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(png));
    Pixels::new(&Pixbuf::from_stream(&stream, None::<&gio::Cancellable>).ok()?)
}
