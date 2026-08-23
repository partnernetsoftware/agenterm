use super::{
    auth::{AuthHelper, AuthResult, SecurityType},
    connection::VncClient,
};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{info, trace};

use crate::{PixelFormat, VncEncoding, VncError, VncVersion};

pub enum VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    Handshake(VncConnector<S, F>),
    Authenticate(VncConnector<S, F>),
    Connected(VncClient),
}

impl<S, F> VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    pub fn try_start(
        self,
    ) -> Pin<Box<dyn Future<Output = Result<Self, VncError>> + Send + Sync + 'static>> {
        Box::pin(async move {
            match self {
                VncState::Handshake(mut connector) => {
                    // Read the rfbversion informed by the server
                    let rfbversion = VncVersion::read(&mut connector.stream).await?;
                    trace!(
                        "Our version {:?}, server version {:?}",
                        connector.rfb_version,
                        rfbversion
                    );
                    let rfbversion = if connector.rfb_version < rfbversion {
                        connector.rfb_version
                    } else {
                        rfbversion
                    };

                    // Record the negotiated rfbversion
                    connector.rfb_version = rfbversion;
                    trace!("Negotiated rfb version: {:?}", rfbversion);
                    rfbversion.write(&mut connector.stream).await?;
                    Ok(VncState::Authenticate(connector).try_start().await?)
                }
                VncState::Authenticate(mut connector) => {
                    let security_types =
                        SecurityType::read(&mut connector.stream, &connector.rfb_version).await?;

                    assert!(!security_types.is_empty());

                    if security_types.contains(&SecurityType::None) {
                        match connector.rfb_version {
                            VncVersion::RFB33 => {
                                // If the security-type is 1, for no authentication, the server does not
                                // send the SecurityResult message but proceeds directly to the
                                // initialization messages (Section 7.3).
                                info!("No auth needed in vnc3.3");
                            }
                            VncVersion::RFB37 => {
                                // After the security handshake, if the security-type is 1, for no
                                // authentication, the server does not send the SecurityResult message
                                // but proceeds directly to the initialization messages (Section 7.3).
                                info!("No auth needed in vnc3.7");
                                SecurityType::write(&SecurityType::None, &mut connector.stream)
                                    .await?;
                            }
                            VncVersion::RFB38 => {
                                info!("No auth needed in vnc3.8");
                                SecurityType::write(&SecurityType::None, &mut connector.stream)
                                    .await?;
                                let mut ok = [0; 4];
                                connector.stream.read_exact(&mut ok).await?;
                            }
                        }
                    } else if security_types.contains(&SecurityType::AppleRemoteDesktop)
                        && connector.ard_handler.is_some()
                    {
                        // AGENTERM PATCH: Apple Remote Management (type 30).
                        // The Diffie-Hellman / AES work lives in the embedding
                        // crate; this branch only does the RFB framing and
                        // hands the parameters across.
                        SecurityType::write(
                            &SecurityType::AppleRemoteDesktop,
                            &mut connector.stream,
                        )
                        .await?;

                        let mut generator = [0u8; 2];
                        connector.stream.read_exact(&mut generator).await?;
                        let key_length = connector.stream.read_u16().await? as usize;
                        if key_length == 0 || key_length > 1024 {
                            return Err(VncError::General(
                                "the server proposed an unreasonable Diffie-Hellman key length"
                                    .to_owned(),
                            ));
                        }
                        let mut prime = vec![0u8; key_length];
                        connector.stream.read_exact(&mut prime).await?;
                        let mut server_public_key = vec![0u8; key_length];
                        connector.stream.read_exact(&mut server_public_key).await?;

                        let handler = connector.ard_handler.take().unwrap();
                        let (ciphertext, public_key) = handler(ArdChallenge {
                            generator: generator.to_vec(),
                            prime,
                            server_public_key,
                        })
                        .map_err(VncError::General)?;
                        connector.stream.write_all(&ciphertext).await?;
                        connector.stream.write_all(&public_key).await?;

                        let result = connector.stream.read_u32().await?;
                        if let AuthResult::Failed = result.into() {
                            return Err(VncError::WrongPassword);
                        }
                    } else {
                        // choose a auth method
                        if security_types.contains(&SecurityType::VncAuth) {
                            if connector.rfb_version != VncVersion::RFB33 {
                                // In the security handshake (Section 7.1.2), rather than a two-way
                                // negotiation, the server decides the security type and sends a single
                                // word:

                                //            +--------------+--------------+---------------+
                                //            | No. of bytes | Type [Value] | Description   |
                                //            +--------------+--------------+---------------+
                                //            | 4            | U32          | security-type |
                                //            +--------------+--------------+---------------+

                                // The security-type may only take the value 0, 1, or 2.  A value of 0
                                // means that the connection has failed and is followed by a string
                                // giving the reason, as described in Section 7.1.2.
                                SecurityType::write(&SecurityType::VncAuth, &mut connector.stream)
                                    .await?;
                            }
                        } else {
                            let msg = "Security type apart from Vnc Auth has not been implemented";
                            return Err(VncError::General(msg.to_owned()));
                        }

                        // get password
                        if connector.auth_methond.is_none() {
                            return Err(VncError::NoPassword);
                        }

                        let credential = (connector.auth_methond.take().unwrap()).await?;

                        // auth
                        let auth = AuthHelper::read(&mut connector.stream, &credential).await?;
                        auth.write(&mut connector.stream).await?;
                        let result = auth.finish(&mut connector.stream).await?;
                        if let AuthResult::Failed = result {
                            if let VncVersion::RFB37 = connector.rfb_version {
                                // In VNC Authentication (Section 7.2.2), if the authentication fails,
                                // the server sends the SecurityResult message, but does not send an
                                // error message before closing the connection.
                                return Err(VncError::WrongPassword);
                            } else {
                                let _ = connector.stream.read_u32().await?;
                                let mut err_msg = String::new();
                                connector.stream.read_to_string(&mut err_msg).await?;
                                return Err(VncError::General(err_msg));
                            }
                        }
                    }
                    info!("auth done, client connected");

                    Ok(VncState::Connected(
                        VncClient::new(
                            connector.stream,
                            connector.allow_shared,
                            connector.pixel_format,
                            connector.encodings,
                        )
                        .await?,
                    ))
                }
                _ => unreachable!(),
            }
        })
    }

    pub fn finish(self) -> Result<VncClient, VncError> {
        if let VncState::Connected(client) = self {
            Ok(client)
        } else {
            Err(VncError::ConnectError)
        }
    }
}

/// Connection Builder to setup a vnc client
/// AGENTERM PATCH: the Diffie-Hellman parameters an Apple Remote Management
/// server sends, handed to the embedder so the cryptography stays outside this
/// vendored copy.
#[derive(Debug, Clone)]
pub struct ArdChallenge {
    pub generator: Vec<u8>,
    pub prime: Vec<u8>,
    pub server_public_key: Vec<u8>,
}

/// AGENTERM PATCH: returns `(ciphertext, client_public_key)` or a message.
pub type ArdHandler =
    Box<dyn FnOnce(ArdChallenge) -> Result<(Vec<u8>, Vec<u8>), String> + Send + Sync>;

pub struct VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    stream: S,
    auth_methond: Option<F>,
    /// AGENTERM PATCH: set to enable Apple Remote Management.
    ard_handler: Option<ArdHandler>,
    rfb_version: VncVersion,
    allow_shared: bool,
    pixel_format: Option<PixelFormat>,
    encodings: Vec<VncEncoding>,
}

impl<S, F> VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    /// To new a vnc client configuration with stream `S`
    ///
    /// `S` should implement async I/O methods
    ///
    /// ```no_run
    /// use vnc::{PixelFormat, VncConnector, VncError};
    /// use tokio::{self, net::TcpStream};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), VncError> {
    ///     let tcp = TcpStream::connect("127.0.0.1:5900").await?;
    ///     let vnc = VncConnector::new(tcp)
    ///         .set_auth_method(async move { Ok("password".to_string()) })
    ///         .add_encoding(vnc::VncEncoding::Tight)
    ///         .add_encoding(vnc::VncEncoding::Zrle)
    ///         .add_encoding(vnc::VncEncoding::CopyRect)
    ///         .add_encoding(vnc::VncEncoding::Raw)
    ///         .allow_shared(true)
    ///         .set_pixel_format(PixelFormat::bgra())
    ///         .build()?
    ///         .try_start()
    ///         .await?
    ///         .finish()?;
    ///     Ok(())
    /// }
    /// ```
    ///
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            auth_methond: None,
            ard_handler: None,
            allow_shared: true,
            rfb_version: VncVersion::RFB38,
            pixel_format: None,
            encodings: Vec::new(),
        }
    }

    /// An async callback which is used to query credentials if the vnc server has set
    ///
    /// ```no_compile
    /// connector = connector.set_auth_method(async move { Ok("password".to_string()) })
    /// ```
    ///
    /// if you're building a wasm app,
    /// the async callback also allows you to combine it to a promise
    ///
    /// ```no_compile
    /// #[wasm_bindgen]
    /// extern "C" {
    ///     fn get_password() -> js_sys::Promise;
    /// }
    ///
    /// connector = connector
    ///        .set_auth_method(async move {
    ///            let auth = JsFuture::from(get_password()).await.unwrap();
    ///            Ok(auth.as_string().unwrap())
    ///     });
    /// ```
    ///
    /// While in the js code
    ///
    ///
    /// ```javascript
    /// var password = '';
    /// function get_password() {
    ///     return new Promise((reslove, reject) => {
    ///        document.getElementById("submit_password").addEventListener("click", () => {
    ///             password = window.document.getElementById("input_password").value
    ///             reslove(password)
    ///         })
    ///     });
    /// }
    /// ```
    ///
    /// The future won't be polled if the sever doesn't apply any password protections to the session
    ///
    pub fn set_auth_method(mut self, auth_callback: F) -> Self {
        self.auth_methond = Some(auth_callback);
        self
    }

    /// The max vnc version that we supported
    ///
    /// Version should be one of the [VncVersion]
    ///
    /// AGENTERM PATCH: supply the Apple Remote Management responder.
    ///
    /// Setting this makes type 30 selectable; without it the connector behaves
    /// exactly as upstream and ignores the security type entirely.
    pub fn set_ard_handler(mut self, handler: ArdHandler) -> Self {
        self.ard_handler = Some(handler);
        self
    }

    pub fn set_version(mut self, version: VncVersion) -> Self {
        self.rfb_version = version;
        self
    }

    /// Set the rgb order which you will use to resolve the image data
    ///
    /// In most of the case, use `PixelFormat::bgra()` on little endian PCs
    ///
    /// And use `PixelFormat::rgba()` on wasm apps (with canvas)
    ///
    /// Also, customized format is allowed
    ///
    /// Will use the default format informed by the vnc server if not set
    ///
    /// In this condition, the client will get a [crate::VncEvent::SetPixelFormat] event notified
    ///
    pub fn set_pixel_format(mut self, pf: PixelFormat) -> Self {
        self.pixel_format = Some(pf);
        self
    }

    /// Shared-flag is non-zero (true) if the server should try to share the
    ///
    /// desktop by leaving other clients connected, and zero (false) if it
    ///
    /// should give exclusive access to this client by disconnecting all
    ///
    /// other clients.
    ///
    pub fn allow_shared(mut self, allow_shared: bool) -> Self {
        self.allow_shared = allow_shared;
        self
    }

    /// Client encodings that we want to use
    ///
    /// One of [VncEncoding]
    ///
    /// [VncEncoding::Raw] must be sent as the RFC required
    ///
    /// The order to add encodings is the order to inform the server
    ///
    pub fn add_encoding(mut self, encoding: VncEncoding) -> Self {
        self.encodings.push(encoding);
        self
    }

    /// Complete the client configuration
    ///
    pub fn build(self) -> Result<VncState<S, F>, VncError> {
        if self.encodings.is_empty() {
            return Err(VncError::NoEncoding);
        }
        Ok(VncState::Handshake(self))
    }
}
