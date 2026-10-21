//! The music page of the quick view: the album's cover (else the icon of
//! the type), title, artist, album and track from the file's tags, the
//! wave of what plays and GTK's player controls.

use std::path::Path;

use dc_i18n::{fill, gettext};
use gtk::prelude::*;
use gtk::gdk;

use crate::audio_stream::{AudioStream, Tags};
use crate::quick_view::play_when_prepared;
use crate::wave::Wave;

/// Size of the cover (and the icon instead of it).
const COVER_SIZE: i32 = 192;

pub struct MusicView {
    root: gtk::Box,
    labels: Labels,
    wave: Wave,
    controls: gtk::MediaControls,
}

impl MusicView {
    pub fn new() -> Self {
        let cover = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .width_request(COVER_SIZE)
            .height_request(COVER_SIZE)
            .halign(gtk::Align::Center)
            .build();
        let icon = gtk::Image::builder().pixel_size(COVER_SIZE * 2 / 3).build();
        // Names and tags can be long: shortened, whole in the tooltip.
        let label = |class: &str| {
            let label = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).build();
            label.add_css_class(class);
            label
        };
        let (title, artist, album, track) = (label("title-2"), label("title-4"), label("dim-label"), label("dim-label"));
        let wave = Wave::new();
        wave.widget().set_margin_top(12);

        let song = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .margin_start(24)
            .margin_end(24)
            .build();
        for widget in [cover.upcast_ref::<gtk::Widget>(), icon.upcast_ref(), title.upcast_ref(), artist.upcast_ref()] {
            song.append(widget);
        }
        song.append(&album);
        song.append(&track);
        song.append(wave.widget());

        let controls = gtk::MediaControls::new(None::<&gtk::MediaStream>);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&song);
        root.append(&controls);
        Self { root, labels: Labels { title, artist, album, track, cover, icon }, wave, controls }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Plays `path` as soon as it is ready. Without our GStreamer pipeline
    /// GTK's own player plays it, without tags and the wave.
    pub fn load(&self, path: &Path, icon: &str) {
        self.stop();
        self.labels.icon.set_icon_name(Some(icon));
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        self.labels.show(&Tags::default(), &name);
        if let Some(stream) = AudioStream::new(path) {
            // The stream holds the labels only until `stop` lets go of it.
            let labels = self.labels.clone();
            stream.connect_tags(move |tags| labels.show(tags, &name));
            self.wave.set_stream(Some(&stream));
            self.controls.set_media_stream(Some(&stream));
            play_when_prepared(&stream);
        } else {
            let media = gtk::MediaFile::for_filename(path);
            self.controls.set_media_stream(Some(&media));
            play_when_prepared(&media);
        }
    }

    /// Stops playing and lets go of the file.
    pub fn stop(&self) {
        if let Some(stream) = self.controls.media_stream() {
            stream.pause();
            if let Some(stream) = stream.downcast_ref::<AudioStream>() {
                stream.shut_down();
            }
        }
        self.controls.set_media_stream(None::<&gtk::MediaStream>);
        self.wave.set_stream(None);
    }
}

/// The widgets the tags go to (also from the stream's callback).
#[derive(Clone)]
struct Labels {
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
    track: gtk::Label,
    cover: gtk::Picture,
    icon: gtk::Image,
}

impl Labels {
    /// `name` of the file stands for a missing title.
    fn show(&self, tags: &Tags, name: &str) {
        set_text(&self.title, Some(tags.title.as_deref().unwrap_or(name)));
        set_text(&self.artist, tags.artist.as_deref());
        let album = match (&tags.album, tags.year) {
            (Some(album), Some(year)) => Some(fill(&gettext("{album} ({year})"), &[("album", album), ("year", &year)])),
            (Some(album), None) => Some(album.clone()),
            (None, Some(year)) => Some(year.to_string()),
            (None, None) => None,
        };
        set_text(&self.album, album.as_deref());
        let track = match (tags.track, tags.track_count) {
            (Some(number), Some(count)) => {
                Some(fill(&gettext("Track {number} of {count}"), &[("number", &number), ("count", &count)]))
            }
            (Some(number), None) => Some(fill(&gettext("Track {number}"), &[("number", &number)])),
            _ => None,
        };
        set_text(&self.track, track.as_deref());
        let texture = tags.cover.as_ref().and_then(|bytes| gdk::Texture::from_bytes(bytes).ok());
        self.cover.set_paintable(texture.as_ref());
        self.cover.set_visible(texture.is_some());
        self.icon.set_visible(texture.is_none());
    }
}

/// The text, also as the tooltip (it may be shortened); hidden when none.
fn set_text(label: &gtk::Label, text: Option<&str>) {
    let text = text.filter(|text| !text.is_empty());
    label.set_text(text.unwrap_or_default());
    label.set_tooltip_text(text);
    label.set_visible(text.is_some());
}
