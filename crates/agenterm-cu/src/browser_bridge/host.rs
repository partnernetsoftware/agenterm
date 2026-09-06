use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Duration,
};

use agenterm_platform::{
    entropy::secure_random_array,
    filesystem::{host_directories, protect_private_directory, write_private_atomic},
    filesystem_open::{ExistingEntryType, open_existing_path},
    ipc::{IpcEndpoint, IpcTransportErrorCode, NativeListener, NativeStream},
    process_observation, user_identity,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    ACU_EXTENSION_ID, ACU_NATIVE_HOST_NAME, BridgeProtocolError, BridgeRequest, ConnectionEndpoint,
    ConnectionEntry, ConnectionId, DebugReadFailure, DebugReadRequest, DebugReadResult,
    NATIVE_MESSAGE_MAX_BYTES, PROTOCOL_VERSION, ProcessIdentity, REQUEST_LEDGER_MAX_ENTRIES,
    TabsResult, WindowStateRequest, WindowStateResult, WindowsResult, decode_request,
    encode_native_message,
};

const CONNECTION_SCHEMA: u32 = 1;
const CONNECTION_RECORD_MAX_BYTES: usize = 16 * 1024;
const CONNECTION_SCAN_MAX: usize = 128;
const IPC_TIMEOUT: Duration = Duration::from_secs(35);
const ACCEPT_TICK: Duration = Duration::from_millis(250);
const BROWSER_INPUT_QUEUE: usize = 2;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeStatus {
    pub protocol: u32,
    pub extension_id: String,
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeWireError {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detach: Option<super::DetachOutcome>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeResponse {
    pub protocol: u32,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeWireError>,
}

impl BridgeResponse {
    fn validate_for(&self, request: &BridgeRequest) -> Result<(), BridgeHostError> {
        if self.protocol != PROTOCOL_VERSION || self.id != request.id {
            return Err(BridgeHostError::new(
                "browser_bridge_response_identity_mismatch",
            ));
        }
        if self.ok != self.result.is_some() || self.ok == self.error.is_some() {
            return Err(BridgeHostError::new("browser_bridge_response_invalid"));
        }
        if let Some(error) = &self.error {
            validate_wire_error(error, request)?;
            return Ok(());
        }
        let result = self.result.clone().expect("validated success result");
        match request.command.as_str() {
            "status" => {
                let status: BridgeStatus = serde_json::from_value(result)
                    .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?;
                if status.protocol != PROTOCOL_VERSION
                    || status.extension_id != ACU_EXTENSION_ID
                    || status.commands
                        != ["status", "tabs", "windows", "window-state", "debug-read"]
                {
                    return Err(BridgeHostError::new(
                        "browser_bridge_status_identity_mismatch",
                    ));
                }
            }
            "tabs" => serde_json::from_value::<TabsResult>(result)
                .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?
                .validate()
                .map_err(BridgeHostError::protocol)?,
            "windows" => serde_json::from_value::<WindowsResult>(result)
                .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?
                .validate()
                .map_err(BridgeHostError::protocol)?,
            "window-state" => {
                let args: WindowStateRequest =
                    serde_json::from_value(Value::Object(request.args.clone()))
                        .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?;
                serde_json::from_value::<WindowStateResult>(result)
                    .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?
                    .validate_for(&args)
                    .map_err(BridgeHostError::protocol)?;
            }
            "debug-read" => {
                let args: DebugReadRequest =
                    serde_json::from_value(Value::Object(request.args.clone()))
                        .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?;
                serde_json::from_value::<DebugReadResult>(result)
                    .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?
                    .validate_for(&args)
                    .map_err(BridgeHostError::protocol)?;
            }
            _ => return Err(BridgeHostError::new("browser_bridge_command_unknown")),
        }
        Ok(())
    }
}

fn validate_wire_error(
    error: &BridgeWireError,
    request: &BridgeRequest,
) -> Result<(), BridgeHostError> {
    if request.command == "debug-read" && (error.tab_id.is_some() || error.detach.is_some()) {
        let args: DebugReadRequest = serde_json::from_value(Value::Object(request.args.clone()))
            .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?;
        DebugReadFailure {
            tab_id: error
                .tab_id
                .ok_or_else(|| BridgeHostError::new("browser_bridge_response_invalid"))?,
            code: error.code.clone(),
            detach: error
                .detach
                .clone()
                .ok_or_else(|| BridgeHostError::new("browser_bridge_response_invalid"))?,
        }
        .validate_for(&args)
        .map_err(BridgeHostError::protocol)
    } else if error.tab_id.is_none()
        && error.detach.is_none()
        && !error.code.is_empty()
        && error.code.len() <= 96
        && !error.code.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(BridgeHostError::new("browser_bridge_response_invalid"))
    }
}

#[derive(Clone, Debug)]
enum LedgerEntry {
    Reserved([u8; 32]),
    Complete([u8; 32], BridgeResponse),
}

#[derive(Default)]
pub struct RequestLedger {
    entries: BTreeMap<String, LedgerEntry>,
}

enum Admission {
    New,
    Replay(BridgeResponse),
}

impl RequestLedger {
    fn admit(&mut self, request: &BridgeRequest) -> Result<Admission, BridgeHostError> {
        let digest = request_digest(request)?;
        match self.entries.get(&request.id) {
            Some(LedgerEntry::Complete(original, response)) if original == &digest => {
                Ok(Admission::Replay(response.clone()))
            }
            Some(LedgerEntry::Reserved(original)) if original == &digest => {
                Err(BridgeHostError::new("browser_bridge_request_uncertain"))
            }
            Some(_) => Err(BridgeHostError::new("browser_bridge_request_conflict")),
            None if self.entries.len() >= REQUEST_LEDGER_MAX_ENTRIES => {
                Err(BridgeHostError::new("browser_bridge_request_ledger_full"))
            }
            None => {
                self.entries
                    .insert(request.id.clone(), LedgerEntry::Reserved(digest));
                Ok(Admission::New)
            }
        }
    }

    fn complete(&mut self, request: &BridgeRequest, response: BridgeResponse) {
        let LedgerEntry::Reserved(digest) = self
            .entries
            .get(&request.id)
            .expect("admitted request remains reserved")
        else {
            unreachable!("only a reserved request can complete")
        };
        self.entries
            .insert(request.id.clone(), LedgerEntry::Complete(*digest, response));
    }
}

fn request_digest(request: &BridgeRequest) -> Result<[u8; 32], BridgeHostError> {
    let encoded = serde_json::to_vec(request)
        .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?;
    Ok(Sha256::digest(encoded).into())
}

/// Routes one already-bounded local request to the MV3 port. Reservation is
/// made before stdout is touched; an interrupted exchange therefore stays
/// uncertain and is never sent twice under the same identity.
#[cfg(test)]
fn exchange_one(
    ledger: &mut RequestLedger,
    browser_in: &mut impl Read,
    browser_out: &mut impl Write,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeHostError> {
    request.validate().map_err(BridgeHostError::protocol)?;
    match ledger.admit(&request)? {
        Admission::Replay(response) => return Ok(response),
        Admission::New => {}
    }
    let frame = encode_native_message(
        &serde_json::to_value(&request)
            .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?,
    )
    .map_err(BridgeHostError::protocol)?;
    browser_out
        .write_all(&frame)
        .and_then(|()| browser_out.flush())
        .map_err(|_| BridgeHostError::new("browser_bridge_extension_write_failed"))?;
    let value = read_frame(browser_in)?;
    let response: BridgeResponse = serde_json::from_value(value)
        .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?;
    response.validate_for(&request)?;
    ledger.complete(&request, response.clone());
    Ok(response)
}

struct BrowserInput {
    messages: Receiver<Result<Value, BridgeHostError>>,
    terminal: Arc<AtomicBool>,
}

impl BrowserInput {
    fn spawn(mut reader: impl Read + Send + 'static) -> Self {
        let (sender, messages) = mpsc::sync_channel(BROWSER_INPUT_QUEUE);
        let terminal = Arc::new(AtomicBool::new(false));
        let thread_terminal = Arc::clone(&terminal);
        std::thread::spawn(move || {
            loop {
                match read_frame(&mut reader) {
                    Ok(value) => {
                        if sender.try_send(Ok(value)).is_err() {
                            thread_terminal.store(true, Ordering::Release);
                            return;
                        }
                    }
                    Err(error) => {
                        thread_terminal.store(true, Ordering::Release);
                        let _ = sender.try_send(Err(error));
                        return;
                    }
                }
            }
        });
        Self { messages, terminal }
    }

    fn is_terminal(&self) -> bool {
        self.terminal.load(Ordering::Acquire)
    }

    fn receive(&self) -> Result<Value, BridgeHostError> {
        self.messages
            .recv_timeout(IPC_TIMEOUT)
            .map_err(|_| BridgeHostError::new("browser_bridge_extension_read_failed"))?
    }
}

fn exchange_from_browser_input(
    ledger: &mut RequestLedger,
    browser_in: &BrowserInput,
    browser_out: &mut impl Write,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeHostError> {
    request.validate().map_err(BridgeHostError::protocol)?;
    match ledger.admit(&request)? {
        Admission::Replay(response) => return Ok(response),
        Admission::New => {}
    }
    let frame = encode_native_message(
        &serde_json::to_value(&request)
            .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?,
    )
    .map_err(BridgeHostError::protocol)?;
    browser_out
        .write_all(&frame)
        .and_then(|()| browser_out.flush())
        .map_err(|_| BridgeHostError::new("browser_bridge_extension_write_failed"))?;
    let response: BridgeResponse = serde_json::from_value(browser_in.receive()?)
        .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?;
    response.validate_for(&request)?;
    ledger.complete(&request, response.clone());
    Ok(response)
}

fn read_frame(reader: &mut impl Read) -> Result<Value, BridgeHostError> {
    let mut header = [0_u8; 4];
    reader
        .read_exact(&mut header)
        .map_err(|_| BridgeHostError::new("browser_bridge_message_missing"))?;
    let size = u32::from_le_bytes(header) as usize;
    if size > NATIVE_MESSAGE_MAX_BYTES {
        return Err(BridgeHostError::new("browser_bridge_message_too_large"));
    }
    let mut body = vec![0; size];
    reader
        .read_exact(&mut body)
        .map_err(|_| BridgeHostError::new("browser_bridge_message_truncated"))?;
    serde_json::from_slice(&body)
        .map_err(|_| BridgeHostError::new("browser_bridge_message_invalid"))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConnectionRecord {
    schema_version: u32,
    owner_kind: String,
    owner_digest: String,
    entry: ConnectionEntry,
}

fn connection_root() -> Result<PathBuf, BridgeHostError> {
    Ok(host_directories()
        .map_err(|_| BridgeHostError::new("browser_bridge_state_unavailable"))?
        .local_data
        .join("agenterm")
        .join("cu")
        .join("browser-bridge")
        .join("connections"))
}

fn current_owner() -> Result<(String, String), BridgeHostError> {
    let identity = user_identity::current_user_identity()
        .map_err(|_| BridgeHostError::new("browser_bridge_user_identity_unavailable"))?;
    let digest = Sha256::digest(identity.stable_bytes());
    Ok((identity.stable_kind().to_owned(), hex(&digest)))
}

fn endpoint_for(id: &ConnectionId) -> IpcEndpoint {
    let suffix = &id.as_str()[..32];
    #[cfg(windows)]
    return IpcEndpoint::NamedPipe(format!(r"\\.\pipe\agenterm-cu-browser-{suffix}"));
    #[cfg(unix)]
    return IpcEndpoint::UnixSocket(
        agenterm_platform::ipc::native_runtime_directory()
            .join(format!("cu-browser-{suffix}.sock"))
            .to_string_lossy()
            .into_owned(),
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct PublishedConnection {
    path: PathBuf,
}

impl Drop for PublishedConnection {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn publish_connection()
-> Result<(ConnectionRecord, NativeListener, PublishedConnection), BridgeHostError> {
    publish_connection_at(&connection_root()?)
}

fn publish_connection_at(
    root: &std::path::Path,
) -> Result<(ConnectionRecord, NativeListener, PublishedConnection), BridgeHostError> {
    fs::create_dir_all(root)
        .map_err(|_| BridgeHostError::new("browser_bridge_state_unavailable"))?;
    protect_private_directory(root)
        .map_err(|_| BridgeHostError::new("browser_bridge_state_unavailable"))?;
    let random = secure_random_array::<32>()
        .map_err(|_| BridgeHostError::new("browser_bridge_entropy_unavailable"))?;
    let id = ConnectionId::from_random(random)
        .map_err(|_| BridgeHostError::new("browser_bridge_entropy_unavailable"))?;
    let pid = std::process::id();
    let start_identity = process_observation::start_identity(pid)
        .map_err(|_| BridgeHostError::new("browser_bridge_process_identity_unavailable"))?;
    let endpoint = endpoint_for(&id);
    let listener = NativeListener::bind(&endpoint)
        .map_err(|_| BridgeHostError::new("browser_bridge_endpoint_unavailable"))?;
    let (owner_kind, owner_digest) = current_owner()?;
    let record = ConnectionRecord {
        schema_version: CONNECTION_SCHEMA,
        owner_kind,
        owner_digest,
        entry: ConnectionEntry {
            connection_id: id.clone(),
            process: ProcessIdentity {
                pid,
                start_identity,
            },
            endpoint: ConnectionEndpoint::NativeMessaging {
                native_host: ACU_NATIVE_HOST_NAME.to_owned(),
                extension_id: ACU_EXTENSION_ID.to_owned(),
            },
        },
    };
    let path = root.join(format!("{}.json", id.as_str()));
    let encoded = serde_json::to_vec(&record)
        .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    write_private_atomic(&path, &encoded)
        .map_err(|_| BridgeHostError::new("browser_bridge_state_unavailable"))?;
    Ok((record, listener, PublishedConnection { path }))
}

/// Browser-owned Native Messaging mode. This is not a daemon: its lifetime is
/// exactly the browser-created stdio connection, and the browser terminates it
/// when that connection closes.
pub fn run_native_host(origin: &str) -> Result<(), BridgeHostError> {
    if origin != format!("chrome-extension://{ACU_EXTENSION_ID}/") {
        return Err(BridgeHostError::new("browser_bridge_origin_invalid"));
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_native_host_with_io(stdin, stdout.lock(), None)
}

fn run_native_host_with_io(
    browser_in: impl Read + Send + 'static,
    mut browser_out: impl Write,
    connection_root_override: Option<PathBuf>,
) -> Result<(), BridgeHostError> {
    let (_record, mut listener, _published) = match connection_root_override {
        Some(root) => publish_connection_at(&root)?,
        None => publish_connection()?,
    };
    let browser_in = BrowserInput::spawn(browser_in);
    let mut ledger = RequestLedger::default();
    loop {
        if browser_in.is_terminal() {
            return Ok(());
        }
        match listener.accept(ACCEPT_TICK) {
            Ok(mut stream) => {
                if stream.set_io_timeout(IPC_TIMEOUT).is_err() {
                    continue;
                }
                let response = serve_local(&mut ledger, &browser_in, &mut browser_out, &mut stream);
                if let Ok(response) = response {
                    let encoded = serde_json::to_value(response)
                        .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?;
                    write_frame(&mut stream, &encoded)?;
                    let _ = stream.finish_server_response();
                }
            }
            Err(error) if error.code == IpcTransportErrorCode::AcceptTimeout => {}
            Err(_) => return Err(BridgeHostError::new("browser_bridge_endpoint_unavailable")),
        }
    }
}

fn serve_local(
    ledger: &mut RequestLedger,
    browser_in: &BrowserInput,
    browser_out: &mut impl Write,
    local: &mut impl Read,
) -> Result<BridgeResponse, BridgeHostError> {
    let request = decode_request(read_frame(local)?).map_err(BridgeHostError::protocol)?;
    match exchange_from_browser_input(ledger, browser_in, browser_out, request.clone()) {
        Ok(response) => Ok(response),
        Err(error) => Ok(BridgeResponse {
            protocol: PROTOCOL_VERSION,
            id: request.id,
            ok: false,
            result: None,
            error: Some(BridgeWireError {
                code: error.code,
                tab_id: None,
                detach: None,
            }),
        }),
    }
}

fn write_frame(writer: &mut impl Write, value: &Value) -> Result<(), BridgeHostError> {
    let frame = encode_native_message(value).map_err(BridgeHostError::protocol)?;
    writer
        .write_all(&frame)
        .and_then(|()| writer.flush())
        .map_err(|_| BridgeHostError::new("browser_bridge_protocol_io"))
}

pub fn send_to_connection(
    connection_id: &ConnectionId,
    request: &BridgeRequest,
) -> Result<BridgeResponse, BridgeHostError> {
    request.validate().map_err(BridgeHostError::protocol)?;
    let record = load_live_record(connection_id)?;
    let endpoint = endpoint_for(&record.entry.connection_id);
    let mut stream = NativeStream::connect(&endpoint, Duration::from_secs(2))
        .map_err(|_| BridgeHostError::new("browser_bridge_host_unavailable"))?;
    stream
        .set_io_timeout(IPC_TIMEOUT)
        .map_err(|_| BridgeHostError::new("browser_bridge_protocol_io"))?;
    let value = serde_json::to_value(request)
        .map_err(|_| BridgeHostError::new("browser_bridge_request_invalid"))?;
    write_frame(&mut stream, &value)?;
    let response: BridgeResponse = serde_json::from_value(read_frame(&mut stream)?)
        .map_err(|_| BridgeHostError::new("browser_bridge_response_invalid"))?;
    response.validate_for(request)?;
    Ok(response)
}

fn load_live_record(id: &ConnectionId) -> Result<ConnectionRecord, BridgeHostError> {
    load_live_record_at(&connection_root()?, id)
}

fn load_live_record_at(
    root: &std::path::Path,
    id: &ConnectionId,
) -> Result<ConnectionRecord, BridgeHostError> {
    let path = root.join(format!("{}.json", id.as_str()));
    let mut file = open_existing_path(&path, ExistingEntryType::File)
        .map_err(|_| BridgeHostError::new("browser_bridge_connection_not_found"))?;
    let metadata = file
        .metadata()
        .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    if metadata.len() as usize > CONNECTION_RECORD_MAX_BYTES {
        return Err(BridgeHostError::new("browser_bridge_state_invalid"));
    }
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take((CONNECTION_RECORD_MAX_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    if encoded.len() > CONNECTION_RECORD_MAX_BYTES {
        return Err(BridgeHostError::new("browser_bridge_state_invalid"));
    }
    let record: ConnectionRecord = serde_json::from_slice(&encoded)
        .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    validate_live_record(&record, id)?;
    Ok(record)
}

fn validate_live_record(
    record: &ConnectionRecord,
    id: &ConnectionId,
) -> Result<(), BridgeHostError> {
    let (owner_kind, owner_digest) = current_owner()?;
    if record.schema_version != CONNECTION_SCHEMA
        || record.owner_kind != owner_kind
        || record.owner_digest != owner_digest
        || &record.entry.connection_id != id
        || process_observation::start_identity(record.entry.process.pid)
            .ok()
            .as_deref()
            != Some(record.entry.process.start_identity.as_str())
    {
        return Err(BridgeHostError::new("browser_bridge_connection_stale"));
    }
    // Re-serialization through the pure registry validates the fixed endpoint.
    let mut registry = super::ConnectionRegistry::new(format!("{}:{}", owner_kind, owner_digest))
        .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    let _ = registry
        .register(
            record.entry.process.clone(),
            record.entry.endpoint.clone(),
            id_random_bytes(id)?,
        )
        .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    Ok(())
}

fn id_random_bytes(id: &ConnectionId) -> Result<[u8; 32], BridgeHostError> {
    let raw = id.as_str().as_bytes();
    let mut bytes = [0_u8; 32];
    for (index, pair) in raw.chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair)
            .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
        bytes[index] = u8::from_str_radix(text, 16)
            .map_err(|_| BridgeHostError::new("browser_bridge_state_invalid"))?;
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectionInventory {
    pub connections: Vec<ConnectionEntry>,
    /// Candidate records actually identity-validated; never exceeds the scan ceiling.
    pub visited: usize,
    /// More directory entries existed than could enter this bounded inventory.
    pub truncated: bool,
}

pub fn list_live_connections() -> Result<ConnectionInventory, BridgeHostError> {
    inventory_at(&connection_root()?)
}

fn inventory_at(root: &std::path::Path) -> Result<ConnectionInventory, BridgeHostError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ConnectionInventory {
                connections: Vec::new(),
                visited: 0,
                truncated: false,
            });
        }
        Err(_) => return Err(BridgeHostError::new("browser_bridge_state_unavailable")),
    };
    // Keep the lexicographically first candidate ids while walking the whole
    // private directory. Memory and record validation stay bounded, and an OS
    // enumeration prefix can never nondeterministically choose the result.
    let mut candidates = BTreeSet::new();
    let mut directory_entries = 0usize;
    let mut enumeration_incomplete = false;
    for entry in entries {
        let Ok(entry) = entry else {
            enumeration_incomplete = true;
            continue;
        };
        directory_entries = directory_entries.saturating_add(1);
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(id) = ConnectionId::parse(stem) else {
            continue;
        };
        candidates.insert(id);
        if candidates.len() > CONNECTION_SCAN_MAX {
            candidates.pop_last();
        }
    }
    let visited = candidates.len();
    let mut connections = Vec::new();
    for id in candidates {
        if let Ok(record) = load_live_record_at(root, &id) {
            connections.push(record.entry);
        }
    }
    Ok(ConnectionInventory {
        connections,
        visited,
        truncated: enumeration_incomplete || directory_entries > CONNECTION_SCAN_MAX,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeHostError {
    pub code: String,
}

impl BridgeHostError {
    fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
    fn protocol(error: BridgeProtocolError) -> Self {
        Self::new(error.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_bridge::NativeMessageDecoder;
    use serde_json::{Map, json};
    use std::sync::{Condvar, Mutex};

    fn request(command: &str) -> BridgeRequest {
        BridgeRequest {
            protocol: 1,
            id: "request-1".into(),
            command: command.into(),
            args: Map::new(),
        }
    }

    fn response(request: &BridgeRequest, result: Value) -> Vec<u8> {
        encode_native_message(&json!({"protocol":1,"id":request.id,"ok":true,"result":result}))
            .unwrap()
    }

    #[test]
    fn exact_retry_replays_without_second_browser_effect_and_conflict_fails() {
        let request = request("status");
        let status = json!({"protocol":1,"extension_id":ACU_EXTENSION_ID,"commands":["status","tabs","windows","window-state","debug-read"]});
        let response_bytes = response(&request, status);
        let mut browser_in = response_bytes.as_slice();
        let mut browser_out = Vec::new();
        let mut ledger = RequestLedger::default();
        let first = exchange_one(
            &mut ledger,
            &mut browser_in,
            &mut browser_out,
            request.clone(),
        )
        .unwrap();
        let sent_once = browser_out.clone();
        assert_eq!(
            exchange_one(
                &mut ledger,
                &mut io::empty(),
                &mut browser_out,
                request.clone()
            )
            .unwrap(),
            first
        );
        assert_eq!(browser_out, sent_once);
        let mut conflict = request;
        conflict.command = "tabs".into();
        assert_eq!(
            exchange_one(&mut ledger, &mut io::empty(), &mut browser_out, conflict)
                .unwrap_err()
                .code,
            "browser_bridge_request_conflict"
        );
    }

    #[test]
    fn interrupted_exchange_is_uncertain_and_never_replayed() {
        let request = request("tabs");
        let mut ledger = RequestLedger::default();
        let mut browser_out = Vec::new();
        assert_eq!(
            exchange_one(
                &mut ledger,
                &mut io::empty(),
                &mut browser_out,
                request.clone()
            )
            .unwrap_err()
            .code,
            "browser_bridge_message_missing"
        );
        let sent_once = browser_out.clone();
        assert_eq!(
            exchange_one(&mut ledger, &mut io::empty(), &mut browser_out, request)
                .unwrap_err()
                .code,
            "browser_bridge_request_uncertain"
        );
        assert_eq!(browser_out, sent_once);
    }

    #[test]
    fn response_validation_keeps_focus_detach_and_output_bounds_closed() {
        let mut valid_request = request("debug-read");
        valid_request.args = serde_json::from_value(
            json!({"tab_id":7,"max_frames":1,"max_depth":2,"max_scan":3,"max_results":2}),
        )
        .unwrap();
        let valid = json!({"tab_id":7,"frame_count":1,"scanned":1,"truncated":false,"nodes":[{"frame_id":"f","backend_node_id":9,"depth":1,"role":"heading","name":"Account"}],"presentation":{"tab_active_before":false,"tab_active_after":false,"window_focused_before":false,"window_focused_after":false,"activation_requested":false},"detach":{"outcome":"detached"}});
        let response_bytes = response(&valid_request, valid);
        let mut browser_in = response_bytes.as_slice();
        exchange_one(
            &mut RequestLedger::default(),
            &mut browser_in,
            &mut Vec::new(),
            valid_request,
        )
        .unwrap();

        let changed = json!({"tab_id":7,"frame_count":1,"scanned":0,"truncated":false,"nodes":[],"presentation":{"tab_active_before":false,"tab_active_after":true,"window_focused_before":false,"window_focused_after":false,"activation_requested":false},"detach":{"outcome":"detached"}});
        let mut changed_request = request("debug-read");
        changed_request.args = serde_json::from_value(
            json!({"tab_id":7,"max_frames":1,"max_depth":2,"max_scan":3,"max_results":2}),
        )
        .unwrap();
        let response_bytes = response(&changed_request, changed);
        assert_eq!(
            exchange_one(
                &mut RequestLedger::default(),
                &mut response_bytes.as_slice(),
                &mut Vec::new(),
                changed_request
            )
            .unwrap_err()
            .code,
            "browser_bridge_debug_read_presentation_changed"
        );
    }

    #[test]
    fn replay_ledger_has_a_fixed_connection_lifetime_bound() {
        let mut ledger = RequestLedger::default();
        for index in 0..REQUEST_LEDGER_MAX_ENTRIES {
            let mut request = request("status");
            request.id = format!("request-{index}");
            let status = json!({"protocol":1,"extension_id":ACU_EXTENSION_ID,"commands":["status","tabs","windows","window-state","debug-read"]});
            let response_bytes = response(&request, status);
            exchange_one(
                &mut ledger,
                &mut response_bytes.as_slice(),
                &mut Vec::new(),
                request,
            )
            .unwrap();
        }
        let mut overflow = request("status");
        overflow.id = "request-overflow".into();
        assert_eq!(
            exchange_one(&mut ledger, &mut io::empty(), &mut Vec::new(), overflow)
                .unwrap_err()
                .code,
            "browser_bridge_request_ledger_full"
        );
    }

    #[test]
    fn runtime_record_requires_current_user_and_exact_live_process() {
        let id = ConnectionId::from_random([0x31; 32]).unwrap();
        let (owner_kind, owner_digest) = current_owner().unwrap();
        let mut record = ConnectionRecord {
            schema_version: CONNECTION_SCHEMA,
            owner_kind,
            owner_digest,
            entry: ConnectionEntry {
                connection_id: id.clone(),
                process: ProcessIdentity {
                    pid: std::process::id(),
                    start_identity: process_observation::start_identity(std::process::id())
                        .unwrap(),
                },
                endpoint: ConnectionEndpoint::NativeMessaging {
                    native_host: ACU_NATIVE_HOST_NAME.into(),
                    extension_id: ACU_EXTENSION_ID.into(),
                },
            },
        };
        validate_live_record(&record, &id).unwrap();
        record.entry.process.start_identity.push_str("-replacement");
        assert_eq!(
            validate_live_record(&record, &id).unwrap_err().code,
            "browser_bridge_connection_stale"
        );
        record.entry.process.start_identity =
            process_observation::start_identity(std::process::id()).unwrap();
        record.owner_digest.push('0');
        assert_eq!(
            validate_live_record(&record, &id).unwrap_err().code,
            "browser_bridge_connection_stale"
        );
    }

    #[test]
    fn connection_inventory_is_stable_bounded_and_reports_truncation() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-inventory-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let (owner_kind, owner_digest) = current_owner().unwrap();
        let process = ProcessIdentity {
            pid: std::process::id(),
            start_identity: process_observation::start_identity(std::process::id()).unwrap(),
        };
        let mut expected_first = None;
        for serial in (1_u16..=130).rev() {
            let mut random = [0_u8; 32];
            random[30..].copy_from_slice(&serial.to_be_bytes());
            let id = ConnectionId::from_random(random).unwrap();
            if serial == 1 {
                expected_first = Some(id.clone());
            }
            let record = ConnectionRecord {
                schema_version: CONNECTION_SCHEMA,
                owner_kind: owner_kind.clone(),
                owner_digest: owner_digest.clone(),
                entry: ConnectionEntry {
                    connection_id: id.clone(),
                    process: process.clone(),
                    endpoint: ConnectionEndpoint::NativeMessaging {
                        native_host: ACU_NATIVE_HOST_NAME.into(),
                        extension_id: ACU_EXTENSION_ID.into(),
                    },
                },
            };
            fs::write(
                root.join(format!("{}.json", id.as_str())),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
        }
        let inventory = inventory_at(&root).unwrap();
        assert_eq!(inventory.visited, CONNECTION_SCAN_MAX);
        assert!(inventory.truncated);
        assert_eq!(inventory.connections.len(), CONNECTION_SCAN_MAX);
        assert_eq!(
            inventory
                .connections
                .first()
                .map(|entry| &entry.connection_id),
            expected_first.as_ref()
        );
        assert!(
            inventory
                .connections
                .windows(2)
                .all(|pair| pair[0].connection_id < pair[1].connection_id)
        );
        fs::remove_dir_all(root).unwrap();
    }

    struct GatedEofReader {
        state: Arc<(Mutex<bool>, Condvar)>,
        entered: mpsc::SyncSender<()>,
    }

    impl Read for GatedEofReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            let _ = self.entered.try_send(());
            let (lock, changed) = &*self.state;
            let mut closed = lock.lock().unwrap();
            while !*closed {
                closed = changed.wait(closed).unwrap();
            }
            Ok(0)
        }
    }

    #[test]
    fn browser_eof_without_local_request_exits_and_releases_record_and_endpoint() {
        let root =
            std::env::temp_dir().join(format!("agenterm-cu-browser-eof-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let reader = GatedEofReader {
            state: Arc::clone(&state),
            entered: entered_tx,
        };
        let thread_root = root.clone();
        let host = std::thread::spawn(move || {
            run_native_host_with_io(reader, io::sink(), Some(thread_root))
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let record_path = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
            .expect("connection record published before browser read");
        let id = ConnectionId::parse(record_path.file_stem().unwrap().to_str().unwrap()).unwrap();
        let endpoint = endpoint_for(&id);
        {
            let (lock, changed) = &*state;
            *lock.lock().unwrap() = true;
            changed.notify_all();
        }
        host.join().unwrap().unwrap();
        assert!(!record_path.exists());
        let replacement = NativeListener::bind(&endpoint)
            .expect("exact native endpoint is reusable after host EOF cleanup");
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
    }

    struct OneByteReader(io::Cursor<Vec<u8>>);

    impl Read for OneByteReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if buffer.is_empty() {
                Ok(0)
            } else {
                self.0.read(&mut buffer[..1])
            }
        }
    }

    #[test]
    fn sole_browser_reader_preserves_split_frames_during_exchange() {
        let request = request("status");
        let status = json!({"protocol":1,"extension_id":ACU_EXTENSION_ID,"commands":["status","tabs","windows","window-state","debug-read"]});
        let input = BrowserInput::spawn(OneByteReader(io::Cursor::new(response(&request, status))));
        let mut output = Vec::new();
        exchange_from_browser_input(
            &mut RequestLedger::default(),
            &input,
            &mut output,
            request.clone(),
        )
        .unwrap();
        assert_eq!(
            NativeMessageDecoder::default().push(&output).unwrap(),
            vec![serde_json::to_value(request).unwrap()]
        );
    }
}
