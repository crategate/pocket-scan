use std::time::{Duration, Instant};

use evdev::{Device, EventSummary, KeyCode};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

use crate::action::Action;
// And on Kali you'll need read access to /dev/input/event* — add your user to the input group or ship a udev rule, otherwise grab() fails and you're back to wedge-into-terminal mode.
/// How long of a gap ends a scan burst without an Enter (bad read).
const BURST_TIMEOUT: Duration = Duration::from_millis(400);

fn find_scanner() -> Option<Device> {
    // Manual override wins
    if let Ok(path) = std::env::var("POCKET_SCAN_DEVICE") {
        return Device::open(path).ok();
    }
    for (path, device) in evdev::enumerate() {
        let name = device.name().unwrap_or("").to_lowercase();
        // Match common HID wedge scanner names; tweak for yours
        if name.contains("barcode") || name.contains("scanner") || name.contains("honeywell") {
            info!("found scanner at {}: {}", path.display(), name);
            return Device::open(path).ok();
        }
    }
    None
}
fn key_to_char(code: KeyCode, shift: bool) -> Option<char> {
    let raw = code.0;
    let c = match raw {
        r if (KeyCode::KEY_A.0..=KeyCode::KEY_Z.0).contains(&r) => {
            let base = b'a' + (r - KeyCode::KEY_A.0) as u8;
            if shift {
                (base as char).to_ascii_uppercase()
            } else {
                base as char
            }
        }
        r if (KeyCode::KEY_1.0..=KeyCode::KEY_9.0).contains(&r) => {
            let d = r - KeyCode::KEY_1.0;
            if !shift {
                (b'1' + d as u8) as char
            } else {
                ['!', '@', '#', '$', '%', '^', '&', '*', '('][d as usize]
            }
        }
        r if r == KeyCode::KEY_0.0 => {
            if shift {
                ')'
            } else {
                '0'
            }
        }
        r if r == KeyCode::KEY_MINUS.0 => {
            if shift {
                '_'
            } else {
                '-'
            }
        }
        r if r == KeyCode::KEY_EQUAL.0 => {
            if shift {
                '+'
            } else {
                '='
            }
        }
        r if r == KeyCode::KEY_SLASH.0 => {
            if shift {
                '?'
            } else {
                '/'
            }
        }
        r if r == KeyCode::KEY_DOT.0 => {
            if shift {
                '>'
            } else {
                '.'
            }
        }
        r if r == KeyCode::KEY_COMMA.0 => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        r if r == KeyCode::KEY_SEMICOLON.0 => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        r if r == KeyCode::KEY_APOSTROPHE.0 => {
            if shift {
                '"'
            } else {
                '\''
            }
        }
        r if r == KeyCode::KEY_LEFTBRACE.0 => {
            if shift {
                '{'
            } else {
                '['
            }
        }
        r if r == KeyCode::KEY_RIGHTBRACE.0 => {
            if shift {
                '}'
            } else {
                ']'
            }
        }
        r if r == KeyCode::KEY_BACKSLASH.0 => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        r if r == KeyCode::KEY_GRAVE.0 => {
            if shift {
                '~'
            } else {
                '`'
            }
        }
        r if r == KeyCode::KEY_SPACE.0 => ' ',
        _ => return None,
    };
    Some(c)
}
/// Spawn the blocking scanner reader on its own thread.
/// Returns Ok(false) if no scanner was found (app keeps running).
pub fn spawn(tx: UnboundedSender<Action>) -> color_eyre::Result<bool> {
    let Some(mut device) = find_scanner() else {
        warn!("no scanner found; keyboard-wedge input will go to crossterm instead");
        return Ok(false);
    };
    device.grab()?; // exclusive: keystrokes never reach the terminal
    info!("scanner grabbed");

    std::thread::spawn(move || {
        let mut buf = String::new();
        let mut shift = false;
        let mut burst_start: Option<Instant> = None;

        loop {
            let events = match device.fetch_events() {
                Ok(evs) => evs,
                Err(e) => {
                    error!("scanner read error: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            for event in events {
                let EventSummary::Key(_, code, value) = event.destructure() else {
                    continue;
                };
                match code {
                    KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => {
                        shift = value != 0;
                        continue;
                    }
                    KeyCode::KEY_ENTER | KeyCode::KEY_KPENTER if value == 1 => {
                        if !buf.is_empty() {
                            let _ = tx.send(Action::Scan(std::mem::take(&mut buf)));
                        }
                        burst_start = None;
                        continue;
                    }
                    _ => {}
                }
                if value != 1 {
                    continue;
                } // key-down only, ignore release & repeat

                // First keystroke of a burst: throw sand immediately
                if buf.is_empty() && burst_start.is_none_or(|t| t.elapsed() > BURST_TIMEOUT) {
                    let _ = tx.send(Action::SandStart);
                }
                burst_start = Some(Instant::now());

                if let Some(c) = key_to_char(code, shift) {
                    buf.push(c);
                }
            }
        }
    });
    Ok(true)
}
