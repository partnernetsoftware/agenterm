//! Bounded typed client for AgenTerm's product-owned local control protocol.
//!
//! This crate owns no terminal state and no OS policy. It lets small sibling
//! products such as `agenterm-cu` reuse AgenTerm's newline-framed protocol
//! without linking the GUI/product crate, parsing human output, or spawning a
//! second CLI process.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use agenterm_platform::ipc::{IpcEndpoint, NativeStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const RESPONSE_MAX_BYTES: u64 = 8 * 1024 * 1024;
const SCOPE_NAMESPACE: &str = "agenterm.server-scope.v1";
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    Query,
    Mutation,
}

impl Intent {
    fn wire(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ControlClient {
    endpoint: IpcEndpoint,
    server_scope_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    pub output: String,
    pub error: String,
    #[serde(default)]
    pub error_code: String,
    #[serde(default)]
    pub error_category: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub receipt: Option<Value>,
}

#[derive(Debug)]
pub struct ClientError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ClientError {}

impl ControlClient {
    /// Resolve the same native endpoint identity as AgenTerm. Explicit
    /// `AGENTERM_IPC_ENDPOINT` / legacy loopback `AGENTERM_IPC_ADDRESS` win;
    /// otherwise `AGENTERM_INSTANCE` (default `main`) selects the native pipe
    /// or socket. Legacy registry discovery remains in the full product CLI.
    pub fn from_environment() -> Result<Self, ClientError> {
        let instance = std::env::var("AGENTERM_INSTANCE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "main".to_owned());
        validate_instance(&instance)?;
        let server_scope_id = server_scope_id(&instance)?;
        let endpoint = if let Some(raw) = nonempty_env("AGENTERM_IPC_ENDPOINT") {
            raw.parse::<IpcEndpoint>()
                .map_err(|error| client_error("control_endpoint_invalid", error.to_string()))?
        } else if let Some(raw) = nonempty_env("AGENTERM_IPC_ADDRESS") {
            IpcEndpoint::from_legacy_address(&raw)
                .map_err(|error| client_error("control_endpoint_invalid", error.to_string()))?
        } else {
            default_native_endpoint(&server_scope_id)
        };
        endpoint
            .validate_local()
            .map_err(|error| client_error("control_endpoint_invalid", error.to_string()))?;
        Ok(Self {
            endpoint,
            server_scope_id,
        })
    }

    pub fn server_scope_id(&self) -> &str {
        &self.server_scope_id
    }

    pub fn request(
        &self,
        args: Vec<String>,
        operation_id: &str,
        intent: Intent,
        timeout: Duration,
    ) -> Result<ControlResponse, ClientError> {
        if timeout.is_zero() {
            return Err(client_error(
                "control_timeout_invalid",
                "control timeout must be greater than zero",
            ));
        }
        validate_identifier("operation_id", operation_id)?;
        let payload = serde_json::to_vec(&args)
            .map_err(|error| client_error("control_request_invalid", error.to_string()))?;
        let request_id = format!(
            "acu:{}:{}",
            std::process::id(),
            NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
        );
        let deadline_unix_ms =
            now_ms().saturating_add(timeout.as_millis().min(u128::from(u64::MAX)) as u64);
        let envelope = json!({
            "args": args,
            "control": {
                "schema_version": 1,
                "request_id": request_id,
                "operation_id": operation_id,
                "payload_fingerprint": fingerprint(&payload),
                "intent": intent.wire(),
                "deadline_unix_ms": deadline_unix_ms,
            }
        });
        let encoded = serde_json::to_vec(&envelope)
            .map_err(|error| client_error("control_request_invalid", error.to_string()))?;
        let deadline = Instant::now() + timeout;
        let mut stream = Stream::connect(&self.endpoint, timeout.min(Duration::from_millis(100)))?;
        stream.set_timeout(deadline.saturating_duration_since(Instant::now()))?;
        stream
            .write_all(&encoded)
            .and_then(|()| stream.write_all(b"\n"))
            .and_then(|()| stream.flush())
            .map_err(|error| client_error("control_write_failed", error.to_string()))?;
        stream.set_timeout(deadline.saturating_duration_since(Instant::now()))?;
        let line = read_bounded_line(BufReader::new(stream), RESPONSE_MAX_BYTES)?;
        serde_json::from_str(&line)
            .map_err(|error| client_error("control_response_invalid", error.to_string()))
    }
}

enum Stream {
    Native(NativeStream),
    Tcp(TcpStream),
}

impl Stream {
    fn connect(endpoint: &IpcEndpoint, timeout: Duration) -> Result<Self, ClientError> {
        match endpoint {
            IpcEndpoint::Tcp { host, port } => {
                let address = format_tcp_authority(host, *port).parse().map_err(|error| {
                    client_error("control_endpoint_invalid", format!("{error}"))
                })?;
                TcpStream::connect_timeout(&address, timeout)
                    .map(Self::Tcp)
                    .map_err(|error| client_error("control_unavailable", error.to_string()))
            }
            IpcEndpoint::UnixSocket(_) | IpcEndpoint::NamedPipe(_) => {
                NativeStream::connect(endpoint, timeout)
                    .map(Self::Native)
                    .map_err(|error| client_error("control_unavailable", error.to_string()))
            }
            _ => Err(client_error(
                "control_endpoint_invalid",
                "unsupported local IPC endpoint",
            )),
        }
    }

    fn set_timeout(&mut self, timeout: Duration) -> Result<(), ClientError> {
        if timeout.is_zero() {
            return Err(client_error("control_timeout", "control request timed out"));
        }
        match self {
            Self::Native(stream) => stream
                .set_io_timeout(timeout)
                .map_err(|error| client_error("control_timeout", error.to_string())),
            Self::Tcp(stream) => stream
                .set_read_timeout(Some(timeout))
                .and_then(|()| stream.set_write_timeout(Some(timeout)))
                .map_err(|error| client_error("control_timeout", error.to_string())),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Native(stream) => stream.read(buffer),
            Self::Tcp(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Native(stream) => stream.write(buffer),
            Self::Tcp(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Native(stream) => stream.flush(),
            Self::Tcp(stream) => stream.flush(),
        }
    }
}

fn read_bounded_line(mut reader: impl BufRead, maximum: u64) -> Result<String, ClientError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(maximum + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| client_error("control_read_failed", error.to_string()))?;
    if bytes.len() as u64 > maximum {
        return Err(client_error(
            "control_response_too_large",
            "AgenTerm control response exceeded the bounded frame",
        ));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(client_error(
            "control_response_incomplete",
            "AgenTerm control response ended before its newline frame",
        ));
    }
    bytes.pop();
    String::from_utf8(bytes)
        .map_err(|error| client_error("control_response_invalid", error.to_string()))
}

fn server_scope_id(instance: &str) -> Result<String, ClientError> {
    let identity = agenterm_platform::ipc::trusted_user_identity()
        .map_err(|error| client_error("control_identity_unavailable", error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(SCOPE_NAMESPACE.as_bytes());
    digest.update([0]);
    digest.update(identity.kind.as_bytes());
    digest.update([0]);
    digest.update(identity.bytes);
    digest.update([0]);
    digest.update(canonical_instance(instance).as_bytes());
    let bytes = digest.finalize();
    let mut encoded = String::from("agt-v1-");
    for byte in &bytes[..16] {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(encoded)
}

fn default_native_endpoint(scope: &str) -> IpcEndpoint {
    if cfg!(windows) {
        IpcEndpoint::NamedPipe(format!(r"\\.\pipe\agenterm-{scope}"))
    } else {
        IpcEndpoint::UnixSocket(
            agenterm_platform::ipc::native_runtime_directory()
                .join("agenterm")
                .join(format!("{scope}.sock"))
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn canonical_instance(instance: &str) -> String {
    match instance {
        "main" | "dev" => instance.to_owned(),
        value if value.starts_with("custom:") || value.starts_with("ephemeral:") => {
            value.to_owned()
        }
        value => format!("custom:{value}"),
    }
}

fn validate_instance(instance: &str) -> Result<(), ClientError> {
    let value = instance
        .strip_prefix("custom:")
        .or_else(|| instance.strip_prefix("ephemeral:"))
        .unwrap_or(instance);
    if value.is_empty() || value.len() > 96 || value.chars().any(char::is_control) {
        return Err(client_error(
            "control_instance_invalid",
            "AgenTerm instance must be non-empty, bounded and contain no control characters",
        ));
    }
    if instance.contains(':')
        && !instance.starts_with("custom:")
        && !instance.starts_with("ephemeral:")
    {
        return Err(client_error(
            "control_instance_invalid",
            "unknown AgenTerm instance prefix",
        ));
    }
    Ok(())
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ClientError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(client_error(
            "control_request_invalid",
            format!("{field} is not a valid control identifier"),
        ));
    }
    Ok(())
}

fn fingerprint(payload: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in payload {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn format_tcp_authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn client_error(code: &'static str, message: impl Into<String>) -> ClientError {
    ClientError {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn fingerprint_matches_the_product_contract() {
        assert_eq!(
            fingerprint(br#"["capture-pane","--json"]"#),
            "fnv1a64:db143d1fc68804b0"
        );
    }

    #[test]
    fn instance_canonicalization_matches_product_identity() {
        assert_eq!(canonical_instance("main"), "main");
        assert_eq!(canonical_instance("work"), "custom:work");
        assert!(validate_instance("unknown:work").is_err());
    }

    #[test]
    fn bounded_reader_requires_one_complete_frame() {
        assert_eq!(read_bounded_line(&b"ok\n"[..], 3).unwrap(), "ok");
        assert_eq!(
            read_bounded_line(&b"four\n"[..], 3).unwrap_err().code,
            "control_response_too_large"
        );
        assert_eq!(
            read_bounded_line(&b"open"[..], 8).unwrap_err().code,
            "control_response_incomplete"
        );
    }

    #[test]
    fn request_writes_typed_control_metadata_and_reads_one_bounded_reply() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["args"], json!(["capture-pane", "--json"]));
            assert_eq!(request["control"]["operation_id"], "pane.capture");
            assert_eq!(request["control"]["intent"], "query");
            assert_eq!(
                request["control"]["payload_fingerprint"],
                "fnv1a64:db143d1fc68804b0"
            );
            let mut stream = reader.into_inner();
            stream
                .write_all(b"{\"ok\":true,\"output\":\"{}\",\"error\":\"\"}\n")
                .unwrap();
        });
        let client = ControlClient {
            endpoint: IpcEndpoint::Tcp {
                host: address.ip().to_string(),
                port: address.port(),
            },
            server_scope_id: "agt-v1-test".into(),
        };
        let response = client
            .request(
                vec!["capture-pane".into(), "--json".into()],
                "pane.capture",
                Intent::Query,
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(response.ok);
        server.join().unwrap();
    }
}
