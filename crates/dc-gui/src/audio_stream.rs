//! Music for the quick view: a `GtkMediaStream` over GStreamer's `playbin`,
//! so GTK's own player controls work with it, plus what GTK's player does
//! not give: the tags of the file (title, artist, album, track, cover) and
//! the spectrum of what plays, for the wave (`spectrum` of gst-plugins-good).
//!
//! The spectrum is measured a little before the sound is heard (the audio
//! output buffers it), so every measurement keeps its running time and the
//! wave takes the one for the moment being heard.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;

use gst::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// Frequency bands of the spectrum, linear from 0 to half the sample rate.
pub const BANDS: u32 = 64;
/// Levels below this many dB are silence.
const THRESHOLD_DB: i32 = -70;
/// A measurement every this many nanoseconds (30 per second).
const INTERVAL_NS: u64 = 33_333_333;
/// The controls see the position this often while playing.
const POSITION_MS: u64 = 100;

type TagsCallback = Box<dyn Fn(&Tags)>;

/// Tags of a song, as far as the file has them.
#[derive(Debug, Clone, Default)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub track_count: Option<u32>,
    pub year: Option<i32>,
    /// The picture of the album as in the file (JPEG, PNG).
    pub cover: Option<glib::Bytes>,
}

impl Tags {
    fn merge(&mut self, list: &gst::TagListRef) {
        let text = |value: Option<gst::tags::TagValue<&str>>| {
            value.map(|value| value.get().trim().to_owned()).filter(|value| !value.is_empty())
        };
        self.title = self.title.take().or_else(|| text(list.get::<gst::tags::Title>()));
        self.artist = self
            .artist
            .take()
            .or_else(|| text(list.get::<gst::tags::Artist>()))
            .or_else(|| text(list.get::<gst::tags::AlbumArtist>()));
        self.album = self.album.take().or_else(|| text(list.get::<gst::tags::Album>()));
        self.track = self.track.or_else(|| list.get::<gst::tags::TrackNumber>().map(|value| value.get()));
        self.track_count = self.track_count.or_else(|| list.get::<gst::tags::TrackCount>().map(|value| value.get()));
        self.year = self
            .year
            .or_else(|| list.get::<gst::tags::DateTime>().map(|value| value.get().year()))
            .or_else(|| list.get::<gst::tags::Date>().map(|value| i32::from(value.get().year())));
        if self.cover.is_none() {
            let image = list.get::<gst::tags::Image>().or_else(|| list.get::<gst::tags::PreviewImage>());
            self.cover = image.and_then(|sample| {
                let sample = sample.get();
                let map = sample.buffer()?.map_readable().ok()?;
                Some(glib::Bytes::from(map.as_slice()))
            });
        }
    }
}

mod imp {
    use super::{Cell, RefCell, Tags, VecDeque, glib};
    use gst::prelude::*;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct AudioStream {
        pub playbin: RefCell<Option<gst::Element>>,
        pub bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
        pub position_timer: RefCell<Option<glib::SourceId>>,
        /// Measurements not heard yet: running time and levels 0..=1.
        pub spectrum: RefCell<VecDeque<(gst::ClockTime, Vec<f32>)>>,
        /// The last one heard.
        pub heard: RefCell<Vec<f32>>,
        pub tags: RefCell<Tags>,
        pub on_tags: RefCell<Option<super::TagsCallback>>,
        pub prepared: Cell<bool>,
    }

    impl std::fmt::Debug for AudioStream {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AudioStream")
                .field("playbin", &self.playbin.borrow())
                .field("tags", &self.tags.borrow())
                .field("prepared", &self.prepared.get())
                .finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AudioStream {
        const NAME: &'static str = "DcAudioStream";
        type Type = super::AudioStream;
        type ParentType = gtk::MediaStream;
    }

    impl ObjectImpl for AudioStream {
        fn dispose(&self) {
            self.obj().shut_down();
        }
    }

    impl MediaStreamImpl for AudioStream {
        fn play(&self) -> bool {
            let obj = self.obj();
            // Played to the end: once more from the start.
            if obj.is_ended() {
                self.seek(0);
            }
            let started = self.playbin.borrow().as_ref().is_some_and(|playbin| playbin.set_state(gst::State::Playing).is_ok());
            if started {
                obj.watch_position();
            }
            started
        }

        fn pause(&self) {
            if let Some(playbin) = self.playbin.borrow().as_ref() {
                let _ = playbin.set_state(gst::State::Paused);
            }
            if let Some(timer) = self.position_timer.take() {
                timer.remove();
            }
        }

        fn seek(&self, timestamp: i64) {
            let obj = self.obj();
            let target = gst::ClockTime::from_useconds(u64::try_from(timestamp).unwrap_or(0));
            let sought = self.playbin.borrow().as_ref().is_some_and(|playbin| {
                playbin.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target).is_ok()
            });
            self.spectrum.borrow_mut().clear();
            if sought {
                obj.seek_success();
                obj.update(timestamp);
            } else {
                obj.seek_failed();
            }
        }

        fn update_audio(&self, muted: bool, volume: f64) {
            if let Some(playbin) = self.playbin.borrow().as_ref() {
                playbin.set_property("mute", muted);
                playbin.set_property("volume", volume);
            }
        }
    }
}

glib::wrapper! {
    pub struct AudioStream(ObjectSubclass<imp::AudioStream>)
        @extends gtk::MediaStream,
        @implements gtk::gdk::Paintable;
}

impl AudioStream {
    /// Prepares `path` (paused at the start, the tags come meanwhile);
    /// `None` without GStreamer or its `spectrum` element.
    pub fn new(path: &Path) -> Option<Self> {
        gst::init().ok()?;
        let uri = glib::filename_to_uri(path, None).ok()?;
        let spectrum = gst::ElementFactory::make("spectrum")
            .property("bands", BANDS)
            .property("threshold", THRESHOLD_DB)
            .property("interval", INTERVAL_NS)
            .property("post-messages", true)
            .property("message-magnitude", true)
            .build()
            .ok()?;
        // Pictures of songs (a cover as a video stream) are not shown in a window.
        let video_sink = gst::ElementFactory::make("fakesink").build().ok()?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .property("audio-filter", &spectrum)
            .property("video-sink", &video_sink)
            .build()
            .ok()?;

        let stream: Self = glib::Object::new();
        let bus = playbin.bus()?;
        let weak = stream.downgrade();
        let watch = bus
            .add_watch_local(move |_, message| {
                if let Some(stream) = weak.upgrade() {
                    stream.on_message(message);
                }
                glib::ControlFlow::Continue
            })
            .ok()?;
        let imp = stream.imp();
        imp.bus_watch.replace(Some(watch));
        playbin.set_state(gst::State::Paused).ok()?;
        imp.playbin.replace(Some(playbin));
        Some(stream)
    }

    /// `callback` runs whenever more tags are known.
    pub fn connect_tags(&self, callback: impl Fn(&Tags) + 'static) {
        callback(&self.imp().tags.borrow());
        self.imp().on_tags.replace(Some(Box::new(callback)));
    }

    /// Levels of the bands being heard now, 0 to 1; empty when silent.
    pub fn levels(&self) -> Vec<f32> {
        let imp = self.imp();
        if self.is_playing()
            && let Some(now) = imp.playbin.borrow().as_ref().and_then(ElementExtManual::current_running_time)
        {
            let mut queue = imp.spectrum.borrow_mut();
            while queue.front().is_some_and(|(time, _)| *time <= now) {
                if let Some((_, levels)) = queue.pop_front() {
                    imp.heard.replace(levels);
                }
            }
        }
        imp.heard.borrow().clone()
    }

    /// Stops and lets go of the file.
    pub fn shut_down(&self) {
        let imp = self.imp();
        if let Some(timer) = imp.position_timer.take() {
            timer.remove();
        }
        imp.bus_watch.take();
        if let Some(playbin) = imp.playbin.take() {
            let _ = playbin.set_state(gst::State::Null);
        }
    }

    fn on_message(&self, message: &gst::Message) {
        let imp = self.imp();
        match message.view() {
            gst::MessageView::AsyncDone(_) if !imp.prepared.get() => {
                imp.prepared.set(true);
                let duration = imp
                    .playbin
                    .borrow()
                    .as_ref()
                    .and_then(ElementExtManual::query_duration::<gst::ClockTime>)
                    .map_or(0, |duration| i64::try_from(duration.useconds()).unwrap_or(0));
                self.stream_prepared(true, false, true, duration);
            }
            gst::MessageView::Tag(tag) => {
                imp.tags.borrow_mut().merge(&tag.tags());
                if let Some(callback) = imp.on_tags.borrow().as_ref() {
                    callback(&imp.tags.borrow());
                }
            }
            gst::MessageView::Element(element) => {
                if let Some(structure) = element.structure()
                    && structure.name() == "spectrum"
                    && let Some(measured) = spectrum_levels(structure)
                {
                    imp.spectrum.borrow_mut().push_back(measured);
                }
            }
            gst::MessageView::Eos(_) => {
                if let Some(timer) = imp.position_timer.take() {
                    timer.remove();
                }
                imp.heard.borrow_mut().clear();
                self.stream_ended();
            }
            gst::MessageView::Error(err) => {
                if let Some(timer) = imp.position_timer.take() {
                    timer.remove();
                }
                self.set_error(err.error());
            }
            _ => {}
        }
    }

    fn watch_position(&self) {
        let weak = self.downgrade();
        let timer = glib::timeout_add_local(std::time::Duration::from_millis(POSITION_MS), move || {
            let Some(stream) = weak.upgrade() else { return glib::ControlFlow::Break };
            let position = stream.imp().playbin.borrow().as_ref().and_then(ElementExtManual::query_position::<gst::ClockTime>);
            if let Some(position) = position {
                stream.update(i64::try_from(position.useconds()).unwrap_or(0));
            }
            glib::ControlFlow::Continue
        });
        if let Some(old) = self.imp().position_timer.replace(Some(timer)) {
            old.remove();
        }
    }
}

/// Running time and levels (0..=1 from the threshold to 0 dB) of a
/// `spectrum` message, all channels together.
fn spectrum_levels(structure: &gst::StructureRef) -> Option<(gst::ClockTime, Vec<f32>)> {
    let time = structure.get::<gst::ClockTime>("running-time").ok()?;
    let magnitudes = structure.get::<gst::List>("magnitude").ok()?;
    #[expect(clippy::cast_precision_loss, reason = "a small number of decibels")]
    let floor = THRESHOLD_DB as f32;
    let levels = magnitudes
        .iter()
        .map(|value| value.get::<f32>().map_or(0.0, |db| ((db - floor) / -floor).clamp(0.0, 1.0)))
        .collect();
    Some((time, levels))
}
