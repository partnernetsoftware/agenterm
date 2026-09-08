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

const TARGETS_PROBE_ATOMS: usize = crate::contract::clipboard::MAX_CLIPBOARD_TYPES;
/// Canonical MCU / CU UTF-8 plain-text clipboard type.
const PLAIN_UTF8_TYPE: &str = "text/plain;charset=utf-8";

struct Atoms {
    clipboard: Atom,
    utf8_string: Atom,
    plain_utf8: Atom,
    targets: Atom,
    incr: Atom,
    property: Atom,
    string: Atom,
    atom: Atom,
}

fn is_plain_utf8_type_name(type_name: &str) -> bool {
    type_name.eq_ignore_ascii_case(PLAIN_UTF8_TYPE) || type_name.eq_ignore_ascii_case("text/plain")
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
            if owned.type_atom == self.atoms.utf8_string
                || owned.type_atom == self.atoms.plain_utf8
                || is_plain_utf8_type_name(&owned.type_name)
            {
                targets.push(self.atoms.utf8_string);
                targets.push(self.atoms.string);
                targets.push(self.atoms.plain_utf8);
            } else if owned.type_atom == self.atoms.utf8_string {
                targets.push(self.atoms.string);
            }
            targets.sort_unstable();
            targets.dedup();
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
                    || request.target == self.atoms.string
                    || request.target == self.atoms.plain_utf8))
            || (owned.type_atom == self.atoms.plain_utf8
                && (request.target == self.atoms.plain_utf8
                    || request.target == self.atoms.utf8_string
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
            if owned.type_atom == self.atoms.utf8_string
                || owned.type_atom == self.atoms.plain_utf8
                || owned.type_name == "UTF8_STRING"
                || is_plain_utf8_type_name(&owned.type_name)
            {
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
                || (is_plain_utf8_type_name(type_name)
                    && (owned.type_atom == self.atoms.utf8_string
                        || owned.type_atom == self.atoms.plain_utf8
                        || is_plain_utf8_type_name(&owned.type_name)))
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
        match self.convert_clipboard_bytes(target, max_read_bytes, timeout) {
            Ok(bytes) => Ok(bytes),
            Err(error)
                if is_plain_utf8_type_name(type_name) && target != self.atoms.utf8_string =>
            {
                self.convert_clipboard_bytes(self.atoms.utf8_string, max_read_bytes, timeout)
                    .map_err(|_| error)
            }
            Err(error) => Err(error),
        }
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

    fn available_types(&mut self, timeout: Duration) -> Result<Vec<String>, ClipboardError> {
        self.pump()?;
        if let Some(owned) = self.owned.as_ref() {
            return Ok(self.owned_type_names(owned));
        }
        let owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(|error| backend(format!("X11 GetSelectionOwner send failed: {error}")))?
            .reply()
            .map_err(|error| backend(format!("X11 GetSelectionOwner failed: {error}")))?
            .owner;
        if owner == NONE {
            return Ok(Vec::new());
        }
        let atoms = self.convert_clipboard_atoms(self.atoms.targets, timeout)?;
        let mut names = Vec::new();
        for atom in atoms {
            if names.len() >= crate::contract::clipboard::MAX_CLIPBOARD_TYPES {
                break;
            }
            names.push(self.atom_name(atom)?);
        }
        Ok(names)
    }

    fn owned_type_names(&self, owned: &OwnedSelection) -> Vec<String> {
        let mut names = vec!["TARGETS".to_owned(), owned.type_name.clone()];
        let utf8ish = owned.type_atom == self.atoms.utf8_string
            || owned.type_atom == self.atoms.plain_utf8
            || owned.type_name == "UTF8_STRING"
            || is_plain_utf8_type_name(&owned.type_name);
        if utf8ish {
            for alias in ["UTF8_STRING", "STRING", PLAIN_UTF8_TYPE] {
                if !names.iter().any(|name| name == alias) {
                    names.push(alias.to_owned());
                }
            }
        } else if owned.type_atom == self.atoms.utf8_string && owned.type_name != "STRING" {
            names.push("STRING".to_owned());
        }
        names.truncate(crate::contract::clipboard::MAX_CLIPBOARD_TYPES);
        names
    }

    fn atom_name(&self, atom: Atom) -> Result<String, ClipboardError> {
        if atom == self.atoms.targets {
            return Ok("TARGETS".to_owned());
        }
        if atom == self.atoms.utf8_string {
            return Ok("UTF8_STRING".to_owned());
        }
        if atom == self.atoms.string {
            return Ok("STRING".to_owned());
        }
        if atom == self.atoms.plain_utf8 {
            return Ok(PLAIN_UTF8_TYPE.to_owned());
        }
        if atom == self.atoms.incr {
            return Ok("INCR".to_owned());
        }
        if atom == self.atoms.clipboard {
            return Ok("CLIPBOARD".to_owned());
        }
        if atom == self.atoms.atom {
            return Ok("ATOM".to_owned());
        }
        self.conn
            .get_atom_name(atom)
            .map_err(|error| backend(format!("X11 GetAtomName send failed: {error}")))?
            .reply()
            .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
            .map_err(|error| backend(format!("X11 GetAtomName failed: {error}")))
    }

    fn convert_clipboard_atoms(
        &mut self,
        target: Atom,
        timeout: Duration,
    ) -> Result<Vec<Atom>, ClipboardError> {
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
                        return Err(backend("X11 selection owner did not offer TARGETS"));
                    }
                    return self.read_property_atoms(notify.property);
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

    fn read_property_atoms(&self, property: Atom) -> Result<Vec<Atom>, ClipboardError> {
        let reply = self
            .conn
            .get_property(
                true,
                self.window,
                property,
                AtomEnum::ANY,
                0,
                TARGETS_PROBE_ATOMS as u32,
            )
            .map_err(|error| backend(format!("X11 GetProperty send failed: {error}")))?
            .reply()
            .map_err(|error| backend(format!("X11 GetProperty failed: {error}")))?;
        if reply.format == 0 {
            return Ok(Vec::new());
        }
        if reply.format != 32 {
            return Err(backend(format!(
                "X11 TARGETS property has format {}, expected 32",
                reply.format
            )));
        }
        reply
            .value32()
            .map(|values| values.take(TARGETS_PROBE_ATOMS).map(Atom::from).collect())
            .ok_or_else(|| backend("X11 TARGETS property is not a 32-bit atom array"))
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
        plain_utf8: intern(conn, PLAIN_UTF8_TYPE.as_bytes())?,
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

pub(super) fn available_types(timeout: Duration) -> Result<Vec<String>, ClipboardError> {
    with_state(|state| state.available_types(timeout))
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
                && (owned.type_atom == state.atoms.utf8_string
                    || owned.type_atom == state.atoms.plain_utf8
                    || owned.type_name == "UTF8_STRING"
                    || is_plain_utf8_type_name(&owned.type_name))
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
                PLAIN_UTF8_TYPE,
                "TARGETS",
                "INCR",
                "PLATFORM_CLIPBOARD"
            ]
        );
    }

    #[test]
    fn native_x11_available_types_when_display_is_set() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let marker = format!(
            "agenterm-linux-x11-types-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        super::set_text(&marker, Duration::from_millis(500)).expect("set_text");
        let types = super::available_types(Duration::from_millis(500)).expect("available_types");
        assert!(
            types
                .iter()
                .any(|name| name == "UTF8_STRING" || name == "STRING"),
            "expected UTF8_STRING or STRING in {types:?}"
        );
        assert!(types.iter().any(|name| name == "TARGETS"));
        assert!(
            types.iter().any(|name| name == PLAIN_UTF8_TYPE),
            "expected {PLAIN_UTF8_TYPE} in {types:?}"
        );
        super::clear(Duration::from_millis(500)).expect("clear");
        let empty = super::available_types(Duration::from_millis(500)).expect("empty types");
        assert!(
            empty.is_empty(),
            "cleared clipboard should enumerate no types"
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

    fn intern_atoms_names() -> [&'static str; 6] {
        [
            "CLIPBOARD",
            "UTF8_STRING",
            PLAIN_UTF8_TYPE,
            "TARGETS",
            "INCR",
            "PLATFORM_CLIPBOARD",
        ]
    }
}
