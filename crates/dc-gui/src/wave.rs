//! A wave that moves with the music: its height along the width follows
//! the spectrum of what is heard (bass on the left, treble on the right),
//! in the accent color of the theme. Calm, flat line when nothing plays.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::prelude::*;

use crate::audio_stream::AudioStream;

/// Points along the width the shape is computed at.
const POINTS: usize = 48;
/// Waves across the width.
const CYCLES: f64 = 3.0;
/// How fast the wave runs, radians per second.
const SPEED: f64 = 5.0;
/// Rising follows the music at once, falling takes a while.
const RISE: f32 = 0.6;
const FALL: f32 = 0.9;

struct State {
    stream: Option<AudioStream>,
    levels: [f32; POINTS],
    phase: f64,
    last_frame: Option<i64>,
}

impl Default for State {
    fn default() -> Self {
        Self { stream: None, levels: [0.0; POINTS], phase: 0.0, last_frame: None }
    }
}

pub struct Wave {
    area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
    ticking: Rc<Cell<bool>>,
}

impl Wave {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().content_height(96).hexpand(true).build();
        // libadwaita's `.accent` makes the widget's color the accent color.
        area.add_css_class("accent");
        let state = Rc::new(RefCell::new(State::default()));
        let drawn = Rc::clone(&state);
        area.set_draw_func(move |area, cr, width, height| draw(area, cr, width, height, &drawn.borrow()));
        Self { area, state, ticking: Rc::new(Cell::new(false)) }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Follows `stream`; `None` lets the wave calm down.
    pub fn set_stream(&self, stream: Option<&AudioStream>) {
        {
            let mut state = self.state.borrow_mut();
            state.stream = stream.cloned();
            state.last_frame = None;
        }
        if self.ticking.replace(true) {
            return;
        }
        let (state, ticking) = (Rc::clone(&self.state), Rc::clone(&self.ticking));
        self.area.add_tick_callback(move |area, clock| {
            let mut state = state.borrow_mut();
            let now = clock.frame_time();
            #[expect(clippy::cast_precision_loss, reason = "microseconds between two frames")]
            let elapsed = state.last_frame.map_or(0.0, |last| (now - last) as f64 / 1e6);
            state.last_frame = Some(now);
            let target = state.stream.as_ref().map(AudioStream::levels).unwrap_or_default();
            let playing = state.stream.as_ref().is_some_and(MediaStreamExt::is_playing);
            let mut moving = false;
            for (index, level) in state.levels.iter_mut().enumerate() {
                let wanted = if playing { level_at(&target, index) } else { 0.0 };
                *level = if wanted > *level { *level + (wanted - *level) * RISE } else { *level * FALL + wanted * (1.0 - FALL) };
                moving |= *level > 0.002;
            }
            state.phase = (state.phase + elapsed * SPEED) % (2.0 * PI);
            area.queue_draw();
            // Nothing to follow and calm: stop until the next stream.
            if state.stream.is_none() && !moving {
                ticking.set(false);
                return gtk::glib::ControlFlow::Break;
            }
            gtk::glib::ControlFlow::Continue
        });
    }
}

/// Level of point `index` from the spectrum's bands: logarithmic in
/// frequency, as hearing is, the weaker treble lifted a little.
fn level_at(bands: &[f32], index: usize) -> f32 {
    if bands.is_empty() {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, reason = "small counts")]
    let (x, count) = (index as f32 / (POINTS - 1) as f32, bands.len() as f32);
    let position = (count.powf(x) - 1.0).clamp(0.0, count - 1.0);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "within the bands")]
    let low = position.floor() as usize;
    let high = (low + 1).min(bands.len() - 1);
    let fraction = position - position.floor();
    let level = bands[low] * (1.0 - fraction) + bands[high] * fraction;
    (level * (1.0 + 1.5 * x)).min(1.0)
}

fn draw(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32, state: &State) {
    let color = area.color();
    let (width, height) = (f64::from(width), f64::from(height));
    let middle = height / 2.0;
    // Strokes: amplitude, phase shift, opacity, width.
    let strokes = [(1.0, 0.0, 0.95, 2.0), (0.6, 2.1, 0.45, 1.5), (-0.8, 4.2, 0.25, 1.0)];
    for (amplitude, shift, alpha, line) in strokes {
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()) * alpha,
        );
        cr.set_line_width(line);
        let steps = width.max(1.0).round();
        let mut step = 0.0;
        while step <= steps {
            let x = step / steps;
            let envelope = (PI * x).sin() * interpolate(&state.levels, x);
            let y = middle + amplitude * envelope * middle * 0.9 * (2.0 * PI * CYCLES * x - state.phase + shift).sin();
            if step == 0.0 {
                cr.move_to(x * width, y);
            } else {
                cr.line_to(x * width, y);
            }
            step += 1.0;
        }
        let _ = cr.stroke();
    }
}

/// The level between the points at `x` (0 to 1).
fn interpolate(levels: &[f32; POINTS], x: f64) -> f64 {
    #[expect(clippy::cast_precision_loss, reason = "a small count")]
    let position = x * (POINTS - 1) as f64;
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "within the points")]
    let low = (position.floor() as usize).min(POINTS - 1);
    let high = (low + 1).min(POINTS - 1);
    let fraction = position - position.floor();
    f64::from(levels[low]) * (1.0 - fraction) + f64::from(levels[high]) * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_from_bands() {
        assert!(level_at(&[], 5).abs() < f32::EPSILON);
        let bands = [0.5_f32; 64];
        assert!((level_at(&bands, 0) - 0.5).abs() < 1e-6, "bass as measured");
        assert!((level_at(&bands, POINTS - 1) - 1.0).abs() < 1e-6, "treble lifted");
    }
}
