//! GTK key events to [`Shortcut`]s and typed characters.

use dc_core::keymap::{Key, Modifiers, Shortcut};
use gtk::gdk;

/// The shortcut a key press stands for, `None` for keys the keymap cannot
/// name (modifier keys themselves, media keys, ...).
///
/// Keypad navigation keys (Num Lock off) count as the ordinary ones.
pub fn shortcut(key: gdk::Key, state: gdk::ModifierType) -> Option<Shortcut> {
    let key = match key {
        gdk::Key::Up | gdk::Key::KP_Up => Key::Up,
        gdk::Key::Down | gdk::Key::KP_Down => Key::Down,
        gdk::Key::Left | gdk::Key::KP_Left => Key::Left,
        gdk::Key::Right | gdk::Key::KP_Right => Key::Right,
        gdk::Key::Page_Up | gdk::Key::KP_Page_Up => Key::PageUp,
        gdk::Key::Page_Down | gdk::Key::KP_Page_Down => Key::PageDown,
        gdk::Key::Home | gdk::Key::KP_Home => Key::Home,
        gdk::Key::End | gdk::Key::KP_End => Key::End,
        gdk::Key::Insert | gdk::Key::KP_Insert => Key::Insert,
        gdk::Key::Delete | gdk::Key::KP_Delete => Key::Delete,
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => Key::Enter,
        // Shift+Tab arrives as ISO_Left_Tab.
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab | gdk::Key::KP_Tab => Key::Tab,
        gdk::Key::BackSpace => Key::Backspace,
        gdk::Key::space | gdk::Key::KP_Space => Key::Space,
        gdk::Key::Escape => Key::Escape,
        gdk::Key::KP_Add => Key::NumPlus,
        gdk::Key::KP_Subtract => Key::NumMinus,
        gdk::Key::KP_Multiply => Key::NumMultiply,
        gdk::Key::KP_Divide => Key::NumDivide,
        other => function_key(other).or_else(|| {
            // With Shift held GTK already reports the shifted character
            // (`A`, but also `!` for Shift+1 on a US layout).
            other.to_unicode().filter(|ch| !ch.is_control()).map(Key::char)
        })?,
    };
    Some(Shortcut::new(modifiers(state), key))
}

/// The character a key press types, if it types one. Shift is part of the
/// character (`A`), any other modifier makes it a shortcut.
pub fn typed_char(key: gdk::Key, state: gdk::ModifierType) -> Option<char> {
    let held = modifiers(state);
    if held.ctrl || held.alt || held.super_key {
        return None;
    }
    key.to_unicode().filter(|ch| !ch.is_control())
}

/// Pressing a modifier alone (Shift for a capital letter) must not end a
/// quick search.
pub fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::ISO_Level5_Shift
    )
}

/// Only the modifiers that make shortcuts. Lock keys (Num Lock, Caps Lock)
/// are part of the GTK state too and must not change the meaning of a key.
fn modifiers(state: gdk::ModifierType) -> Modifiers {
    Modifiers {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        super_key: state.intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
    }
}

/// GTK accelerator text (`<Control>Page_Up`), used to show shortcuts in
/// menus. Keys GTK cannot name give `None`.
pub fn accelerator(shortcut: &Shortcut) -> Option<String> {
    let key_name = match shortcut.key {
        Key::Char(ch) => char_key_name(ch)?,
        Key::F(n) => format!("F{n}"),
        Key::Up => "Up".into(),
        Key::Down => "Down".into(),
        Key::Left => "Left".into(),
        Key::Right => "Right".into(),
        Key::PageUp => "Page_Up".into(),
        Key::PageDown => "Page_Down".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::Insert => "Insert".into(),
        Key::Delete => "Delete".into(),
        Key::Enter => "Return".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "BackSpace".into(),
        Key::Space => "space".into(),
        Key::Escape => "Escape".into(),
        Key::NumPlus => "KP_Add".into(),
        Key::NumMinus => "KP_Subtract".into(),
        Key::NumMultiply => "KP_Multiply".into(),
        Key::NumDivide => "KP_Divide".into(),
    };
    let m = shortcut.modifiers;
    let mut accel = String::new();
    for (held, name) in [(m.ctrl, "<Control>"), (m.super_key, "<Super>"), (m.shift, "<Shift>"), (m.alt, "<Alt>")] {
        if held {
            accel.push_str(name);
        }
    }
    accel.push_str(&key_name);
    Some(accel)
}

/// GDK key name of a character key; letters and digits are their own name.
fn char_key_name(ch: char) -> Option<String> {
    if ch.is_ascii_alphanumeric() {
        return Some(ch.to_ascii_lowercase().to_string());
    }
    let name = match ch {
        '\\' => "backslash",
        '/' => "slash",
        '.' => "period",
        ',' => "comma",
        '-' => "minus",
        '+' => "plus",
        '=' => "equal",
        '`' => "grave",
        ';' => "semicolon",
        '\'' => "apostrophe",
        '[' => "bracketleft",
        ']' => "bracketright",
        _ => return None,
    };
    Some(name.to_owned())
}

fn function_key(key: gdk::Key) -> Option<Key> {
    let name = key.name()?;
    let number = name.strip_prefix('F')?.parse::<u8>().ok()?;
    (1..=24).contains(&number).then_some(Key::F(number))
}
