// keyboard and mouse -> the bytes a terminal app expects

use eframe::egui;

// map egui keys to terminal escape sequences
// ik i wont need this for eguiemo-minus but who knows maybe ill recycle this wrapper
pub fn special_key_bytes(key: &egui::Key, modifiers: &egui::Modifiers) -> Option<Vec<u8>> {
    use egui::Key;
    match key {
        Key::ArrowUp => Some(b"\x1b[A".to_vec()),
        Key::ArrowDown => Some(b"\x1b[B".to_vec()),
        Key::ArrowRight => Some(b"\x1b[C".to_vec()),
        Key::ArrowLeft => Some(b"\x1b[D".to_vec()),
        Key::Home => Some(b"\x1b[H".to_vec()),
        Key::End => Some(b"\x1b[F".to_vec()),
        Key::PageUp => Some(b"\x1b[5~".to_vec()),
        Key::PageDown => Some(b"\x1b[6~".to_vec()),
        Key::Insert => Some(b"\x1b[2~".to_vec()),
        Key::Delete => Some(b"\x1b[3~".to_vec()),
        Key::Escape => Some(b"\x1b".to_vec()),
        Key::Tab => {
            if modifiers.shift {
                Some(b"\x1b[Z".to_vec())
            } else {
                Some(b"\x09".to_vec())
            }
        }
        Key::Backspace => Some(b"\x7f".to_vec()),
        Key::Enter => Some(b"\x0d".to_vec()),
        Key::F1 => Some(b"\x1bOP".to_vec()),
        Key::F2 => Some(b"\x1bOQ".to_vec()),
        Key::F3 => Some(b"\x1bOR".to_vec()),
        Key::F4 => Some(b"\x1bOS".to_vec()),
        Key::F5 => Some(b"\x1b[15~".to_vec()),
        Key::F6 => Some(b"\x1b[17~".to_vec()),
        Key::F7 => Some(b"\x1b[18~".to_vec()),
        Key::F8 => Some(b"\x1b[19~".to_vec()),
        Key::F9 => Some(b"\x1b[20~".to_vec()),
        Key::F10 => Some(b"\x1b[21~".to_vec()),
        Key::F11 => Some(b"\x1b[23~".to_vec()),
        Key::F12 => Some(b"\x1b[24~".to_vec()),
        _ => None,
    }
}

// ctrl+letter -> control character byte
pub fn ctrl_key_byte(key: &egui::Key) -> Option<u8> {
    use egui::Key;
    match key {
        Key::A => Some(0x01),
        Key::B => Some(0x02),
        Key::C => Some(0x03),
        Key::D => Some(0x04),
        Key::E => Some(0x05),
        Key::F => Some(0x06),
        Key::G => Some(0x07),
        Key::H => Some(0x08),
        Key::I => Some(0x09),
        Key::J => Some(0x0A),
        Key::K => Some(0x0B),
        Key::L => Some(0x0C),
        Key::M => Some(0x0D),
        Key::N => Some(0x0E),
        Key::O => Some(0x0F),
        Key::P => Some(0x10),
        Key::Q => Some(0x11),
        Key::R => Some(0x12),
        Key::S => Some(0x13),
        Key::T => Some(0x14),
        Key::U => Some(0x15),
        Key::V => Some(0x16),
        Key::W => Some(0x17),
        Key::X => Some(0x18),
        Key::Y => Some(0x19),
        Key::Z => Some(0x1A),
        _ => None,
    }
}

/// what the on screen keyboards {special} keys send, same as the real ones
pub fn named_key_bytes(name: &str) -> Option<Vec<u8>> {
    use egui::Key;
    let key = match name {
        "esc" => Key::Escape,
        "tab" => Key::Tab,
        "bksp" => Key::Backspace,
        "enter" => Key::Enter,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pgup" => Key::PageUp,
        "pgdn" => Key::PageDown,
        "del" => Key::Delete,
        "ins" => Key::Insert,
        "space" => return Some(b" ".to_vec()),
        f => Key::from_name(&f.to_uppercase())?,
    };
    special_key_bytes(&key, &egui::Modifiers::NONE)
}

/// ctrl + a typed character, for the on screen keyboards ctrl key
pub fn ctrl_char_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        ' ' | '@' => Some(0x00),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Drag,
}

/// button: 0 left, 1 middle, 2 right, 64/65 wheel up/down. col/row start at 0
pub fn mouse_seq(button: u8, col: u16, row: u16, action: MouseAction, sgr: bool) -> Vec<u8> {
    let button = if action == MouseAction::Drag { button + 32 } else { button }; // motion flag
    if sgr {
        let end = if action == MouseAction::Release { 'm' } else { 'M' };
        format!("\x1b[<{};{};{}{}", button, col + 1, row + 1, end).into_bytes()
    } else {
        // release = button 3 in normal mode, legacy x10 cant say which one
        let cb = if action == MouseAction::Release { 3 } else { button } + 32;
        let cx = (col + 33).min(255) as u8;
        let cy = (row + 33).min(255) as u8;
        vec![0x1b, b'[', b'M', cb, cx, cy]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_sequences() {
        // left press at the top left corner, sgr
        assert_eq!(mouse_seq(0, 0, 0, MouseAction::Press, true), b"\x1b[<0;1;1M");
        assert_eq!(mouse_seq(0, 4, 2, MouseAction::Release, true), b"\x1b[<0;5;3m");
        // hover = no button (3) + motion (32), like xterm
        assert_eq!(mouse_seq(3, 9, 9, MouseAction::Drag, true), b"\x1b[<35;10;10M");
        // same thing in the old x10 encoding, everything + 32
        assert_eq!(mouse_seq(3, 0, 0, MouseAction::Drag, false), vec![0x1b, b'[', b'M', 67, 33, 33]);
    }
}
