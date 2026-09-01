//! Native X11 CLIPBOARD (ICCCM selection), not `xclip` / `xsel`.
//!
//! `SetSelectionOwner` seeds CLIPBOARD; a later `get_text` in the same
//! process returns that owned UTF-8 payload. Foreign owners are read with
//! `ConvertSelection`. No helper binary.
//!
//! A CLI process that exits after `SetSelectionOwner` leaves CLIPBOARD
//! unowned. When `PLATFORM_X11_CLIPBOARD_SERVE` is set, `set_text` stays in
//! the X11 event loop and answers `SelectionRequest` until `SelectionClear`
//! (another owner). That is how a CLI consumer keeps the native selection
//! alive for a later process.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

use super::ClipboardError;

struct Atoms {
    clipboard: Atom,
    utf8_string: Atom,
    targets: Atom,
    incr: Atom,
    property: Atom,
    string: Atom,
    atom: Atom,
}

struct OwnedSelection {
    type_name: String,
    type_atom: Atom,
    bytes: Vec<u8>,
}

struct NativeClipboard {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
    owned: Option<OwnedSelection>,
}

static STATE: Mutex<Option<NativeClipboard>> = Mutex::new(None);

fn backend(message: impl ToString) -> ClipboardError {
    ClipboardError::Backend {
        message: message.to_string(),
    }
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<Atom, ClipboardError> {
    conn.intern_atom(false, name)
        .map_err(|error| backend(format!("X11 InternAtom send failed: {error}")))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| backend(format!("X11 InternAtom failed: {error}")))
}

impl NativeClipboard {
    fn open() -> Result<Self, ClipboardError> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|error| ClipboardError::Unavailable {
                message: format!("X11 display could not be opened: {error}"),
            })?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| backend("configured X11 screen does not exist"))?
            .root;
        let window = conn
            .generate_id()
            .map_err(|error| backend(format!("X11 generate_id failed: {error}")))?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            -10,
            -10,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|error| backend(format!("X11 CreateWindow failed: {error}")))?;
        let atoms = match intern_atoms(&conn) {
            Ok(atoms) => atoms,
            Err(error) => {
                let _ = conn.destroy_window(window);
                let _ = conn.flush();
                return Err(error);
            }
        };
        conn.flush()
            .map_err(|error| backend(format!("X11 clipboard flush failed: {error}")))?;
        Ok(Self {
            conn,
            window,
            atoms,
            owned: None,
        })
    }

    fn pump(&mut self) -> Result<(), ClipboardError> {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(event)) => self.handle_event(event)?,
                Ok(None) => return Ok(()),
                Err(error) => {
                    return Err(backend(format!("X11 poll_for_event failed: {error}")));
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<(), ClipboardError> {
        match event {
            Event::SelectionRequest(request) => self.serve_request(&request),
            Event::SelectionClear(clear) if clear.selection == self.atoms.clipboard => {
                self.owned = None;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn serve_request(&self, request: &SelectionRequestEvent) -> Result<(), ClipboardError> {
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let ok =
            request.selection == self.atoms.clipboard && self.write_selection(request, property);
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if ok { property } else { NONE },
        };
        self.conn
            .send_event(false, request.requestor, EventMask::NO_EVENT, notify)
            .map_err(|error| backend(format!("X11 SelectionNotify send failed: {error}")))?;
        self.conn
            .flush()
            .map_err(|error| backend(format!("X11 SelectionNotify flush failed: {error}")))
    }

    fn write_selection(&self, request: &SelectionRequestEvent, property: Atom) -> bool {
        let Some(owned) = self.owned.as_ref() else {
            return false;
        };
        let bytes = owned.bytes.as_slice();
        if request.target == self.atoms.targets {
            let mut targets = vec![self.atoms.targets, owned.type_atom];
            if owned.type_atom == self.atoms.utf8_string {
                targets.push(self.atoms.string);
            }
            return self
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    self.atoms.atom,
                    &targets,
                )
                .and_then(|_| self.conn.flush())
                .is_ok();
        }
        if request.target == owned.type_atom
            || (owned.type_atom == self.atoms.utf8_string
                && (request.target == self.atoms.utf8_string
                    || request.target == self.atoms.string))
        {
            return self
                .conn
                .change_property8(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    request.target,
                    bytes,
                )
                .and_then(|_| self.conn.flush())
                .is_ok();
        }
        false
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.set_type("UTF8_STRING", text.as_bytes())
    }

    fn set_type(&mut self, type_name: &str, bytes: &[u8]) -> Result<(), ClipboardError> {
        self.pump()?;
        let type_atom = intern(&self.conn, type_name.as_bytes())?;
        self.owned = Some(OwnedSelection {
            type_name: type_name.to_owned(),
            type_atom,
            bytes: bytes.to_vec(),
        });
        self.conn
            .set_selection_owner(self.window, self.atoms.clipboard, CURRENT_TIME)
            .map_err(|error| backend(format!("X11 SetSelectionOwner send failed: {error}")))?;
        self.conn
            .flush()
            .map_err(|error| backend(format!("X11 SetSelectionOwner flush failed: {error}")))?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(|error| backend(format!("X11 GetSelectionOwner send failed: {error}")))?
            .reply()
            .map_err(|error| backend(format!("X11 GetSelectionOwner failed: {error}")))?
            .owner;
        if owner != self.window {
            self.owned = None;
            return Err(backend(
                "X11 SetSelectionOwner did not leave this connection as CLIPBOARD owner",
            ));
        }
        if serve_owned_selection() {
            self.serve_until_replaced()?;
        }
        Ok(())
    }

    /// Answer CLIPBOARD `SelectionRequest` until another client takes the
    /// selection. Returns when `owned` is cleared (`SelectionClear`).
    fn serve_until_replaced(&mut self) -> Result<(), ClipboardError> {
        while self.owned.is_some() {
            match self.conn.wait_for_event() {
                Ok(event) => self.handle_event(event)?,
                Err(error) => {
                    return Err(backend(format!("X11 wait_for_event failed: {error}")));
                }
            }
            self.pump()?;
        }
        Ok(())
    }

    fn get_text(
        &mut self,
        max_read_bytes: usize,
        timeout: Duration,
    ) -> Result<String, ClipboardError> {
        self.pump()?;
        if let Some(owned) = self.owned.as_ref() {
            if owned.type_atom == self.atoms.utf8_string || owned.type_name == "UTF8_STRING" {
                if owned.bytes.len() > max_read_bytes {
                    return Err(ClipboardError::TooLarge {
                        limit: max_read_bytes,
                    });
                }
                return String::from_utf8(owned.bytes.clone())
                    .map_err(|error| backend(error.to_string()));
            }
        }
        self.convert_clipboard(self.atoms.utf8_string, max_read_bytes, timeout)
    }

    fn get_type(
        &mut self,
        type_name: &str,
        max_read_bytes: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, ClipboardError> {
        self.pump()?;
        if let Some(owned) = self.owned.as_ref() {
            if owned.type_name == type_name
                || (type_name == "UTF8_STRING" && owned.type_atom == self.atoms.utf8_string)
            {
                if owned.bytes.len() > max_read_bytes {
                    return Err(ClipboardError::TooLarge {
                        limit: max_read_bytes,
                    });
                }
                return Ok(owned.bytes.clone());
            }
        }
        let target = intern(&self.conn, type_name.as_bytes())?;
        self.convert_clipboard_bytes(target, max_read_bytes, timeout)
    }

    fn convert_clipboard(
        &mut self,
        target: Atom,
        max_read_bytes: usize,
        timeout: Duration,
    ) -> Result<String, ClipboardError> {
        let bytes = self.convert_clipboard_bytes(target, max_read_bytes, timeout)?;
        if target == self.atoms.string {
            return Ok(bytes.into_iter().map(char::from).collect());
        }
        String::from_utf8(bytes).map_err(|error| backend(error.to_string()))
    }

    fn convert_clipboard_bytes(
        &mut self,
        target: Atom,
        max_read_bytes: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, ClipboardError> {
        let _ = self.conn.delete_property(self.window, self.atoms.property);
        self.conn
            .convert_selection(
                self.window,
                self.atoms.clipboard,
                target,
                self.atoms.property,
                CURRENT_TIME,
            )
            .map_err(|error| backend(format!("X11 ConvertSelection send failed: {error}")))?;
        self.conn
            .flush()
            .map_err(|error| backend(format!("X11 ConvertSelection flush failed: {error}")))?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(Event::SelectionNotify(notify)))
                    if notify.selection == self.atoms.clipboard
                        && notify.requestor == self.window
                        && notify.target == target =>
                {
                    if notify.property == NONE {
                        return Err(backend("X11 selection has no such target"));
                    }
                    return self.read_property_bytes(notify.property, max_read_bytes);
                }
                Ok(Some(event)) => self.handle_event(event)?,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        return Err(ClipboardError::Timeout {
                            message: format!(
                                "clipboard_timeout: X11 ConvertSelection exceeded {} ms",
                                timeout.as_millis()
                            ),
                        });
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => {
                    return Err(backend(format!("X11 poll_for_event failed: {error}")));
                }
            }
        }
    }

    fn read_property_bytes(
        &self,
        property: Atom,
        max_read_bytes: usize,
    ) -> Result<Vec<u8>, ClipboardError> {
        let long_length = u32::try_from(max_read_bytes / 4 + 2).unwrap_or(u32::MAX);
        let reply = self
            .conn
            .get_property(true, self.window, property, AtomEnum::ANY, 0, long_length)
            .map_err(|error| backend(format!("X11 GetProperty send failed: {error}")))?
            .reply()
            .map_err(|error| backend(format!("X11 GetProperty failed: {error}")))?;
        if reply.type_ == self.atoms.incr {
            return Err(ClipboardError::TooLarge {
                limit: max_read_bytes,
            });
        }
        if reply.bytes_after > 0 {
            return Err(ClipboardError::TooLarge {
                limit: max_read_bytes,
            });
        }
        if reply.value.len() > max_read_bytes {
            return Err(ClipboardError::TooLarge {
                limit: max_read_bytes,
            });
        }
        Ok(reply.value)
    }
}

fn intern_atoms(conn: &RustConnection) -> Result<Atoms, ClipboardError> {
    Ok(Atoms {
        clipboard: intern(conn, b"CLIPBOARD")?,
        utf8_string: intern(conn, b"UTF8_STRING")?,
        targets: intern(conn, b"TARGETS")?,
        incr: intern(conn, b"INCR")?,
        property: intern(conn, b"PLATFORM_CLIPBOARD")?,
        string: Atom::from(AtomEnum::STRING),
        atom: Atom::from(AtomEnum::ATOM),
    })
}

fn with_state<T>(
    f: impl FnOnce(&mut NativeClipboard) -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    let mut guard = STATE.lock().unwrap_or_else(|error| error.into_inner());
    if guard.is_none() {
        *guard = Some(NativeClipboard::open()?);
    }
    f(guard.as_mut().expect("native clipboard state just opened"))
}

fn serve_owned_selection() -> bool {
    match std::env::var("PLATFORM_X11_CLIPBOARD_SERVE") {
        Ok(value) => !value.is_empty() && value != "0",
        Err(_) => false,
    }
}

pub(super) fn set_text(text: &str, _timeout: Duration) -> Result<(), ClipboardError> {
    with_state(|state| state.set_text(text))
}

pub(super) fn get_text(max_read_bytes: usize, timeout: Duration) -> Result<String, ClipboardError> {
    with_state(|state| state.get_text(max_read_bytes, timeout))
}

pub(super) fn get_type(
    type_name: &str,
    max_read_bytes: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ClipboardError> {
    with_state(|state| state.get_type(type_name, max_read_bytes, timeout))
}

pub(super) fn set_type(
    type_name: &str,
    bytes: &[u8],
    _timeout: Duration,
) -> Result<(), ClipboardError> {
    with_state(|state| state.set_type(type_name, bytes))
}

pub(super) fn clear(_timeout: Duration) -> Result<(), ClipboardError> {
    with_state(|state| {
        state.pump()?;
        state.owned = None;
        state
            .conn
            .set_selection_owner(NONE, state.atoms.clipboard, CURRENT_TIME)
            .map_err(|error| {
                backend(format!("X11 clear SetSelectionOwner send failed: {error}"))
            })?;
        state
            .conn
            .flush()
            .map_err(|error| backend(format!("X11 clear flush failed: {error}")))?;
        Ok(())
    })
}

/// A 1-byte `get_text` probe of a longer payload is `TooLarge`, not absence.
fn probe_indicates_unicode_text(result: Result<String, ClipboardError>) -> bool {
    match result {
        Ok(text) => !text.is_empty(),
        Err(ClipboardError::TooLarge { .. }) => true,
        Err(_) => false,
    }
}

pub(super) fn has_unicode_text() -> bool {
    match with_state(|state| {
        state.pump()?;
        Ok(state.owned.as_ref().is_some_and(|owned| {
            !owned.bytes.is_empty()
                && (owned.type_atom == state.atoms.utf8_string || owned.type_name == "UTF8_STRING")
        }))
    }) {
        Ok(true) => true,
        Ok(false) => probe_indicates_unicode_text(get_text(1, Duration::from_millis(200))),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn property_atom_is_product_neutral() {
        assert_eq!(
            intern_atoms_names(),
            [
                "CLIPBOARD",
                "UTF8_STRING",
                "TARGETS",
                "INCR",
                "PLATFORM_CLIPBOARD"
            ]
        );
    }

    #[test]
    fn one_byte_probe_too_large_still_means_unicode_text() {
        assert!(super::probe_indicates_unicode_text(Ok("x".into())));
        assert!(!super::probe_indicates_unicode_text(Ok(String::new())));
        assert!(super::probe_indicates_unicode_text(Err(
            super::ClipboardError::TooLarge { limit: 1 }
        )));
        assert!(!super::probe_indicates_unicode_text(Err(
            super::ClipboardError::Timeout {
                message: "clipboard_timeout".into(),
            }
        )));
    }

    fn intern_atoms_names() -> [&'static str; 5] {
        [
            "CLIPBOARD",
            "UTF8_STRING",
            "TARGETS",
            "INCR",
            "PLATFORM_CLIPBOARD",
        ]
    }
}
