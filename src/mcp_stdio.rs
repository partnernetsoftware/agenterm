use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    mcp_acu_mutation::{
        CancelResult, CompletionDisposition, CompletionOutcome, ConnectionMutationState,
        IdempotencyKey, JsonRpcRequestId, MutationRequest, ProviderCompletion,
        SessionEndCompletion, SubmitResult,
    },
    mcp_catalog::{MCP_PROTOCOL_REVISION, capabilities},
    mcp_fleet,
};

const JSON_RPC_VERSION: &str = "2.0";
const ERROR_PARSE: i64 = -32700;
const ERROR_INVALID_REQUEST: i64 = -32600;
const ERROR_METHOD_NOT_FOUND: i64 = -32601;
const ERROR_INVALID_PARAMS: i64 = -32602;
const ERROR_NOT_INITIALIZED: i64 = -32002;
const ERROR_PROTOCOL_VERSION: i64 = -32005;
const ERROR_RESPONSE_TOO_LARGE: i64 = -32004;
const ERROR_ACU_PROVIDER: i64 = -32006;
const MCP_MUTATION_SESSION_TTL_SECONDS: u64 = 3_600;
const MCP_PROVIDER_SHELL_GRACE: Duration = Duration::from_secs(2);
const WAIT_PENDING: u8 = 0;
const WAIT_CANCELLED: u8 = 1;
const WAIT_COMPLETED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionState {
    New,
    InitializeResponded,
    Ready,
}

enum BoundedLine {
    Eof,
    Line(Vec<u8>),
    Oversized,
}

enum ServerEvent {
    Input(BoundedLine),
    InputError(io::Error),
    WaitComplete {
        key: String,
        id: Value,
        result: Result<Value, mcp_fleet::McpFleetError>,
    },
    ProviderComplete(ProviderCompletionEvent),
    ProviderRecovered {
        generation: u64,
    },
}

enum ProviderCall {
    ReadOnly {
        response_id: Value,
        request: String,
    },
    SessionStart,
    ShellExec {
        id: JsonRpcRequestId,
        request: String,
    },
    SessionEnd {
        request: String,
    },
}

enum ProviderMessage {
    Call(ProviderWork),
    Shutdown,
}

struct ProviderWork {
    generation: u64,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
    call: ProviderCall,
}

enum ProviderComplete {
    ReadOnly {
        response_id: Value,
        result: Result<String, String>,
    },
    SessionStart(Result<String, String>),
    ShellExec {
        id: JsonRpcRequestId,
        result: Result<String, String>,
    },
    SessionEnd(Result<String, String>),
}

struct ProviderCompletionEvent {
    generation: u64,
    release_gate: bool,
    completion: ProviderComplete,
}

#[derive(Debug)]
enum ProviderSubmitError {
    Busy,
    Stopped,
}

struct ProviderGate {
    active_generation: AtomicU64,
    quarantined_generation: AtomicU64,
    next_generation: AtomicU64,
    active_read: Mutex<Option<(u64, String, Arc<AtomicBool>)>>,
}

impl ProviderGate {
    fn new() -> Self {
        Self {
            active_generation: AtomicU64::new(0),
            quarantined_generation: AtomicU64::new(0),
            next_generation: AtomicU64::new(1),
            active_read: Mutex::new(None),
        }
    }

    fn acquire(&self) -> Option<u64> {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.active_generation
            .compare_exchange(0, generation, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| generation)
    }

    fn release(&self, generation: u64) {
        if let Ok(mut active) = self.active_read.lock()
            && active
                .as_ref()
                .is_some_and(|(active_generation, _, _)| *active_generation == generation)
        {
            active.take();
        }
        let _ = self.active_generation.compare_exchange(
            generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let _ = self.quarantined_generation.compare_exchange(
            generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn is_busy(&self) -> bool {
        self.active_generation.load(Ordering::Acquire) != 0
    }

    fn quarantine(&self, generation: u64) {
        if self.active_generation.load(Ordering::Acquire) == generation {
            self.quarantined_generation
                .store(generation, Ordering::Release);
        }
    }

    fn is_quarantined(&self) -> bool {
        self.quarantined_generation.load(Ordering::Acquire) != 0
    }

    fn register_read(&self, generation: u64, response_id: &Value, cancel: Arc<AtomicBool>) {
        if let Ok(mut active) = self.active_read.lock() {
            *active = Some((generation, request_id_key(response_id), cancel));
        }
    }

    fn cancel_read(&self, response_id: &Value) {
        let key = request_id_key(response_id);
        if let Ok(active) = self.active_read.lock()
            && let Some((_, active_key, cancel)) = active.as_ref()
            && active_key == &key
        {
            cancel.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone)]
struct ProviderClient {
    sender: mpsc::Sender<ProviderMessage>,
    gate: Arc<ProviderGate>,
    default_timeout: Duration,
}

impl ProviderClient {
    fn submit(&self, call: ProviderCall) -> Result<(), ProviderSubmitError> {
        self.submit_with_timeout(call, self.default_timeout)
    }

    fn submit_with_timeout(
        &self,
        call: ProviderCall,
        timeout: Duration,
    ) -> Result<(), ProviderSubmitError> {
        let generation = self.gate.acquire().ok_or(ProviderSubmitError::Busy)?;
        let cancel = Arc::new(AtomicBool::new(false));
        if let ProviderCall::ReadOnly { response_id, .. } = &call {
            self.gate
                .register_read(generation, response_id, Arc::clone(&cancel));
        }
        if self
            .sender
            .send(ProviderMessage::Call(ProviderWork {
                generation,
                timeout,
                cancel,
                call,
            }))
            .is_err()
        {
            self.gate.release(generation);
            return Err(ProviderSubmitError::Stopped);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ShellExecCommand {
    command: String,
    timeout_ms: u64,
    max_output_bytes: usize,
}

enum MutationLifecycle {
    Dormant,
    Starting {
        first: Option<MutationRequest<ShellExecCommand>>,
        queued: Option<MutationRequest<ShellExecCommand>>,
        eof: bool,
    },
    Active {
        state: ConnectionMutationState<ShellExecCommand>,
        exit_after_end: bool,
    },
}

#[doc(hidden)]
pub trait McpAcuProvider: Send + Sync + 'static {
    fn call(&self, request: &str) -> Result<String, String>;

    fn call_controlled(&self, request: &str, _cancel: &AtomicBool) -> Result<String, String> {
        self.call(request)
    }
}

struct NativeAcuProvider;

impl McpAcuProvider for NativeAcuProvider {
    fn call(&self, request: &str) -> Result<String, String> {
        crate::acu_provider::call(request)
    }

    fn call_controlled(&self, request: &str, cancel: &AtomicBool) -> Result<String, String> {
        match crate::acu_provider::call_controlled(request, Some(cancel)) {
            Err(code) if code == "acu_provider_cooperative_cancel_unavailable" => {
                crate::acu_provider::call(request)
            }
            result => result,
        }
    }
}

struct CleanupWriter<W> {
    inner: W,
    first_error: Option<io::Error>,
}

impl<W> CleanupWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            first_error: None,
        }
    }

    fn take_error(&mut self) -> Option<io::Error> {
        self.first_error.take()
    }

    fn has_error(&self) -> bool {
        self.first_error.is_some()
    }

    fn record(&mut self, error: io::Error) {
        if self.first_error.is_none() {
            self.first_error = Some(error);
        }
    }
}

impl<W: Write> Write for CleanupWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.first_error.is_some() {
            return Ok(bytes.len());
        }
        loop {
            match self.inner.write(bytes) {
                Ok(0) if !bytes.is_empty() => {
                    self.record(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "MCP output closed",
                    ));
                    return Ok(bytes.len());
                }
                Ok(written) => return Ok(written),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.record(error);
                    return Ok(bytes.len());
                }
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.first_error.is_some() {
            return Ok(());
        }
        loop {
            match self.inner.flush() {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.record(error);
                    return Ok(());
                }
            }
        }
    }
}

impl<F> McpAcuProvider for F
where
    F: Fn(&str) -> Result<String, String> + Send + Sync + 'static,
{
    fn call(&self, request: &str) -> Result<String, String> {
        self(request)
    }
}

struct ActiveWait {
    cancelled: Arc<AtomicBool>,
    terminal: Arc<AtomicU8>,
    worker: thread::JoinHandle<()>,
}

#[derive(Clone, Debug, Default)]
pub struct McpStdioConfig {
    pub address: Option<String>,
    #[doc(hidden)]
    pub provider_call_timeout_ms: Option<u64>,
}

pub fn serve_stdio<R: BufRead + Send + 'static, W: Write>(input: R, output: W) -> io::Result<()> {
    serve_stdio_with_config(input, output, McpStdioConfig::default())
}

pub fn serve_stdio_with_config<R: BufRead + Send + 'static, W: Write>(
    input: R,
    output: W,
    config: McpStdioConfig,
) -> io::Result<()> {
    serve_stdio_core(input, output, config, NativeAcuProvider, false)
}

#[doc(hidden)]
pub fn serve_stdio_with_config_and_provider<
    R: BufRead + Send + 'static,
    W: Write,
    P: McpAcuProvider,
>(
    input: R,
    output: W,
    config: McpStdioConfig,
    provider: P,
) -> io::Result<()> {
    serve_stdio_core(input, output, config, provider, true)
}

fn serve_stdio_core<R: BufRead + Send + 'static, W: Write, P: McpAcuProvider>(
    input: R,
    output: W,
    config: McpStdioConfig,
    acu_provider: P,
    mutation_enabled: bool,
) -> io::Result<()> {
    let mut output = CleanupWriter::new(output);
    let limit = capabilities().limits.frame_bytes as usize;
    let (sender, receiver) = mpsc::channel();
    let stop_reader = Arc::new(AtomicBool::new(false));
    let reader = thread::spawn({
        let sender = sender.clone();
        let stop_reader = Arc::clone(&stop_reader);
        move || read_input(input, limit, sender, &stop_reader)
    });
    let (provider_sender, provider_receiver) = mpsc::channel();
    let provider_gate = Arc::new(ProviderGate::new());
    let provider_client = ProviderClient {
        sender: provider_sender,
        gate: Arc::clone(&provider_gate),
        default_timeout: Duration::from_millis(
            config
                .provider_call_timeout_ms
                .unwrap_or_else(|| u64::from(capabilities().limits.provider_call_timeout_ms))
                .max(1),
        ),
    };
    let provider_worker = thread::spawn({
        let sender = sender.clone();
        let gate = Arc::clone(&provider_gate);
        let acu_provider = Arc::new(acu_provider);
        move || provider_loop(acu_provider, provider_receiver, sender, gate)
    });
    let mut state = SessionState::New;
    let mut active = HashMap::<String, ActiveWait>::new();
    let mut mutation = MutationLifecycle::Dormant;
    let mut input_error = None;
    let mut output_disconnected = false;
    loop {
        match receiver.recv() {
            Ok(ServerEvent::Input(_)) | Ok(ServerEvent::InputError(_)) if output_disconnected => {}
            Ok(ServerEvent::Input(BoundedLine::Eof)) => {
                cancel_and_join_waits(&mut active);
                receive_eof_and_maybe_end(&mut mutation, &provider_client)
                    .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
                if matches!(mutation, MutationLifecycle::Dormant)
                    || (mutation_exits_after_provider_timeout(&mutation)
                        && provider_gate.is_quarantined())
                {
                    break;
                }
            }
            Ok(ServerEvent::InputError(error)) => {
                input_error = Some(error);
                cancel_and_join_waits(&mut active);
                receive_eof_and_maybe_end(&mut mutation, &provider_client)
                    .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
                if matches!(mutation, MutationLifecycle::Dormant)
                    || (mutation_exits_after_provider_timeout(&mutation)
                        && provider_gate.is_quarantined())
                {
                    break;
                }
            }
            Ok(ServerEvent::Input(BoundedLine::Oversized)) => {
                write_message(
                    &mut output,
                    &error_response(
                        Value::Null,
                        ERROR_INVALID_REQUEST,
                        "MCP message exceeds the frame limit",
                        Some(json!({"maximum_bytes": limit})),
                    ),
                )?;
            }
            Ok(ServerEvent::Input(BoundedLine::Line(line))) => {
                let message = match decode_line(&line) {
                    Ok(message) => message,
                    Err(response) => {
                        write_message(&mut output, &response)?;
                        continue;
                    }
                };
                if handle_cancel_notification(&message, &active, &provider_gate) {
                    handle_mutation_cancel(&message, &mut mutation, &mut output)?;
                    continue;
                }
                if mutation_enabled
                    && state == SessionState::Ready
                    && is_shell_exec_tool_call(&message)
                {
                    // JSON-RPC notifications never receive a response and
                    // cannot own an idempotent mutation lifecycle. Ignore the
                    // notification before starting the private ACU session.
                    if message.get("id").is_none() {
                        continue;
                    }
                    match submit_shell_exec(&message, &mut mutation, &provider_client) {
                        Ok(()) => {}
                        Err(response) => write_message(&mut output, &response)?,
                    }
                    continue;
                }
                if state == SessionState::Ready && is_wait_tool_call(&message) {
                    match start_wait(
                        &message,
                        &config,
                        &sender,
                        &mut active,
                        capabilities().limits.waiter_concurrency as usize,
                    ) {
                        Ok(()) => {}
                        Err(response) => write_message(&mut output, &response)?,
                    }
                    continue;
                }
                if let Some(response) = process_message(
                    message,
                    &mut state,
                    &config,
                    mutation_enabled,
                    &provider_client,
                ) {
                    write_message(&mut output, &response)?;
                }
            }
            Ok(ServerEvent::WaitComplete { key, id, result }) => {
                let Some(wait) = active.remove(&key) else {
                    continue;
                };
                let _ = wait.worker.join();
                let response = match result {
                    Ok(result) => {
                        let is_error = result["outcome"].as_str() != Some("matched");
                        success_response(
                            id,
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": serde_json::to_string(&result)
                                        .expect("wait result serializes")
                                }],
                                "structuredContent": result,
                                "isError": is_error
                            }),
                        )
                    }
                    Err(error) => error_response(id, error.code, error.message, Some(error.data)),
                };
                write_message(&mut output, &response)?;
            }
            Ok(ServerEvent::ProviderComplete(event)) => {
                let provider_quarantined = !event.release_gate;
                if event.release_gate {
                    provider_gate.release(event.generation);
                }
                if handle_provider_complete(
                    event.completion,
                    &mut mutation,
                    &provider_client,
                    &mut output,
                )? {
                    break;
                }
                if mutation_exits_after_provider_timeout(&mutation)
                    && (provider_quarantined || output_disconnected)
                {
                    break;
                }
            }
            Ok(ServerEvent::ProviderRecovered { generation }) => {
                provider_gate.release(generation);
                resume_mutation_after_provider_recovery(&mut mutation, &provider_client)
                    .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
            }
            Err(_) => break,
        }
        if output.has_error() && !output_disconnected {
            output_disconnected = true;
            stop_reader.store(true, Ordering::Release);
            cancel_and_join_waits(&mut active);
            receive_eof_and_maybe_end(&mut mutation, &provider_client)
                .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
            if matches!(mutation, MutationLifecycle::Dormant)
                || (mutation_exits_after_provider_timeout(&mutation)
                    && provider_gate.is_quarantined())
            {
                break;
            }
        }
    }
    if provider_gate.is_busy() {
        drop(provider_client);
        drop(provider_worker);
    } else {
        let _ = provider_client.sender.send(ProviderMessage::Shutdown);
        let _ = provider_worker.join();
    }
    // A generic blocking reader cannot be interrupted safely. Once its peer
    // output is gone, let the process return instead of waiting for unrelated
    // stdin EOF; the stop flag still lets a reader between frames retire.
    if !output_disconnected {
        let _ = reader.join();
    }
    if let Some(error) = input_error {
        Err(error)
    } else if let Some(error) = output.take_error() {
        Err(error)
    } else {
        Ok(())
    }
}

fn read_input<R: BufRead>(
    mut input: R,
    limit: usize,
    sender: mpsc::Sender<ServerEvent>,
    stop: &AtomicBool,
) {
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        match read_bounded_line(&mut input, limit) {
            Ok(line) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                let eof = matches!(line, BoundedLine::Eof);
                if sender.send(ServerEvent::Input(line)).is_err() || eof {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(ServerEvent::InputError(error));
                return;
            }
        }
    }
}

fn provider_loop<P: McpAcuProvider>(
    provider: Arc<P>,
    receiver: mpsc::Receiver<ProviderMessage>,
    sender: mpsc::Sender<ServerEvent>,
    gate: Arc<ProviderGate>,
) {
    while let Ok(message) = receiver.recv() {
        let ProviderMessage::Call(work) = message else {
            return;
        };
        let generation = work.generation;
        let timeout = work.timeout;
        let timeout_completion = failed_provider_call(&work.call, "acu_provider_timeout");
        let panic_completion = failed_provider_call(&work.call, "acu_provider_worker_panicked");
        let cancel = Arc::clone(&work.cancel);
        let (result_sender, result_receiver) = mpsc::channel();
        let helper = thread::spawn({
            let provider = Arc::clone(&provider);
            let cancel = Arc::clone(&cancel);
            move || {
                let completion = execute_provider_call(provider.as_ref(), work.call, &cancel);
                let _ = result_sender.send(completion);
            }
        });
        match result_receiver.recv_timeout(timeout) {
            Ok(completion) => {
                let _ = helper.join();
                if sender
                    .send(ServerEvent::ProviderComplete(ProviderCompletionEvent {
                        generation,
                        release_gate: true,
                        completion,
                    }))
                    .is_err()
                {
                    gate.release(generation);
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancel.store(true, Ordering::Release);
                gate.quarantine(generation);
                if sender
                    .send(ServerEvent::ProviderComplete(ProviderCompletionEvent {
                        generation,
                        release_gate: false,
                        completion: timeout_completion,
                    }))
                    .is_err()
                {
                    drop(helper);
                    return;
                }
                let recovered = sender.clone();
                thread::spawn(move || {
                    let _ = result_receiver.recv();
                    let _ = helper.join();
                    let _ = recovered.send(ServerEvent::ProviderRecovered { generation });
                });
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = helper.join();
                if sender
                    .send(ServerEvent::ProviderComplete(ProviderCompletionEvent {
                        generation,
                        release_gate: true,
                        completion: panic_completion,
                    }))
                    .is_err()
                {
                    gate.release(generation);
                    return;
                }
            }
        }
    }
}

fn execute_provider_call<P: McpAcuProvider>(
    provider: &P,
    call: ProviderCall,
    cancel: &AtomicBool,
) -> ProviderComplete {
    match call {
        ProviderCall::ReadOnly {
            response_id,
            request,
        } => ProviderComplete::ReadOnly {
            response_id,
            result: provider.call_controlled(&request, cancel),
        },
        ProviderCall::SessionStart => ProviderComplete::SessionStart(
            provider.call_controlled(
                &json!({
                    "acu_request": 1,
                    "kind": "command",
                    "command": {
                        "verb": "session-start",
                        "target": "current",
                        "label": "agenterm-mcp",
                        "ttl_seconds": MCP_MUTATION_SESSION_TTL_SECONDS
                    }
                })
                .to_string(),
                cancel,
            ),
        ),
        ProviderCall::ShellExec { id, request } => ProviderComplete::ShellExec {
            id,
            result: provider.call_controlled(&request, cancel),
        },
        ProviderCall::SessionEnd { request } => {
            ProviderComplete::SessionEnd(provider.call_controlled(&request, cancel))
        }
    }
}

fn failed_provider_call(call: &ProviderCall, code: &str) -> ProviderComplete {
    let error = Err(code.to_owned());
    match call {
        ProviderCall::ReadOnly { response_id, .. } => ProviderComplete::ReadOnly {
            response_id: response_id.clone(),
            result: error,
        },
        ProviderCall::SessionStart => ProviderComplete::SessionStart(error),
        ProviderCall::ShellExec { id, .. } => ProviderComplete::ShellExec {
            id: id.clone(),
            result: error,
        },
        ProviderCall::SessionEnd { .. } => ProviderComplete::SessionEnd(error),
    }
}

fn cancel_and_join_waits(active: &mut HashMap<String, ActiveWait>) {
    for wait in active.values() {
        let _ = wait.terminal.compare_exchange(
            WAIT_PENDING,
            WAIT_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        wait.cancelled.store(true, Ordering::Release);
    }
    for (_, wait) in active.drain() {
        let _ = wait.worker.join();
    }
}

fn is_shell_exec_tool_call(message: &Value) -> bool {
    message
        .as_object()
        .and_then(|object| object.get("method"))
        .and_then(Value::as_str)
        == Some("tools/call")
        && message["params"]["name"] == "agenterm_acu_shell_exec"
}

fn submit_shell_exec(
    message: &Value,
    lifecycle: &mut MutationLifecycle,
    provider: &ProviderClient,
) -> Result<(), Value> {
    let object = message.as_object().ok_or_else(|| {
        error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "Invalid JSON-RPC request",
            None,
        )
    })?;
    let id_value = object.get("id").cloned().ok_or_else(|| {
        error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "Mutating tool calls require an id",
            None,
        )
    })?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some(JSON_RPC_VERSION) {
        return Err(error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "Invalid JSON-RPC request",
            None,
        ));
    }
    let id = mutation_request_id(&id_value)
        .map_err(|message| error_response(id_value.clone(), ERROR_INVALID_PARAMS, message, None))?;
    let arguments = object
        .get("params")
        .and_then(Value::as_object)
        .and_then(|params| params.get("arguments"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            error_response(
                id_value.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_acu_shell_exec arguments must be an object",
                None,
            )
        })?;
    let allowed = [
        "idempotency_key",
        "command",
        "timeout_ms",
        "max_output_bytes",
    ];
    if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(error_response(
            id_value,
            ERROR_INVALID_PARAMS,
            "agenterm_acu_shell_exec contains an unknown argument",
            None,
        ));
    }
    let key = arguments
        .get("idempotency_key")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            error_response(
                id_value.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_acu_shell_exec requires idempotency_key",
                None,
            )
        })?;
    let key = IdempotencyKey::new(key).map_err(|_| {
        error_response(
            id_value.clone(),
            ERROR_INVALID_PARAMS,
            "agenterm_acu_shell_exec idempotency_key is invalid",
            None,
        )
    })?;
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 131_072 && !value.contains('\0'))
        .ok_or_else(|| {
            error_response(
                id_value.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_acu_shell_exec command is outside the published limit",
                None,
            )
        })?;
    let timeout_ms = arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .filter(|value| (100..=120_000).contains(value))
        .ok_or_else(|| {
            error_response(
                id_value.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_acu_shell_exec timeout_ms is outside the published limit",
                None,
            )
        })?;
    let max_output_bytes = arguments
        .get("max_output_bytes")
        .and_then(Value::as_u64)
        .filter(|value| (1..=16_777_216).contains(value))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            error_response(
                id_value.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_acu_shell_exec max_output_bytes is outside the published limit",
                None,
            )
        })?;
    let request = MutationRequest::new(
        id,
        key,
        ShellExecCommand {
            command: command.to_owned(),
            timeout_ms,
            max_output_bytes,
        },
    );
    match lifecycle {
        MutationLifecycle::Dormant => {
            *lifecycle = MutationLifecycle::Starting {
                first: Some(request),
                queued: None,
                eof: false,
            };
            if let Err(error) = provider.submit(ProviderCall::SessionStart) {
                *lifecycle = MutationLifecycle::Dormant;
                return Err(provider_submit_error(id_value, error));
            }
            Ok(())
        }
        MutationLifecycle::Starting { first, queued, .. } => {
            let duplicate_id = first
                .as_ref()
                .is_some_and(|active| active.json_rpc_id() == request.json_rpc_id())
                || queued
                    .as_ref()
                    .is_some_and(|active| active.json_rpc_id() == request.json_rpc_id());
            let duplicate_key = first
                .as_ref()
                .is_some_and(|active| active.idempotency_key() == request.idempotency_key())
                || queued
                    .as_ref()
                    .is_some_and(|active| active.idempotency_key() == request.idempotency_key());
            if duplicate_id {
                Err(error_response(
                    id_value,
                    ERROR_INVALID_REQUEST,
                    "A request with this id is already active",
                    None,
                ))
            } else if duplicate_key {
                Err(error_response(
                    id_value,
                    ERROR_INVALID_PARAMS,
                    "The idempotency_key is already active",
                    None,
                ))
            } else if queued.is_some() {
                Err(error_response(
                    id_value,
                    -32003,
                    "MCP mutation capacity is exhausted while the session starts",
                    Some(json!({"maximum_dispatched": 1, "maximum_queued": 1})),
                ))
            } else {
                *queued = Some(request);
                Ok(())
            }
        }
        MutationLifecycle::Active { state, .. } if provider.gate.is_quarantined() => {
            Err(provider_submit_error(id_value, ProviderSubmitError::Busy))
        }
        MutationLifecycle::Active { state, .. } => match state.submit(request) {
            SubmitResult::Queued => {
                dispatch_next(state, provider)?;
                Ok(())
            }
            SubmitResult::Busy => Err(error_response(
                id_value,
                -32003,
                "MCP mutation capacity is exhausted",
                Some(json!({"maximum_dispatched": 1, "maximum_queued": 1})),
            )),
            SubmitResult::DuplicateJsonRpcId => Err(error_response(
                id_value,
                ERROR_INVALID_REQUEST,
                "A request with this id is already active",
                None,
            )),
            SubmitResult::DuplicateIdempotencyKey => Err(error_response(
                id_value,
                ERROR_INVALID_PARAMS,
                "The idempotency_key is already active",
                None,
            )),
            SubmitResult::RejectedAfterEof => Err(error_response(
                id_value,
                ERROR_INVALID_REQUEST,
                "The MCP input is closed",
                None,
            )),
        },
    }
}

fn dispatch_next(
    state: &mut ConnectionMutationState<ShellExecCommand>,
    provider: &ProviderClient,
) -> Result<(), Value> {
    if provider.gate.is_busy() {
        return Ok(());
    }
    let Some(dispatch) = state.begin_dispatch() else {
        return Ok(());
    };
    let command = dispatch.command();
    let request = dispatch.with_session_lease(|lease| {
        json!({
            "acu_request": 1,
            "kind": "identity_bound_command",
            "command": {
                "verb": "shell-exec",
                "target": "current",
                "command": command.command,
                "timeout_ms": command.timeout_ms,
                "max_output_bytes": command.max_output_bytes
            },
            "request_identity": {
                "request_id": dispatch.idempotency_key().as_str(),
                "session_id": dispatch.session_id(),
                "session_lease": String::from_utf8_lossy(lease)
            }
        })
        .to_string()
    });
    provider
        .submit_with_timeout(
            ProviderCall::ShellExec {
                id: dispatch.json_rpc_id().clone(),
                request,
            },
            Duration::from_millis(command.timeout_ms).saturating_add(MCP_PROVIDER_SHELL_GRACE),
        )
        .map_err(|error| provider_submit_error(mutation_id_value(dispatch.json_rpc_id()), error))
}

fn dispatch_session_end(
    state: &mut ConnectionMutationState<ShellExecCommand>,
    provider: &ProviderClient,
) -> Result<(), Value> {
    let end = state.begin_session_end().map_err(|_| {
        error_response(
            Value::Null,
            ERROR_ACU_PROVIDER,
            "agenterm-cu session end was not ready",
            None,
        )
    })?;
    let request = end.with_session_lease(|lease| {
        json!({
            "acu_request": 1,
            "kind": "command",
            "command": {
                "verb": "session-end",
                "target": "current",
                "session_id": end.session_id(),
                "lease": String::from_utf8_lossy(lease),
                "confirm": true
            }
        })
        .to_string()
    });
    provider
        .submit(ProviderCall::SessionEnd { request })
        .map_err(|error| provider_submit_error(Value::Null, error))
}

fn receive_eof_mutation(lifecycle: &mut MutationLifecycle) {
    match lifecycle {
        MutationLifecycle::Dormant => {}
        MutationLifecycle::Starting { first, queued, eof } => {
            first.take();
            queued.take();
            *eof = true;
        }
        MutationLifecycle::Active {
            state,
            exit_after_end,
        } => {
            let eof = state.receive_eof();
            let _ = (eof.cancelled_queued, eof.wait_for_dispatched);
            *exit_after_end = true;
        }
    }
}

fn receive_eof_and_maybe_end(
    lifecycle: &mut MutationLifecycle,
    provider: &ProviderClient,
) -> Result<(), Value> {
    receive_eof_mutation(lifecycle);
    if let MutationLifecycle::Active { state, .. } = lifecycle
        && state.is_session_ending()
        && !provider.gate.is_busy()
    {
        dispatch_session_end(state, provider)?;
    }
    Ok(())
}

fn mutation_exits_after_provider_timeout(lifecycle: &MutationLifecycle) -> bool {
    matches!(
        lifecycle,
        MutationLifecycle::Active {
            exit_after_end: true,
            ..
        }
    )
}

fn resume_mutation_after_provider_recovery(
    lifecycle: &mut MutationLifecycle,
    provider: &ProviderClient,
) -> Result<(), Value> {
    let MutationLifecycle::Active { state, .. } = lifecycle else {
        return Ok(());
    };
    if state.is_session_ending() {
        dispatch_session_end(state, provider)
    } else {
        dispatch_next(state, provider)
    }
}

fn cancellation_reply(id: Value) -> Value {
    acu_tool_response(
        id,
        json!({
            "ok": false,
            "target": "current",
            "command": "shell-exec",
            "error": {
                "code": "mcp_request_cancelled",
                "message": "The mutating request was cancelled before provider dispatch",
                "detail": {
                    "provider_calls": 0,
                    "reservations": 0,
                    "effects": 0
                }
            }
        }),
    )
}

fn handle_mutation_cancel<W: Write>(
    message: &Value,
    lifecycle: &mut MutationLifecycle,
    output: &mut W,
) -> io::Result<()> {
    let Some(request_id) = message["params"].get("requestId") else {
        return Ok(());
    };
    let Ok(id) = mutation_request_id(request_id) else {
        return Ok(());
    };
    match lifecycle {
        MutationLifecycle::Dormant => {}
        MutationLifecycle::Starting { first, queued, eof } => {
            if first
                .as_ref()
                .is_some_and(|request| request.json_rpc_id() == &id)
            {
                first.take();
                if !*eof {
                    write_message(output, &cancellation_reply(request_id.clone()))?;
                }
            } else if queued
                .as_ref()
                .is_some_and(|request| request.json_rpc_id() == &id)
            {
                queued.take();
                if !*eof {
                    write_message(output, &cancellation_reply(request_id.clone()))?;
                }
            }
        }
        MutationLifecycle::Active { state, .. } => {
            if let CancelResult::QueuedCancelled(_) = state.cancel(&id) {
                write_message(output, &cancellation_reply(request_id.clone()))?;
            }
        }
    }
    Ok(())
}

fn parse_provider_reply(result: Result<String, String>) -> Result<Value, String> {
    let encoded = result?;
    let reply: Value =
        serde_json::from_str(&encoded).map_err(|_| "acu_provider_reply_not_json".to_owned())?;
    let Some(object) = reply.as_object() else {
        return Err("acu_provider_reply_invalid_shape".to_owned());
    };
    if !object.get("ok").is_some_and(Value::is_boolean)
        || !object.get("target").is_some_and(Value::is_string)
        || !object.get("command").is_some_and(Value::is_string)
    {
        return Err("acu_provider_reply_invalid_shape".to_owned());
    }
    if object.get("ok").and_then(Value::as_bool) == Some(false)
        && !object
            .get("error")
            .and_then(Value::as_object)
            .is_some_and(|error| {
                error.get("code").is_some_and(Value::is_string)
                    && error.get("message").is_some_and(Value::is_string)
            })
    {
        return Err("acu_provider_reply_invalid_shape".to_owned());
    }
    Ok(reply)
}

fn provider_submit_error(id: Value, error: ProviderSubmitError) -> Value {
    let (message, code) = match error {
        ProviderSubmitError::Busy => (
            "agenterm-cu provider is still completing an earlier call",
            "acu_provider_busy",
        ),
        ProviderSubmitError::Stopped => (
            "agenterm-cu provider worker stopped",
            "acu_provider_worker_stopped",
        ),
    };
    error_response(id, ERROR_ACU_PROVIDER, message, Some(json!({"code": code})))
}

fn provider_boundary_reply(code: &str) -> Value {
    json!({
        "ok": false,
        "target": "current",
        "command": "shell-exec",
        "error": {
            "code": "outcome_unknown",
            "message": "The provider outcome is unknown after dispatch",
            "detail": {"provider_code": code}
        }
    })
}

fn acu_tool_response(id: Value, reply: Value) -> Value {
    let is_error = reply.get("ok").and_then(Value::as_bool) != Some(true);
    success_response(
        id,
        json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&reply).expect("ACU reply serializes")
            }],
            "structuredContent": reply,
            "isError": is_error
        }),
    )
}

fn handle_provider_complete<W: Write>(
    completion: ProviderComplete,
    lifecycle: &mut MutationLifecycle,
    provider: &ProviderClient,
    output: &mut CleanupWriter<W>,
) -> io::Result<bool> {
    match completion {
        ProviderComplete::ReadOnly {
            response_id,
            result,
        } => {
            write_message(output, &complete_acu_tool_call(response_id, result))?;
            if !provider.gate.is_busy()
                && (!output.has_error() || mutation_exits_after_provider_timeout(lifecycle))
            {
                resume_mutation_after_provider_recovery(lifecycle, provider)
                    .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
            }
        }
        ProviderComplete::SessionStart(result) => {
            let MutationLifecycle::Starting {
                mut first,
                mut queued,
                eof,
            } = std::mem::replace(lifecycle, MutationLifecycle::Dormant)
            else {
                return Ok(false);
            };
            let parsed = parse_provider_reply(result);
            let session = parsed.as_ref().ok().and_then(|reply| {
                (reply["ok"] == true).then(|| {
                    (
                        reply["data"]["session_id"].as_str(),
                        reply["data"]["lease"].as_str(),
                    )
                })
            });
            let Some((Some(session_id), Some(lease))) = session else {
                if !eof {
                    let reply = match parsed {
                        Ok(reply) if reply["ok"] == false => reply,
                        Ok(_) => provider_boundary_reply("acu_provider_session_identity_invalid"),
                        Err(code) => provider_boundary_reply(&code),
                    };
                    for request in first.into_iter().chain(queued) {
                        write_message(
                            output,
                            &acu_tool_response(
                                mutation_id_value(request.json_rpc_id()),
                                reply.clone(),
                            ),
                        )?;
                    }
                }
                return Ok(eof);
            };
            let mut state = match ConnectionMutationState::new(
                session_id.to_owned(),
                lease.as_bytes().to_vec(),
            ) {
                Ok(state) => state,
                Err(_) => {
                    if !eof {
                        for request in first.into_iter().chain(queued) {
                            write_message(
                                output,
                                &acu_tool_response(
                                    mutation_id_value(request.json_rpc_id()),
                                    provider_boundary_reply(
                                        "acu_provider_session_identity_invalid",
                                    ),
                                ),
                            )?;
                        }
                    }
                    return Ok(eof);
                }
            };
            if !eof && (first.is_some() || queued.is_some()) {
                if first.is_none() {
                    first = queued.take();
                }
                if let Some(request) = first {
                    debug_assert_eq!(state.submit(request), SubmitResult::Queued);
                    dispatch_next(&mut state, provider)
                        .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
                }
                if let Some(request) = queued {
                    debug_assert_eq!(state.submit(request), SubmitResult::Queued);
                }
            } else {
                let eof = state.receive_eof();
                let _ = (eof.cancelled_queued, eof.wait_for_dispatched);
                dispatch_session_end(&mut state, provider)
                    .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
            }
            *lifecycle = MutationLifecycle::Active {
                state,
                exit_after_end: eof,
            };
        }
        ProviderComplete::ShellExec { id, result } => {
            let MutationLifecycle::Active { state, .. } = lifecycle else {
                return Ok(false);
            };
            let completion = match parse_provider_reply(result) {
                Ok(reply) => ProviderCompletion::Authoritative {
                    reply,
                    reservation_created: true,
                    effect_attempted: true,
                },
                Err(_) => ProviderCompletion::LostAfterDispatch,
            };
            let disposition = state.complete(&id, completion).map_err(|_| {
                io::Error::other("provider completion did not match the dispatched MCP request")
            })?;
            match disposition {
                CompletionDisposition::Emit(completed) => {
                    let _ = (&completed.idempotency_key, completed.cancellation_requested);
                    let reply = match completed.outcome {
                        CompletionOutcome::Authoritative { reply, counts } => {
                            let _ = counts;
                            reply
                        }
                        CompletionOutcome::OutcomeUnknown { counts } => {
                            let _ = counts;
                            provider_boundary_reply("acu_provider_boundary_lost_after_dispatch")
                        }
                    };
                    write_message(
                        output,
                        &acu_tool_response(mutation_id_value(&completed.json_rpc_id), reply),
                    )?;
                    if !output.has_error() && !provider.gate.is_busy() {
                        dispatch_next(state, provider)
                            .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
                    }
                }
                CompletionDisposition::SuppressAfterEof(_) => {}
            }
            if state.is_session_ending() && !provider.gate.is_busy() {
                dispatch_session_end(state, provider)
                    .map_err(|_| io::Error::other("agenterm-cu provider worker stopped"))?;
            }
        }
        ProviderComplete::SessionEnd(result) => {
            let MutationLifecycle::Active {
                state,
                exit_after_end,
            } = lifecycle
            else {
                return Ok(false);
            };
            let should_exit = *exit_after_end;
            let completion = match parse_provider_reply(result) {
                Ok(reply) if reply["ok"] == true => SessionEndCompletion::Authoritative,
                _ => SessionEndCompletion::LostAfterDispatch,
            };
            state.finish_session_end(completion).map_err(|_| {
                io::Error::other("provider session-end completion was not expected")
            })?;
            if !should_exit {
                *lifecycle = MutationLifecycle::Dormant;
            }
            return Ok(should_exit);
        }
    }
    Ok(false)
}

fn mutation_request_id(value: &Value) -> Result<JsonRpcRequestId, &'static str> {
    if let Some(value) = value.as_str() {
        return JsonRpcRequestId::text(value).map_err(|_| "JSON-RPC id is too long");
    }
    if let Some(value) = value.as_i64() {
        return Ok(JsonRpcRequestId::Integer(value));
    }
    if let Some(value) = value.as_u64() {
        return Ok(JsonRpcRequestId::Unsigned(value));
    }
    Err("JSON-RPC id must be a string or integer")
}

fn mutation_id_value(id: &JsonRpcRequestId) -> Value {
    match id {
        JsonRpcRequestId::Integer(value) => json!(value),
        JsonRpcRequestId::Unsigned(value) => json!(value),
        JsonRpcRequestId::Text(value) => json!(value),
    }
}

fn decode_line(line: &[u8]) -> Result<Value, Value> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Err(error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "MCP message must not be empty",
            None,
        ));
    }
    serde_json::from_slice::<Value>(line).map_err(|error| {
        error_response(
            Value::Null,
            ERROR_PARSE,
            "Parse error",
            Some(json!({"detail": bounded_detail(&error.to_string())})),
        )
    })
}

fn handle_cancel_notification(
    message: &Value,
    active: &HashMap<String, ActiveWait>,
    provider: &ProviderGate,
) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };
    if object.get("method").and_then(Value::as_str) != Some("notifications/cancelled")
        || object.get("id").is_some()
    {
        return false;
    }
    if let Some(request_id) = object
        .get("params")
        .and_then(Value::as_object)
        .and_then(|params| params.get("requestId"))
        .filter(|id| valid_request_id(id))
        && let Some(wait) = active.get(&request_id_key(request_id))
        && wait
            .terminal
            .compare_exchange(
                WAIT_PENDING,
                WAIT_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    {
        wait.cancelled.store(true, Ordering::Release);
    }
    if let Some(request_id) = object
        .get("params")
        .and_then(Value::as_object)
        .and_then(|params| params.get("requestId"))
        .filter(|id| valid_request_id(id))
    {
        provider.cancel_read(request_id);
    }
    true
}

fn is_wait_tool_call(message: &Value) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };
    object.get("method").and_then(Value::as_str) == Some("tools/call")
        && object
            .get("params")
            .and_then(Value::as_object)
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            == Some("agenterm_wait")
}

fn start_wait(
    message: &Value,
    config: &McpStdioConfig,
    sender: &mpsc::Sender<ServerEvent>,
    active: &mut HashMap<String, ActiveWait>,
    maximum_waiters: usize,
) -> Result<(), Value> {
    let object = message.as_object().ok_or_else(|| {
        error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "Invalid JSON-RPC request",
            None,
        )
    })?;
    let Some(id) = object.get("id").cloned() else {
        return Ok(());
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some(JSON_RPC_VERSION)
        || !valid_request_id(&id)
    {
        return Err(error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "Invalid JSON-RPC request",
            None,
        ));
    }
    let key = request_id_key(&id);
    if active.contains_key(&key) {
        return Err(error_response(
            id,
            ERROR_INVALID_REQUEST,
            "A request with this id is already active",
            None,
        ));
    }
    if active.len() >= maximum_waiters {
        return Err(error_response(
            id,
            -32003,
            "MCP waiter capacity is exhausted",
            Some(json!({"maximum_waiters": maximum_waiters})),
        ));
    }
    let Some(params) = object.get("params").and_then(Value::as_object) else {
        return Err(error_response(
            id,
            ERROR_INVALID_PARAMS,
            "tools/call params must be an object",
            None,
        ));
    };
    if params.get("name").and_then(Value::as_str) != Some("agenterm_wait") {
        return Err(error_response(
            id,
            ERROR_INVALID_PARAMS,
            "Unknown tool",
            Some(json!({"name": params.get("name")})),
        ));
    }
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            error_response(
                id.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_wait arguments must be an object",
                None,
            )
        })?;
    let allowed = [
        "epoch",
        "after_sequence",
        "event_kind",
        "tab_id",
        "timeout_ms",
    ];
    if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(error_response(
            id,
            ERROR_INVALID_PARAMS,
            "agenterm_wait contains an unknown argument",
            None,
        ));
    }
    let epoch = arguments
        .get("epoch")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| {
            error_response(
                id.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_wait requires a bounded epoch",
                None,
            )
        })?;
    let after_sequence = arguments
        .get("after_sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            error_response(
                id.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_wait requires after_sequence",
                None,
            )
        })?;
    let event_kind = arguments
        .get("event_kind")
        .and_then(Value::as_str)
        .filter(|kind| mcp_fleet::WAIT_EVENT_KINDS.contains(kind))
        .ok_or_else(|| {
            error_response(
                id.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_wait event_kind is not allowlisted",
                Some(json!({"allowed": mcp_fleet::WAIT_EVENT_KINDS})),
            )
        })?;
    let tab_id = match arguments.get("tab_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if valid_tab_id(value) => Some(value.clone()),
        _ => {
            return Err(error_response(
                id,
                ERROR_INVALID_PARAMS,
                "agenterm_wait tab_id must be a stable @ID",
                None,
            ));
        }
    };
    let timeout_ms = arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .filter(|value| {
            *value >= 1 && *value <= u64::from(capabilities().limits.wait_timeout_ms_maximum)
        })
        .ok_or_else(|| {
            error_response(
                id.clone(),
                ERROR_INVALID_PARAMS,
                "agenterm_wait timeout_ms is outside the published limit",
                None,
            )
        })?;
    let request = mcp_fleet::McpWaitRequest {
        epoch: epoch.to_owned(),
        after_sequence,
        event_kind: event_kind.to_owned(),
        tab_id,
        timeout_ms,
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let terminal = Arc::new(AtomicU8::new(WAIT_PENDING));
    let worker_cancelled = Arc::clone(&cancelled);
    let worker_terminal = Arc::clone(&terminal);
    let worker_sender = sender.clone();
    let worker_key = key.clone();
    let worker_id = id.clone();
    let address = config.address.clone();
    let cancellation_request = request.clone();
    let worker = thread::spawn(move || {
        let mut result = mcp_fleet::wait_event(address.as_deref(), request, worker_cancelled);
        match worker_terminal.compare_exchange(
            WAIT_PENDING,
            WAIT_COMPLETED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(WAIT_CANCELLED) => {
                result = Ok(mcp_fleet::cancelled_wait_outcome(&cancellation_request));
            }
            Err(_) => return,
        }
        let _ = worker_sender.send(ServerEvent::WaitComplete {
            key: worker_key,
            id: worker_id,
            result,
        });
    });
    active.insert(
        key,
        ActiveWait {
            cancelled,
            terminal,
            worker,
        },
    );
    Ok(())
}

fn request_id_key(id: &Value) -> String {
    serde_json::to_string(id).expect("valid request id serializes")
}

fn valid_tab_id(value: &str) -> bool {
    value.strip_prefix('@').is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn process_message(
    message: Value,
    state: &mut SessionState,
    config: &McpStdioConfig,
    mutation_enabled: bool,
    provider: &ProviderClient,
) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(error_response(
            Value::Null,
            ERROR_INVALID_REQUEST,
            "JSON-RPC message must be an object",
            None,
        ));
    };
    let id = object.get("id").cloned();
    let notification = id.is_none();
    let response_id = id.clone().filter(valid_request_id).unwrap_or(Value::Null);
    if object.get("jsonrpc").and_then(Value::as_str) != Some(JSON_RPC_VERSION)
        || object.get("method").and_then(Value::as_str).is_none()
        || id.as_ref().is_some_and(|id| !valid_request_id(id))
    {
        return (!notification).then(|| {
            error_response(
                response_id,
                ERROR_INVALID_REQUEST,
                "Invalid JSON-RPC request",
                None,
            )
        });
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .expect("validated method");
    let params = object.get("params");

    match method {
        "initialize" if notification => None,
        "initialize" if *state != SessionState::New => Some(error_response(
            response_id,
            ERROR_INVALID_REQUEST,
            "MCP session is already initialized",
            None,
        )),
        "initialize" => {
            let Some(params) = params.and_then(Value::as_object) else {
                return Some(error_response(
                    response_id,
                    ERROR_INVALID_PARAMS,
                    "initialize params must be an object",
                    None,
                ));
            };
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            let capabilities_valid = params.get("capabilities").is_some_and(Value::is_object);
            let client_valid = params
                .get("clientInfo")
                .and_then(Value::as_object)
                .is_some_and(|client| {
                    client.get("name").is_some_and(Value::is_string)
                        && client.get("version").is_some_and(Value::is_string)
                });
            if requested.is_none() || !capabilities_valid || !client_valid {
                return Some(error_response(
                    response_id,
                    ERROR_INVALID_PARAMS,
                    "initialize requires protocolVersion, capabilities, and clientInfo",
                    Some(json!({"supported": [MCP_PROTOCOL_REVISION]})),
                ));
            }
            if requested != Some(MCP_PROTOCOL_REVISION) {
                return Some(error_response(
                    response_id,
                    ERROR_PROTOCOL_VERSION,
                    "Unsupported MCP protocol version",
                    Some(json!({
                        "code": "mcp_protocol_version",
                        "requested": requested,
                        "supported": [MCP_PROTOCOL_REVISION]
                    })),
                ));
            }
            *state = SessionState::InitializeResponded;
            Some(success_response(
                response_id,
                json!({
                    "protocolVersion": MCP_PROTOCOL_REVISION,
                    "capabilities": {
                        "resources": {
                            "subscribe": false,
                            "listChanged": false
                        },
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "agenterm-mcp",
                        "title": "AgenTerm MCP",
                        "version": env!("CARGO_PKG_VERSION"),
                        "description": "AgenTerm Fleet and bounded agenterm-cu bridge"
                    },
                    "instructions": if mutation_enabled {
                        "Read metadata-safe Fleet resources, wait for one bounded Fleet event, inspect agenterm-cu, or exercise the internal bounded mutation court."
                    } else {
                        "Read metadata-safe Fleet resources, wait for one bounded Fleet event, or inspect agenterm-cu. Mutation tools remain unavailable."
                    }
                }),
            ))
        }
        "notifications/initialized" if !notification => Some(error_response(
            response_id,
            ERROR_INVALID_REQUEST,
            "notifications/initialized must not contain an id",
            None,
        )),
        "notifications/initialized" if *state == SessionState::InitializeResponded => {
            *state = SessionState::Ready;
            None
        }
        "notifications/initialized" => None,
        "ping" if notification => None,
        "ping" => Some(success_response(response_id, json!({}))),
        _ if notification => None,
        _ if *state != SessionState::Ready => Some(error_response(
            response_id,
            ERROR_NOT_INITIALIZED,
            "MCP session is not initialized",
            None,
        )),
        "resources/list" => Some(success_response(
            response_id,
            json!({
                "resources": capabilities()
                    .resources
                    .into_iter()
                    .map(|resource| json!({
                        "uri": resource.uri,
                        "name": resource.stable_id,
                        "title": resource_title(resource.stable_id),
                        "description": resource_description(resource.stable_id),
                        "mimeType": "application/json"
                    }))
                    .collect::<Vec<_>>()
            }),
        )),
        "resources/read" => {
            let Some(uri) = params
                .and_then(Value::as_object)
                .and_then(|params| params.get("uri"))
                .and_then(Value::as_str)
            else {
                return Some(error_response(
                    response_id,
                    ERROR_INVALID_PARAMS,
                    "resources/read requires a string uri",
                    None,
                ));
            };
            match mcp_fleet::read_resource(uri, config.address.as_deref()) {
                Ok(resource) => Some(success_response(
                    response_id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": serde_json::to_string(&resource)
                                .expect("resource projection serializes")
                        }]
                    }),
                )),
                Err(error) => Some(error_response(
                    response_id,
                    error.code,
                    error.message,
                    Some(error.data),
                )),
            }
        }
        "tools/list" => {
            let mut tools = vec![
                json!({
                    "name": "agenterm_wait",
                    "title": "Wait for an AgenTerm Fleet event",
                    "description": "Read-only bounded wait from a verified epoch and sequence.",
                    "inputSchema": {
                        "$schema": "https://json-schema.org/draft/2020-12/schema",
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "epoch": {"type": "string", "minLength": 1, "maxLength": 256},
                            "after_sequence": {"type": "integer", "minimum": 0},
                            "event_kind": {
                                "type": "string",
                                "enum": mcp_fleet::WAIT_EVENT_KINDS
                            },
                            "tab_id": {"type": ["string", "null"], "pattern": "^@[0-9]+$"},
                            "timeout_ms": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": capabilities().limits.wait_timeout_ms_maximum
                            }
                        },
                        "required": ["epoch", "after_sequence", "event_kind", "timeout_ms"]
                    },
                    "outputSchema": {
                        "$schema": "https://json-schema.org/draft/2020-12/schema",
                        "type": "object",
                        "properties": {
                            "schema_id": {"type": "string"},
                            "outcome": {
                                "type": "string",
                                "enum": [
                                    "matched", "timeout", "cancelled", "server_restart",
                                    "journal_gap", "future_sequence", "target_closed",
                                    "event_read_failed"
                                ]
                            },
                            "position": {"type": "object"}
                        },
                        "required": ["schema_id", "outcome", "position"]
                    },
                    "annotations": {
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                }),
                acu_tool_descriptor(include_str!(
                    "../crates/agenterm-cu/contract/mcp-capabilities-tool.json"
                )),
                acu_tool_descriptor(include_str!(
                    "../crates/agenterm-cu/contract/mcp-observe-tool.json"
                )),
            ];
            if mutation_enabled {
                tools.push(acu_tool_descriptor(include_str!(
                    "../crates/agenterm-cu/contract/mcp-shell-exec-tool.json"
                )));
            }
            Some(success_response(response_id, json!({"tools": tools})))
        }
        "tools/call" if notification => None,
        "tools/call" => submit_acu_tool_call(response_id, params, provider).err(),
        _ => Some(error_response(
            response_id,
            ERROR_METHOD_NOT_FOUND,
            "Method not found",
            Some(json!({"method": method})),
        )),
    }
}

fn acu_tool_descriptor(source: &str) -> Value {
    serde_json::from_str(source).expect("agenterm-cu-owned MCP descriptor must be valid JSON")
}

fn submit_acu_tool_call(
    response_id: Value,
    params: Option<&Value>,
    provider: &ProviderClient,
) -> Result<(), Value> {
    let Some(params) = params.and_then(Value::as_object) else {
        return Err(error_response(
            response_id,
            ERROR_INVALID_PARAMS,
            "tools/call params must be an object",
            None,
        ));
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(error_response(
            response_id,
            ERROR_INVALID_PARAMS,
            "tools/call requires a tool name",
            None,
        ));
    };
    if !matches!(name, "agenterm_acu_capabilities" | "agenterm_acu_observe") {
        return Err(error_response(
            response_id,
            ERROR_INVALID_PARAMS,
            "Unknown tool",
            Some(json!({"name": name})),
        ));
    }
    let Some(arguments) = params.get("arguments").and_then(Value::as_object) else {
        return Err(error_response(
            response_id,
            ERROR_INVALID_PARAMS,
            &format!("{name} arguments must be an object"),
            None,
        ));
    };
    let request = json!({
        "acu_request": 1,
        "kind": "mcp_call",
        "name": name,
        "arguments": arguments
    });
    let encoded = serde_json::to_string(&request).expect("ACU MCP request serializes");
    provider
        .submit(ProviderCall::ReadOnly {
            response_id: response_id.clone(),
            request: encoded,
        })
        .map_err(|error| provider_submit_error(response_id, error))
}

fn complete_acu_tool_call(response_id: Value, result: Result<String, String>) -> Value {
    let reply = match result {
        Ok(reply) => match serde_json::from_str::<Value>(&reply) {
            Ok(reply) => reply,
            Err(_) => {
                return error_response(
                    response_id,
                    ERROR_ACU_PROVIDER,
                    "agenterm-cu provider returned an invalid reply",
                    Some(json!({"code": "acu_provider_reply_not_json"})),
                );
            }
        },
        Err(code) => {
            let message = if code == "acu_provider_timeout" {
                "agenterm-cu provider call exceeded its MCP deadline"
            } else {
                "agenterm-cu provider boundary failed"
            };
            return error_response(
                response_id,
                ERROR_ACU_PROVIDER,
                message,
                Some(json!({
                    "code": code,
                    "outcome": if code == "acu_provider_timeout" {
                        "unknown"
                    } else {
                        "failed"
                    }
                })),
            );
        }
    };
    let is_error = reply.get("ok").and_then(Value::as_bool) != Some(true);
    success_response(
        response_id,
        json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&reply).expect("ACU reply serializes")
            }],
            "structuredContent": reply,
            "isError": is_error
        }),
    )
}

fn resource_title(stable_id: &str) -> &'static str {
    match stable_id {
        "fleet.instances" => "AgenTerm Instances",
        "fleet.workspace" => "AgenTerm Workspace",
        "fleet.tabs" => "AgenTerm Tabs",
        "fleet.snapshot" => "AgenTerm Fleet Snapshot",
        _ => "AgenTerm Resource",
    }
}

fn resource_description(stable_id: &str) -> &'static str {
    match stable_id {
        "fleet.instances" => "Registered local AgenTerm server metadata",
        "fleet.workspace" => "Selected workspace identity and event baseline",
        "fleet.tabs" => "Metadata-only stable tab inventory",
        "fleet.snapshot" => "One causal metadata-only Fleet snapshot",
        _ => "AgenTerm metadata",
    }
}

fn valid_request_id(id: &Value) -> bool {
    id.is_string() || id.is_i64() || id.is_u64()
}

fn success_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": JSON_RPC_VERSION, "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = serde_json::Map::from_iter([
        ("code".to_owned(), Value::from(code)),
        ("message".to_owned(), Value::from(message)),
    ]);
    if let Some(data) = data {
        error.insert("data".to_owned(), data);
    }
    json!({"jsonrpc": JSON_RPC_VERSION, "id": id, "error": error})
}

fn bounded_detail(detail: &str) -> String {
    let maximum = capabilities().limits.error_detail_bytes as usize;
    detail.chars().take(maximum).collect()
}

fn write_message<W: Write>(output: &mut W, message: &Value) -> io::Result<()> {
    let maximum = capabilities().limits.response_bytes as usize;
    let mut encoded = serde_json::to_vec(message).map_err(io::Error::other)?;
    if encoded.len() > maximum {
        encoded = serde_json::to_vec(&error_response(
            Value::Null,
            ERROR_RESPONSE_TOO_LARGE,
            "MCP response exceeds the byte limit",
            Some(json!({
                "class": "response_too_large",
                "code": "agenterm_response_too_large",
                "maximum_bytes": maximum
            })),
        ))
        .map_err(io::Error::other)?;
        if encoded.len() > maximum {
            return Err(io::Error::other(
                "bounded MCP response exceeds its own published limit",
            ));
        }
    }
    output.write_all(&encoded)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn read_bounded_line<R: BufRead>(input: &mut R, maximum: usize) -> io::Result<BoundedLine> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() && !oversized {
                Ok(BoundedLine::Eof)
            } else if oversized {
                Ok(BoundedLine::Oversized)
            } else {
                Ok(BoundedLine::Line(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if !oversized {
            let payload = if newline.is_some() {
                &available[..consumed - 1]
            } else {
                &available[..consumed]
            };
            if line.len().saturating_add(payload.len()) > maximum {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(payload);
            }
        }
        input.consume(consumed);
        if newline.is_some() {
            return if oversized {
                Ok(BoundedLine::Oversized)
            } else {
                Ok(BoundedLine::Line(line))
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufReader, Cursor, Read},
        sync::{Condvar, Mutex},
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn provider_gate_ignores_stale_generation_release() {
        let gate = ProviderGate::new();
        let first = gate.acquire().expect("first provider call acquires gate");
        gate.quarantine(first);
        assert!(gate.is_quarantined());
        gate.release(first);
        let second = gate.acquire().expect("second provider call acquires gate");

        gate.release(first);
        assert!(
            gate.is_busy(),
            "stale completion must not release newer work"
        );
        assert!(!gate.is_quarantined());

        gate.release(second);
        assert!(!gate.is_busy());
    }

    fn exchange(input: &str) -> Vec<Value> {
        let mut output = Vec::new();
        serve_stdio(
            BufReader::new(Cursor::new(input.as_bytes().to_vec())),
            &mut output,
        )
        .unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    struct ChannelInput {
        receiver: mpsc::Receiver<Vec<u8>>,
        buffer: Vec<u8>,
        offset: usize,
        exit: Arc<DisconnectState>,
    }

    impl Read for ChannelInput {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let count = available.len().min(output.len());
            output[..count].copy_from_slice(&available[..count]);
            self.consume(count);
            Ok(count)
        }
    }

    impl BufRead for ChannelInput {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.offset == self.buffer.len() {
                self.buffer = self.receiver.recv().unwrap_or_default();
                self.offset = 0;
            }
            Ok(&self.buffer[self.offset..])
        }

        fn consume(&mut self, amount: usize) {
            self.offset = (self.offset + amount).min(self.buffer.len());
        }
    }

    impl Drop for ChannelInput {
        fn drop(&mut self) {
            *self.exit.reader_exited.lock().expect("reader exit lock") = true;
            self.exit.changed.notify_all();
        }
    }

    #[derive(Default)]
    struct DisconnectState {
        disconnected: AtomicBool,
        observed: Mutex<bool>,
        reader_exited: Mutex<bool>,
        changed: Condvar,
    }

    struct ControlledOutput(Arc<DisconnectState>);

    impl Write for ControlledOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.disconnected.load(Ordering::Acquire) {
                *self.0.observed.lock().expect("disconnect lock") = true;
                self.0.changed.notify_all();
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fixture stdout disconnected",
                ))
            } else {
                Ok(bytes.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct InterruptOnceOutput {
        write_interrupted: bool,
        flush_interrupted: bool,
        bytes: Vec<u8>,
    }

    impl Write for InterruptOnceOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.write_interrupted {
                self.write_interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if !self.flush_interrupted {
                self.flush_interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            Ok(())
        }
    }

    #[test]
    fn cleanup_writer_retries_interrupted_without_entering_disconnect_teardown() {
        let mut output = CleanupWriter::new(InterruptOnceOutput::default());
        output.write_all(b"reply").unwrap();
        output.flush().unwrap();
        assert!(!output.has_error());
        assert_eq!(output.inner.bytes, b"reply");
    }

    impl DisconnectState {
        fn wait_until_observed(&self) {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut observed = self.observed.lock().expect("disconnect lock");
            while !*observed {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "stdout failure was not observed");
                (observed, _) = self
                    .changed
                    .wait_timeout(observed, remaining)
                    .expect("disconnect wait");
            }
        }

        fn wait_for_reader_exit(&self) {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut exited = self.reader_exited.lock().expect("reader exit lock");
            while !*exited {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "stdin reader did not retire");
                (exited, _) = self
                    .changed
                    .wait_timeout(exited, remaining)
                    .expect("reader exit wait");
            }
        }
    }

    #[derive(Default)]
    struct ProviderFixture {
        calls: Mutex<Vec<Value>>,
        changed: Condvar,
        block_start: AtomicBool,
        block_shell: AtomicBool,
        release_start: AtomicBool,
        release_shell: AtomicBool,
    }

    impl ProviderFixture {
        fn call(&self, request: &str) -> Result<String, String> {
            let request: Value = serde_json::from_str(request).expect("provider request JSON");
            let verb = request["command"]["verb"]
                .as_str()
                .expect("provider request verb")
                .to_owned();
            {
                let mut calls = self.calls.lock().expect("provider calls lock");
                calls.push(request);
                self.changed.notify_all();
                while (verb == "session-start"
                    && self.block_start.load(Ordering::Acquire)
                    && !self.release_start.load(Ordering::Acquire))
                    || (verb == "shell-exec"
                        && self.block_shell.load(Ordering::Acquire)
                        && !self.release_shell.load(Ordering::Acquire))
                {
                    calls = self.changed.wait(calls).expect("provider gate wait");
                }
            }
            Ok(match verb.as_str() {
                "session-start" => json!({
                    "ok": true,
                    "target": "current",
                    "command": verb,
                    "data": {"session_id": "fixture-session", "lease": "fixture-lease"}
                }),
                "shell-exec" => json!({
                    "ok": true,
                    "target": "current",
                    "command": verb,
                    "data": {"stdout": "done"}
                }),
                "session-end" => json!({
                    "ok": true,
                    "target": "current",
                    "command": verb,
                    "data": {}
                }),
                other => panic!("unexpected provider verb {other}"),
            }
            .to_string())
        }

        fn wait_for_calls(&self, count: usize) -> Vec<Value> {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut calls = self.calls.lock().expect("provider calls lock");
            while calls.len() < count {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "timed out waiting for provider calls");
                (calls, _) = self
                    .changed
                    .wait_timeout(calls, remaining)
                    .expect("provider calls wait");
            }
            calls.clone()
        }

        fn release_start(&self) {
            self.release_start.store(true, Ordering::Release);
            self.changed.notify_all();
        }

        fn release_shell(&self) {
            self.release_shell.store(true, Ordering::Release);
            self.changed.notify_all();
        }
    }

    fn send_test_message(sender: &mpsc::Sender<Vec<u8>>, message: Value) {
        let mut bytes = message.to_string().into_bytes();
        bytes.push(b'\n');
        sender.send(bytes).expect("send MCP test message");
    }

    fn initialize_test_session(sender: &mpsc::Sender<Vec<u8>>) {
        send_test_message(
            sender,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "fixture", "version": "1"}
                }
            }),
        );
        send_test_message(
            sender,
            json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
        );
    }

    fn send_test_shell(sender: &mpsc::Sender<Vec<u8>>, id: &str) {
        send_test_message(
            sender,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "agenterm_acu_shell_exec",
                    "arguments": {
                        "idempotency_key": format!("disconnect:{id}"),
                        "command": id,
                        "timeout_ms": 100,
                        "max_output_bytes": 1024
                    }
                }
            }),
        );
    }

    type DisconnectServer = (
        mpsc::Sender<Vec<u8>>,
        Arc<DisconnectState>,
        mpsc::Receiver<io::Result<()>>,
        thread::JoinHandle<()>,
    );

    fn start_disconnect_server(provider: Arc<ProviderFixture>) -> DisconnectServer {
        let (input_sender, input_receiver) = mpsc::channel();
        let output = Arc::new(DisconnectState::default());
        let worker_output = Arc::clone(&output);
        let reader_exit = Arc::clone(&output);
        let (done_sender, done_receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = serve_stdio_core(
                ChannelInput {
                    receiver: input_receiver,
                    buffer: Vec::new(),
                    offset: 0,
                    exit: reader_exit,
                },
                ControlledOutput(worker_output),
                McpStdioConfig::default(),
                move |request: &str| provider.call(request),
                true,
            );
            done_sender.send(result).expect("report MCP worker result");
        });
        (input_sender, output, done_receiver, worker)
    }

    fn provider_verbs(calls: &[Value]) -> Vec<&str> {
        calls
            .iter()
            .map(|call| call["command"]["verb"].as_str().expect("provider verb"))
            .collect()
    }

    fn trigger_disconnect(input: &mpsc::Sender<Vec<u8>>, output: &DisconnectState) {
        output.disconnected.store(true, Ordering::Release);
        send_test_message(
            input,
            json!({"jsonrpc":"2.0", "id":"disconnect", "method":"ping"}),
        );
        output.wait_until_observed();
    }

    #[test]
    fn stdout_disconnect_while_session_starts_cancels_all_requests_and_ends_once() {
        let provider = Arc::new(ProviderFixture::default());
        provider.block_start.store(true, Ordering::Release);
        let (input, output, done, worker) = start_disconnect_server(Arc::clone(&provider));
        initialize_test_session(&input);
        send_test_shell(&input, "first");
        send_test_shell(&input, "queued");
        assert_eq!(
            provider_verbs(&provider.wait_for_calls(1)),
            ["session-start"]
        );

        trigger_disconnect(&input, &output);
        send_test_shell(&input, "after-disconnect");
        provider.release_start();

        let result = done
            .recv_timeout(Duration::from_secs(2))
            .expect("stdout-only disconnect must complete while stdin remains open");
        assert_eq!(
            result
                .expect_err("stdout disconnect must be reported")
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            provider_verbs(&provider.wait_for_calls(2)),
            ["session-start", "session-end"]
        );
        drop(input);
        output.wait_for_reader_exit();
        worker.join().expect("join MCP server");
    }

    #[test]
    fn stdout_disconnect_waits_for_dispatched_and_cancels_queued_before_ending_once() {
        let provider = Arc::new(ProviderFixture::default());
        provider.block_shell.store(true, Ordering::Release);
        let (input, output, done, worker) = start_disconnect_server(Arc::clone(&provider));
        initialize_test_session(&input);
        send_test_shell(&input, "dispatched");
        assert_eq!(
            provider_verbs(&provider.wait_for_calls(2)),
            ["session-start", "shell-exec"]
        );
        send_test_shell(&input, "queued");

        trigger_disconnect(&input, &output);
        send_test_shell(&input, "after-disconnect");
        assert!(
            done.recv_timeout(Duration::from_millis(50)).is_err(),
            "teardown returned before the dispatched provider call completed"
        );
        provider.release_shell();

        let result = done
            .recv_timeout(Duration::from_secs(2))
            .expect("stdout-only disconnect teardown exceeded its bounded provider completion");
        assert_eq!(
            result
                .expect_err("stdout disconnect must be reported")
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            provider_verbs(&provider.wait_for_calls(3)),
            ["session-start", "shell-exec", "session-end"]
        );
        drop(input);
        output.wait_for_reader_exit();
        worker.join().expect("join MCP server");
    }

    #[test]
    fn initialize_notification_and_ping_follow_the_lifecycle() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":\"ping-1\",\"method\":\"ping\"}\n"
        ));
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            MCP_PROTOCOL_REVISION
        );
        assert_eq!(
            responses[0]["result"]["capabilities"],
            json!({
                "resources": {"subscribe": false, "listChanged": false},
                "tools": {"listChanged": false}
            })
        );
        assert_eq!(responses[1], success_response(json!("ping-1"), json!({})));
    }

    #[test]
    fn non_ping_request_is_rejected_before_initialized_notification() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/list\"}\n"
        ));
        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            MCP_PROTOCOL_REVISION
        );
        assert_eq!(responses[1]["error"]["code"], ERROR_NOT_INITIALIZED);
    }

    #[test]
    fn unsupported_revision_is_typed_and_does_not_advance_the_lifecycle() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":\"future\",\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"future\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":\"supported\",\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n"
        ));
        assert_eq!(responses[0]["id"], "future");
        assert_eq!(responses[0]["error"]["code"], ERROR_PROTOCOL_VERSION);
        assert_eq!(
            responses[0]["error"]["data"]["code"],
            "mcp_protocol_version"
        );
        assert_eq!(
            responses[0]["error"]["data"]["supported"],
            json!([MCP_PROTOCOL_REVISION])
        );
        assert_eq!(
            responses[1]["result"]["protocolVersion"],
            MCP_PROTOCOL_REVISION
        );
    }

    #[test]
    fn wait_tool_cannot_start_before_initialized_notification() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":",
            "{\"name\":\"agenterm_wait\",\"arguments\":{\"epoch\":\"e\",",
            "\"after_sequence\":0,\"event_kind\":\"tab.selected\",\"timeout_ms\":10}}}\n"
        ));
        assert_eq!(responses[1]["error"]["code"], ERROR_NOT_INITIALIZED);
    }

    #[test]
    fn ready_session_lists_exactly_four_metadata_resources() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/list\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"resources/read\",",
            "\"params\":{\"uri\":\"agenterm://fleet/unknown\"}}\n"
        ));
        assert_eq!(
            responses[1]["result"]["resources"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(responses[2]["error"]["code"], -32002);
    }

    #[test]
    fn ready_session_lists_three_read_only_tools() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n"
        ));
        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["name"], "agenterm_wait");
        assert_eq!(tools[1]["name"], "agenterm_acu_capabilities");
        assert_eq!(tools[2]["name"], "agenterm_acu_observe");
        assert_eq!(tools[1]["inputSchema"]["additionalProperties"], false);
        assert_eq!(tools[2]["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            tools[2]["inputSchema"]["properties"]["command"]["required"],
            json!(["verb", "target"])
        );
        assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
        assert_eq!(tools[1]["annotations"]["readOnlyHint"], true);
        assert_eq!(tools[2]["annotations"]["readOnlyHint"], true);
        assert_eq!(tools[2]["annotations"]["destructiveHint"], false);
        assert_eq!(tools[2]["annotations"]["openWorldHint"], true);
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["timeout_ms"]["maximum"],
            capabilities().limits.wait_timeout_ms_maximum
        );
    }

    #[test]
    fn production_stdio_refuses_the_unadvertised_mutation_without_provider_dispatch() {
        let responses = exchange(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":",
            "{\"name\":\"agenterm_acu_shell_exec\",\"arguments\":{}}}\n"
        ));
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["error"]["code"], ERROR_INVALID_PARAMS);
        assert_eq!(responses[1]["error"]["message"], "Unknown tool");
    }

    #[test]
    fn provider_reply_requires_the_shared_cu_reply_shape() {
        for invalid in [
            "true",
            r#"{"ok":true}"#,
            r#"{"ok":false,"target":"current","command":"shell-exec"}"#,
            r#"{"ok":false,"target":"current","command":"shell-exec","error":{"code":1,"message":"bad"}}"#,
        ] {
            assert_eq!(
                parse_provider_reply(Ok(invalid.to_owned())).unwrap_err(),
                "acu_provider_reply_invalid_shape",
                "{invalid}"
            );
        }
        assert!(
            parse_provider_reply(Ok(
                r#"{"ok":true,"target":"current","command":"shell-exec","data":{}}"#.to_owned()
            ))
            .is_ok()
        );
    }

    #[test]
    fn waiter_capacity_fails_closed_before_backend_allocation() {
        let message = json!({
            "jsonrpc": "2.0",
            "id": "wait-over-capacity",
            "method": "tools/call",
            "params": {
                "name": "agenterm_wait",
                "arguments": {
                    "epoch": "epoch-a",
                    "after_sequence": 7,
                    "event_kind": "tab.note",
                    "timeout_ms": 100
                }
            }
        });
        let (sender, _receiver) = mpsc::channel();
        let mut active = HashMap::new();
        let error = start_wait(
            &message,
            &McpStdioConfig::default(),
            &sender,
            &mut active,
            0,
        )
        .unwrap_err();
        assert_eq!(error["error"]["code"], -32003);
        assert_eq!(error["error"]["data"]["maximum_waiters"], 0);
        assert!(active.is_empty());
    }

    #[test]
    fn malformed_unknown_and_duplicate_initialize_are_typed() {
        let responses = exchange(concat!(
            "{bad json}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"unknown\"}\n"
        ));
        assert_eq!(responses[0]["error"]["code"], ERROR_PARSE);
        assert_eq!(responses[2]["error"]["code"], ERROR_INVALID_REQUEST);
        assert_eq!(responses[3]["error"]["code"], ERROR_METHOD_NOT_FOUND);
    }

    #[test]
    fn oversized_message_is_drained_and_the_next_message_survives() {
        let maximum = capabilities().limits.frame_bytes as usize;
        let mut input = vec![b'x'; maximum + 1];
        input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        serve_stdio(BufReader::new(Cursor::new(input)), &mut output).unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses[0]["error"]["code"], ERROR_INVALID_REQUEST);
        assert_eq!(responses[1], success_response(json!(7), json!({})));
    }

    #[test]
    fn oversized_response_is_replaced_by_a_bounded_typed_error() {
        let maximum = capabilities().limits.response_bytes as usize;
        let message = success_response(json!("oversized"), json!({"text": "x".repeat(maximum)}));
        let mut output = Vec::new();
        write_message(&mut output, &message).unwrap();
        assert!(output.len() <= maximum + 1);
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["error"]["code"], ERROR_RESPONSE_TOO_LARGE);
        assert_eq!(
            response["error"]["data"]["code"],
            "agenterm_response_too_large"
        );
        assert_eq!(response["error"]["data"]["maximum_bytes"], maximum);
    }

    #[test]
    fn notifications_never_receive_responses() {
        assert!(exchange("{\"jsonrpc\":\"2.0\",\"method\":\"unknown\"}\n").is_empty());
        assert!(
            exchange("{\"jsonrpc\":\"2.0\",\"method\":\"initialize\",\"params\":{}}\n").is_empty()
        );
    }
}
