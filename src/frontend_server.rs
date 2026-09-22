//! Frontend-side bootstrap and recovery for the independent AgenTerm server.
//!
//! Not a second server and not an IPC proxy. Product GUI/CLI clients use this
//! module to detect the control endpoint, spawn `agenterm server` when absent,
//! and apply limited reconnect restart policy. Session truth stays in
//! `server_app`; live UI I/O stays on the direct client↔server path after connect.

use crate::instances::intentional_shutdown_matches;
use crate::ui_client::UiClientModel;
use std::time::{Duration, Instant};

pub(crate) const GUI_FRONTEND_SERVER_START_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const GUI_FRONTEND_SERVER_RESTART_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrontendServerRecoveryState {
    server_restart_after: Instant,
    server_restart_suppressed: bool,
}

impl FrontendServerRecoveryState {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            server_restart_after: now,
            server_restart_suppressed: false,
        }
    }

    pub(crate) fn on_disconnected(&mut self, address: &str, server_pid: Option<u32>) {
        if server_pid.is_some_and(|pid| intentional_shutdown_matches(address, pid)) {
            self.server_restart_suppressed = true;
        }
    }

    pub(crate) fn on_reconnected(&mut self, now: Instant) {
        self.server_restart_after = now;
        self.server_restart_suppressed = false;
    }

    pub(crate) fn maybe_recover(&mut self, now: Instant) -> FrontendServerRecovery {
        let recovery = maybe_recover_frontend_server(
            now,
            self.server_restart_after,
            self.server_restart_suppressed,
        );
        if matches!(
            recovery,
            FrontendServerRecovery::Started | FrontendServerRecovery::Failed(_)
        ) {
            self.server_restart_after = next_frontend_server_restart_after(now);
        }
        recovery
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrontendServerRecovery {
    NoAction,
    Started,
    Failed(String),
}

impl FrontendServerRecovery {
    #[allow(dead_code)]
    pub(crate) fn contract_state(self) -> crate::frontend::FrontendContractState {
        match self {
            Self::NoAction | Self::Started => crate::frontend::FrontendContractState::Supported,
            Self::Failed(_) => crate::frontend::FrontendContractState::Failed,
        }
    }
}

/// Connect a GUI to its authority, starting one if none answers, and refuse
/// the attach unless both sides run the same verified chassis image -- or
/// neither runs one. Which image a session runs is never decided by whichever
/// process happened to start the server first.
///
/// Readiness and the image are read through `protocol-info`, which is
/// read-only. `UiClientModel::connect` is not: it attaches the UI lease and
/// bootstraps the client, so it runs only after the image matched.
pub(crate) fn connect_or_start_frontend_gui_client(
    client_id: &str,
) -> Result<UiClientModel, String> {
    let port = LivePort { client_id };
    if server_answers() {
        return verified_attach(&port).map_err(AttachError::into_message);
    }

    // Resolved default pipe/socket is down. Before spawning a second authority
    // (which would re-spawn empty shells from workspace.json and look like a
    // full session reset), attach any live registration for this logical
    // instance (e.g. another `main` still holding agent tabs after Keep Server).
    if let Some(endpoint) = discover_live_peer_endpoint()? {
        pin_client_endpoint(&endpoint);
        return verified_attach(&port).map_err(|error| match error {
            AttachError::Connect(error) => format!(
                "live AgenTerm server for this instance is reachable at {endpoint} but UI connect failed: {error}"
            ),
            other => other.into_message(),
        });
    }

    start_frontend_server_process()?;
    let deadline = Instant::now() + GUI_FRONTEND_SERVER_START_TIMEOUT;
    while !server_answers() {
        if Instant::now() >= deadline {
            return Err(
                "could not start independent AgenTerm server: server did not become ready"
                    .to_owned(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    verified_attach(&port).map_err(|error| match error {
        AttachError::Connect(error) => {
            format!("could not start independent AgenTerm server: {error}")
        }
        other => other.into_message(),
    })
}

/// Every GUI (re)attach goes through here or through
/// [`connect_or_start_frontend_gui_client`]: a first connect, a reconnect
/// after the server went away, a switch to another instance. On a refusal
/// the UI lease was never attached and the caller stays disconnected with an
/// error naming the mismatch. `tests/chassis_server_image.rs` pins that no
/// other GUI path calls `UiClientModel::connect` directly.
pub(crate) fn connect_verified_frontend_gui_client(
    client_id: &str,
) -> Result<UiClientModel, String> {
    verified_attach(&LivePort { client_id }).map_err(AttachError::into_message)
}

/// Whether an authority answers the read-only `protocol-info`.
fn server_answers() -> bool {
    crate::client::send_ipc_request(vec!["protocol-info".to_owned()])
        .is_ok_and(|response| response.ok)
}

/// What a GUI attach needs from the world, so the order of its steps can be
/// tested: read the server's image without side effects, then attach the UI
/// lease, then give it back.
pub(crate) trait AttachPort {
    type Client;
    /// The server's image id through a read-only request.
    fn server_image(&self) -> Result<Option<String>, String>;
    /// This process's image id.
    fn local_image(&self) -> Option<String>;
    /// Attach the UI lease and bootstrap a client. Not read-only.
    fn attach(&self) -> Result<Self::Client, String>;
    fn detach(&self, client: Self::Client);
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AttachError {
    /// The images differ or could not be read; nothing was attached, or what
    /// was attached has been given back.
    Image(String),
    /// The image matched but the server refused the UI client.
    Connect(String),
}

impl AttachError {
    fn into_message(self) -> String {
        match self {
            Self::Image(message) => message,
            Self::Connect(message) => format!("running server rejected replaceable UI: {message}"),
        }
    }
}

/// Check the image read-only, attach only on a match, and check again after:
/// a server replaced between the two steps is detached rather than kept.
pub(crate) fn verified_attach<P: AttachPort>(port: &P) -> Result<P::Client, AttachError> {
    let local = port.local_image();
    let before = port.server_image().map_err(AttachError::Image)?;
    image_match(local.as_deref(), before.as_deref()).map_err(AttachError::Image)?;
    let client = port.attach().map_err(AttachError::Connect)?;
    let after = port
        .server_image()
        .and_then(|server| image_match(local.as_deref(), server.as_deref()));
    if let Err(refusal) = after {
        port.detach(client);
        return Err(AttachError::Image(refusal));
    }
    Ok(client)
}

struct LivePort<'a> {
    client_id: &'a str,
}

impl AttachPort for LivePort<'_> {
    type Client = UiClientModel;
    fn server_image(&self) -> Result<Option<String>, String> {
        read_server_image_id()
    }
    fn local_image(&self) -> Option<String> {
        crate::frontend::chassis_image::loaded_image_identity()
            .map(|identity| identity.image_id.clone())
    }
    fn attach(&self) -> Result<UiClientModel, String> {
        UiClientModel::connect(self.client_id.to_owned()).map_err(|error| error.to_string())
    }
    fn detach(&self, mut client: UiClientModel) {
        let _ = client.detach();
    }
}

/// How a client states the image it expects the authority to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageDeclaration {
    /// A GUI, or a CLI given `--chassis-image`: the server must run exactly
    /// this process's image, and must run none when this process loaded none.
    Strict,
}

/// Compare this process's loaded image with the one the running authority
/// reports through `protocol-info`. Refusals carry content ids only.
pub(crate) fn verify_server_chassis_image(declaration: ImageDeclaration) -> Result<(), String> {
    let ImageDeclaration::Strict = declaration;
    let server = read_server_image_id()?;
    let local = crate::frontend::chassis_image::loaded_image_identity()
        .map(|identity| identity.image_id.as_str());
    image_match(local, server.as_deref())
}

/// The running authority's image id, through the read-only `protocol-info`.
fn read_server_image_id() -> Result<Option<String>, String> {
    let response = crate::client::send_ipc_request(vec!["protocol-info".to_owned()])
        .map_err(|error| format!("chassis_image_unverifiable: {error:#}"))?;
    if !response.ok {
        return Err(format!("chassis_image_unverifiable: {}", response.error));
    }
    let info: serde_json::Value = serde_json::from_str(&response.output)
        .map_err(|_| "chassis_image_unverifiable: protocol-info is not JSON".to_owned())?;
    server_image_id(&info)
}

/// The server's image id from `protocol-info`. An older server that predates
/// the field, or one reporting `null`, runs no image. Anything else must be
/// an object whose `image_id` is 64 lowercase hex digits: a malformed report
/// is unverifiable, never read as "no image".
fn server_image_id(info: &serde_json::Value) -> Result<Option<String>, String> {
    let image = match info.get("chassis_image") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(image) => image,
    };
    let id = image
        .as_object()
        .and_then(|object| object.get("image_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| {
            id.len() == 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| {
            "chassis_image_unverifiable: the server reported a malformed chassis_image".to_owned()
        })?;
    Ok(Some(id.to_owned()))
}

fn image_match(local: Option<&str>, server: Option<&str>) -> Result<(), String> {
    if local == server {
        return Ok(());
    }
    Err(format!(
        "chassis_image_mismatch: this client runs {} but the server runs {}; \
         stop that server or launch with the matching image",
        local.unwrap_or("no image"),
        server.unwrap_or("no image")
    ))
}

fn discover_live_peer_endpoint() -> Result<Option<crate::ipc_endpoint::IpcEndpoint>, String> {
    let resolved = crate::client::resolved_ipc_endpoint().map_err(|error| error.to_string())?;
    crate::instances::find_live_endpoint_for_logical_instance(
        &resolved.logical_instance,
        Some(&resolved.endpoint),
        Duration::from_millis(250),
    )
    .map_err(|error| error.to_string())
}

/// Pin the process IPC selector so subsequent control calls target the live peer.
fn pin_client_endpoint(endpoint: &crate::ipc_endpoint::IpcEndpoint) {
    // Keep current logical-instance identity when only the endpoint peer changes.
    let instance = crate::client::resolved_ipc_endpoint()
        .ok()
        .map(|resolved| resolved.logical_instance.canonical_name());
    let _ = pin_client_peer_for_gui(endpoint, instance.as_deref());
}

/// Pin GUI/CLI process selectors to a discovered peer endpoint.
///
/// Must update [`crate::client::set_ipc_selectors`] (not only env vars): launch
/// `--instance` lives in `IPC_SELECTOR_OVERRIDE` and would otherwise win forever.
/// When `instance` is provided it is kept as logical identity even though the
/// authority is the explicit endpoint (endpoint-only used to force `main`).
pub(crate) fn pin_client_peer_for_gui(
    endpoint: &crate::ipc_endpoint::IpcEndpoint,
    instance: Option<&str>,
) -> Result<(), String> {
    use crate::ipc_endpoint::EndpointSelectorArgs;
    let selectors = EndpointSelectorArgs {
        endpoint: Some(endpoint.to_string()),
        address: None,
        instance: instance.map(str::to_owned),
    };
    crate::client::set_ipc_selectors(selectors).map_err(|error| error.to_string())?;
    // SAFETY: single-threaded at GUI bootstrap / UI-action attach before races.
    unsafe {
        std::env::set_var("AGENTERM_IPC_ENDPOINT", endpoint.to_string());
        std::env::remove_var("AGENTERM_IPC_ADDRESS");
        if let Some(instance) = instance {
            std::env::set_var("AGENTERM_INSTANCE", instance);
        } else {
            std::env::remove_var("AGENTERM_INSTANCE");
        }
    }
    Ok(())
}

pub(crate) fn maybe_recover_frontend_server(
    now: Instant,
    server_restart_after: Instant,
    server_restart_suppressed: bool,
) -> FrontendServerRecovery {
    if server_restart_suppressed
        || now < server_restart_after
        || is_frontend_server_endpoint_listening()
    {
        return FrontendServerRecovery::NoAction;
    }

    // A peer under the same logical instance may still be live on another
    // endpoint (historical dual-main). Prefer pinning to it over spawning.
    if let Ok(Some(endpoint)) = discover_live_peer_endpoint() {
        pin_client_endpoint(&endpoint);
        if is_frontend_server_endpoint_listening() {
            return FrontendServerRecovery::NoAction;
        }
    }

    match start_frontend_server_process() {
        Ok(()) => FrontendServerRecovery::Started,
        Err(error) => FrontendServerRecovery::Failed(error),
    }
}

pub(crate) fn next_frontend_server_restart_after(now: Instant) -> Instant {
    now + GUI_FRONTEND_SERVER_RESTART_INTERVAL
}

pub(crate) fn is_frontend_server_endpoint_listening() -> bool {
    crate::client::ipc_endpoint().is_ok_and(|endpoint| {
        crate::ipc_transport::IpcStream::connect(&endpoint, Duration::from_millis(100)).is_ok()
    })
}

pub(crate) fn start_frontend_server_process() -> Result<(), String> {
    let resolved = crate::client::resolved_ipc_endpoint().map_err(|error| error.to_string())?;
    // Never mint a second Fleet authority while one already answers protocol
    // for this logical instance — that path is what wipes agent tabs via
    // workspace re-spawn of empty shells.
    //
    // The guard is about the *implicit* endpoint. `--address HOST:PORT` and
    // `--endpoint` name a concrete transport, and autostarting the server the
    // operator asked for is that flag's documented behaviour. Applying the
    // logical-instance guard there did two wrong things at once: it refused to
    // start the requested server, and `pin_client_endpoint` re-pointed this
    // client at the *other* live server — so the caller's command then ran
    // against a server it never named and exited 0, with the refusal only on
    // stderr. `cli --address 127.0.0.1:43004 new-window -n X` created X on
    // 127.0.0.1:43001 exactly this way.
    if !crate::client::transport_was_pinned_explicitly()
        && let Some(endpoint) = crate::instances::find_live_endpoint_for_logical_instance(
            &resolved.logical_instance,
            Some(&resolved.endpoint),
            Duration::from_millis(250),
        )
        .map_err(|error| error.to_string())?
    {
        pin_client_endpoint(&endpoint);
        return Err(format!(
            "refusing to start a second AgenTerm server: live instance `{}` already at {endpoint}. \
             Reopen attaches to that server; stop it explicitly if you need a clean slate.",
            resolved.logical_instance.canonical_name()
        ));
    }
    let parameter = frontend_server_spawn_parameter(&resolved);
    let started = crate::platform::process::autostart_server(
        parameter.0,
        &parameter.1,
        crate::frontend::chassis_image::loaded_image_root(),
    )
    .map_err(|error| error.to_string())?;
    if !started {
        Err("independent AgenTerm server autostart is unavailable".to_owned())
    } else {
        Ok(())
    }
}

// The spawned server re-resolves its own identity from the one CLI selector
// it receives, and an endpoint selector cannot carry a logical-instance name
// (the scope hash is one-way, and a CLI selector suppresses inherited
// environment selectors by design). Passing the scope-default native endpoint
// as `--endpoint` therefore registered every autostarted server as `main`
// even when the client was launched with `--instance custom:...`. Hand the
// instance name across the spawn boundary whenever the endpoint is exactly
// the one derived from it; the child re-derives the identical endpoint.
pub(crate) fn frontend_server_spawn_parameter(
    resolved: &crate::ipc_endpoint::ResolvedIpcEndpoint,
) -> (&'static str, String) {
    if let Some(address) = resolved.endpoint.legacy_address() {
        ("--address", address)
    } else if resolved.endpoint
        == crate::platform::ipc::default_native_endpoint(&resolved.server_scope_id)
    {
        ("--instance", resolved.logical_instance.canonical_name())
    } else {
        ("--endpoint", resolved.endpoint.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{AttachError, AttachPort, verified_attach};
    use super::{
        FrontendServerRecovery, GUI_FRONTEND_SERVER_RESTART_INTERVAL, image_match,
        next_frontend_server_restart_after, server_image_id,
    };
    use std::cell::{Cell, RefCell};

    /// A scripted server: the image it reports on each read, whether it
    /// accepts the UI client, and a record of every lease attach and detach.
    struct FakePort {
        local: Option<String>,
        reports: RefCell<Vec<Result<Option<String>, String>>>,
        accept: bool,
        attached: Cell<u32>,
        detached: Cell<u32>,
    }

    impl FakePort {
        fn new(local: Option<&str>, reports: &[Result<Option<&str>, &str>], accept: bool) -> Self {
            Self {
                local: local.map(str::to_owned),
                reports: RefCell::new(
                    reports
                        .iter()
                        .rev()
                        .map(|report| {
                            report
                                .map(|id| id.map(str::to_owned))
                                .map_err(str::to_owned)
                        })
                        .collect(),
                ),
                accept,
                attached: Cell::new(0),
                detached: Cell::new(0),
            }
        }
    }

    impl AttachPort for FakePort {
        type Client = ();
        fn server_image(&self) -> Result<Option<String>, String> {
            self.reports
                .borrow_mut()
                .pop()
                .expect("an unscripted image read")
        }
        fn local_image(&self) -> Option<String> {
            self.local.clone()
        }
        fn attach(&self) -> Result<(), String> {
            self.attached.set(self.attached.get() + 1);
            if self.accept {
                Ok(())
            } else {
                Err("ui_lease_conflict".to_owned())
            }
        }
        fn detach(&self, _client: ()) {
            self.detached.set(self.detached.get() + 1);
        }
    }

    /// The UI lease is never attached to a server running another image,
    /// none, or one whose image cannot be read.
    #[test]
    fn a_mismatched_server_never_sees_a_ui_lease_attach() {
        for (local, report) in [
            (Some("aa"), Ok(Some("bb"))),
            (Some("aa"), Ok(None)),
            (None, Ok(Some("aa"))),
            (Some("aa"), Err("chassis_image_unverifiable: malformed")),
        ] {
            let port = FakePort::new(local, &[report], true);
            let error = verified_attach(&port).expect_err("refused");
            assert!(matches!(error, AttachError::Image(_)), "{error:?}");
            assert_eq!(
                port.attached.get(),
                0,
                "no lease attach for {local:?} vs {report:?}"
            );
            assert_eq!(port.detached.get(), 0);
        }
    }

    #[test]
    fn a_matching_server_is_attached_once_and_kept() {
        let port = FakePort::new(Some("aa"), &[Ok(Some("aa")), Ok(Some("aa"))], true);
        assert!(verified_attach(&port).is_ok());
        assert_eq!((port.attached.get(), port.detached.get()), (1, 0));
        let plain = FakePort::new(None, &[Ok(None), Ok(None)], true);
        assert!(verified_attach(&plain).is_ok());
    }

    /// A server replaced between the check and the attach is given its lease
    /// back rather than kept.
    #[test]
    fn a_server_swapped_during_attach_is_detached() {
        let port = FakePort::new(Some("aa"), &[Ok(Some("aa")), Ok(Some("bb"))], true);
        let error = verified_attach(&port).expect_err("swapped");
        assert!(matches!(error, AttachError::Image(_)), "{error:?}");
        assert_eq!((port.attached.get(), port.detached.get()), (1, 1));
    }

    #[test]
    fn a_refused_ui_client_is_a_connect_error_not_an_image_error() {
        let port = FakePort::new(None, &[Ok(None)], false);
        assert_eq!(
            verified_attach(&port),
            Err(AttachError::Connect("ui_lease_conflict".to_owned()))
        );
    }

    /// A strict client attaches only to a server running the same image, and
    /// either side lacking one is a refusal, not a fallback.
    #[test]
    fn strict_image_match_refuses_every_mismatch() {
        assert!(image_match(None, None).is_ok());
        assert!(image_match(Some("aa"), Some("aa")).is_ok());
        for (local, server) in [
            (Some("aa"), None),
            (None, Some("aa")),
            (Some("aa"), Some("bb")),
        ] {
            let refusal = image_match(local, server).expect_err("mismatch");
            assert!(refusal.starts_with("chassis_image_mismatch:"), "{refusal}");
        }
        let refusal = image_match(Some("aa"), None).expect_err("server has none");
        assert!(
            refusal.contains("runs aa") && refusal.contains("runs no image"),
            "{refusal}"
        );
    }

    /// An older server without the field, or `null`, runs no image; a
    /// malformed report is unverifiable and never read as "no image".
    #[test]
    fn a_malformed_server_image_report_is_refused_not_ignored() {
        use serde_json::json;
        let id = "0123456789abcdef".repeat(4);
        assert_eq!(server_image_id(&json!({})), Ok(None));
        assert_eq!(server_image_id(&json!({"chassis_image": null})), Ok(None));
        assert_eq!(
            server_image_id(&json!({"chassis_image": {"image_id": id}})),
            Ok(Some(id.clone()))
        );
        for malformed in [
            json!({"chassis_image": "0123"}),
            json!({"chassis_image": 7}),
            json!({"chassis_image": {}}),
            json!({"chassis_image": {"image_id": null}}),
            json!({"chassis_image": {"image_id": "abc"}}),
            json!({"chassis_image": {"image_id": id.to_uppercase()}}),
            json!({"chassis_image": {"image_id": format!("{id}0")}}),
            json!({"chassis_image": [id]}),
        ] {
            let refusal = server_image_id(&malformed).expect_err("malformed");
            assert!(
                refusal.starts_with("chassis_image_unverifiable:"),
                "{refusal}"
            );
        }
    }

    use std::time::{Duration, Instant};

    #[test]
    fn spawn_parameter_carries_the_instance_name_for_scope_default_endpoints() {
        let logical_instance = crate::ipc_endpoint::LogicalInstance::Custom("work".to_owned());
        let server_scope_id = crate::ipc_endpoint::ServerScopeId::current(&logical_instance)
            .expect("current OS-user scope");
        let resolved = crate::ipc_endpoint::ResolvedIpcEndpoint {
            endpoint: crate::platform::ipc::default_native_endpoint(&server_scope_id),
            logical_instance,
            server_scope_id,
            explicit: true,
        };
        assert_eq!(
            super::frontend_server_spawn_parameter(&resolved),
            ("--instance", "custom:work".to_owned())
        );
    }

    #[test]
    fn spawn_parameter_preserves_explicit_endpoint_and_legacy_address_authority() {
        let logical_instance = crate::ipc_endpoint::LogicalInstance::Main;
        let server_scope_id = crate::ipc_endpoint::ServerScopeId::current(&logical_instance)
            .expect("current OS-user scope");
        let custom = crate::ipc_endpoint::ResolvedIpcEndpoint {
            endpoint: crate::ipc_endpoint::IpcEndpoint::NamedPipe("agenterm-custom".to_owned()),
            logical_instance: logical_instance.clone(),
            server_scope_id: server_scope_id.clone(),
            explicit: true,
        };
        let (custom_name, custom_value) = super::frontend_server_spawn_parameter(&custom);
        assert_eq!(custom_name, "--endpoint");
        assert!(custom_value.contains("agenterm-custom"));
        let legacy = crate::ipc_endpoint::ResolvedIpcEndpoint {
            endpoint: crate::ipc_endpoint::IpcEndpoint::Tcp {
                host: "127.0.0.1".to_owned(),
                port: 48815,
            },
            logical_instance,
            server_scope_id,
            explicit: true,
        };
        assert_eq!(
            super::frontend_server_spawn_parameter(&legacy),
            ("--address", "127.0.0.1:48815".to_owned())
        );
    }

    #[test]
    fn recovery_delay_is_computable() {
        let now = Instant::now();
        let later = next_frontend_server_restart_after(now);
        assert_eq!(
            later
                .checked_duration_since(now)
                .unwrap_or_else(|| Duration::from_millis(0)),
            GUI_FRONTEND_SERVER_RESTART_INTERVAL
        );
    }

    #[test]
    fn recovery_contract_state_maps_expected_states() {
        assert!(matches!(
            FrontendServerRecovery::NoAction.contract_state(),
            crate::frontend::FrontendContractState::Supported
        ));
        assert!(matches!(
            FrontendServerRecovery::Started.contract_state(),
            crate::frontend::FrontendContractState::Supported
        ));
        assert!(matches!(
            FrontendServerRecovery::Failed("x".to_owned()).contract_state(),
            crate::frontend::FrontendContractState::Failed
        ));
    }
}
