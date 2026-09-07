//! Bounded one-request-per-connection framing for the system privilege broker.

use std::io::{Read, Write};

use crate::{
    CuError,
    privilege_apply::{
        MAX_REPLY_BYTES, MAX_REQUEST_BYTES, PrivilegeApplyReplyV1, parse_apply_reply,
    },
};

const LENGTH_BYTES: usize = 4;

#[cfg(target_os = "macos")]
const SERVER_REPLY_READY: u8 = 1;
#[cfg(target_os = "macos")]
const SERVER_CONSENT_REQUIRED: u8 = 2;
#[cfg(target_os = "macos")]
const CLIENT_AUTHORIZATION_PROOF: u8 = 17;
#[cfg(target_os = "macos")]
const CLIENT_CONSENT_CANCELED: u8 = 18;
#[cfg(target_os = "macos")]
const CLIENT_CONSENT_DENIED: u8 = 19;
#[cfg(target_os = "macos")]
const CLIENT_CONSENT_TIMED_OUT: u8 = 20;
#[cfg(target_os = "macos")]
const CLIENT_CONSENT_FAILED: u8 = 21;

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MacosServerDisposition {
    ReplyReady,
    ConsentRequired,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MacosClientConsent {
    Canceled,
    Denied,
    TimedOut,
    Failed,
}

#[cfg(target_os = "macos")]
pub(crate) enum MacosBrokerConsent {
    Proof(agenterm_platform::privilege_authorization::MacosAuthorizationProof),
    Canceled,
    Denied,
    TimedOut,
    Failed,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn read_request(mut input: impl Read) -> Result<Vec<u8>, CuError> {
    read_frame(&mut input, MAX_REQUEST_BYTES, "request")
}

pub(crate) fn write_request(mut output: impl Write, bytes: &[u8]) -> Result<(), CuError> {
    write_frame(&mut output, bytes, MAX_REQUEST_BYTES, "request")
}

/// Read the first macOS request frame without requiring connection EOF.
///
/// The same authenticated stream remains open for the broker's replay result
/// or one Authorization Services proof; no second connection can substitute
/// another kernel peer between those phases.
#[cfg(target_os = "macos")]
pub(crate) fn read_macos_request(mut input: impl Read) -> Result<Vec<u8>, CuError> {
    read_frame_body(&mut input, MAX_REQUEST_BYTES, "request")
}

#[cfg(target_os = "macos")]
pub(crate) fn write_macos_server_disposition(
    mut output: impl Write,
    disposition: MacosServerDisposition,
) -> Result<(), CuError> {
    let tag = match disposition {
        MacosServerDisposition::ReplyReady => SERVER_REPLY_READY,
        MacosServerDisposition::ConsentRequired => SERVER_CONSENT_REQUIRED,
    };
    write_tag(&mut output, tag, "server-disposition")
}

#[cfg(target_os = "macos")]
pub(crate) fn read_macos_server_disposition(
    mut input: impl Read,
) -> Result<MacosServerDisposition, CuError> {
    match read_tag(&mut input, "server-disposition")? {
        SERVER_REPLY_READY => Ok(MacosServerDisposition::ReplyReady),
        SERVER_CONSENT_REQUIRED => Ok(MacosServerDisposition::ConsentRequired),
        _ => Err(CuError::new(
            "privilege_server_disposition_invalid",
            "macOS privilege broker returned an unknown disposition",
        )),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn write_macos_authorization_proof(
    mut output: impl Write,
    proof: &mut agenterm_platform::privilege_authorization::MacosAuthorizationProof,
) -> Result<(), CuError> {
    write_tag(&mut output, CLIENT_AUTHORIZATION_PROOF, "client-consent")?;
    proof.write_to(&mut output).map_err(|_| {
        CuError::new(
            "privilege_proof_transport_incomplete",
            "macOS authorization proof could not be written completely",
        )
    })?;
    output.flush().map_err(|_| wire_error("proof", "write"))
}

#[cfg(target_os = "macos")]
pub(crate) fn write_macos_client_consent(
    mut output: impl Write,
    consent: MacosClientConsent,
) -> Result<(), CuError> {
    let tag = match consent {
        MacosClientConsent::Canceled => CLIENT_CONSENT_CANCELED,
        MacosClientConsent::Denied => CLIENT_CONSENT_DENIED,
        MacosClientConsent::TimedOut => CLIENT_CONSENT_TIMED_OUT,
        MacosClientConsent::Failed => CLIENT_CONSENT_FAILED,
    };
    write_tag(&mut output, tag, "client-consent")
}

#[cfg(target_os = "macos")]
pub(crate) fn read_macos_broker_consent(
    mut input: impl Read,
) -> Result<MacosBrokerConsent, CuError> {
    let consent = match read_tag(&mut input, "client-consent")? {
        CLIENT_AUTHORIZATION_PROOF => {
            let proof =
                agenterm_platform::privilege_authorization::MacosAuthorizationProof::read_from(
                    &mut input,
                )
                .map_err(|_| {
                    CuError::new(
                        "privilege_proof_invalid",
                        "macOS privilege broker received an invalid authorization proof",
                    )
                })?;
            MacosBrokerConsent::Proof(proof)
        }
        CLIENT_CONSENT_CANCELED => MacosBrokerConsent::Canceled,
        CLIENT_CONSENT_DENIED => MacosBrokerConsent::Denied,
        CLIENT_CONSENT_TIMED_OUT => MacosBrokerConsent::TimedOut,
        CLIENT_CONSENT_FAILED => MacosBrokerConsent::Failed,
        _ => {
            return Err(CuError::new(
                "privilege_client_consent_invalid",
                "macOS privilege client returned an unknown consent disposition",
            ));
        }
    };
    require_eof(&mut input, "consent")?;
    Ok(consent)
}

pub(crate) fn read_reply(mut input: impl Read) -> Result<PrivilegeApplyReplyV1, CuError> {
    let bytes = read_frame(&mut input, MAX_REPLY_BYTES, "reply")?;
    parse_apply_reply(&bytes)
}

pub(crate) fn write_reply(
    mut output: impl Write,
    reply: &PrivilegeApplyReplyV1,
) -> Result<(), CuError> {
    let bytes = serde_json::to_vec(reply).map_err(|_| {
        CuError::new(
            "privilege_reply_invalid",
            "privilege broker reply could not be serialized",
        )
    })?;
    write_frame(&mut output, &bytes, MAX_REPLY_BYTES, "reply")
}

fn read_frame(
    input: &mut impl Read,
    ceiling: usize,
    kind: &'static str,
) -> Result<Vec<u8>, CuError> {
    let bytes = read_frame_body(input, ceiling, kind)?;
    require_eof(input, kind)?;
    Ok(bytes)
}

fn read_frame_body(
    input: &mut impl Read,
    ceiling: usize,
    kind: &'static str,
) -> Result<Vec<u8>, CuError> {
    let mut length = [0_u8; LENGTH_BYTES];
    input
        .read_exact(&mut length)
        .map_err(|_| wire_error(kind, "header"))?;
    let length =
        usize::try_from(u32::from_be_bytes(length)).map_err(|_| wire_error(kind, "length"))?;
    if length == 0 || length > ceiling {
        return Err(CuError::new(
            format!("privilege_{kind}_size_invalid"),
            format!("privilege broker {kind} frame is empty or exceeds its byte budget"),
        ));
    }
    let mut bytes = vec![0_u8; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| wire_error(kind, "body"))?;
    Ok(bytes)
}

fn require_eof(input: &mut impl Read, kind: &'static str) -> Result<(), CuError> {
    let mut trailing = [0_u8; 1];
    match input.read(&mut trailing) {
        Ok(0) => Ok(()),
        Ok(_) => Err(CuError::new(
            format!("privilege_{kind}_trailing_bytes"),
            format!("privilege broker {kind} connection carried more than one frame"),
        )),
        // A read deadline is not EOF. Accepting WouldBlock here would let a
        // peer retain the write half and append a second consent/proof after
        // the broker had already admitted the first effect.
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            Err(wire_error(kind, "trailer-timeout"))
        }
        Err(_) => Err(wire_error(kind, "trailer")),
    }
}

#[cfg(target_os = "macos")]
fn write_tag(output: &mut impl Write, tag: u8, kind: &'static str) -> Result<(), CuError> {
    output
        .write_all(&[tag])
        .and_then(|()| output.flush())
        .map_err(|_| wire_error(kind, "write"))
}

#[cfg(target_os = "macos")]
fn read_tag(input: &mut impl Read, kind: &'static str) -> Result<u8, CuError> {
    let mut tag = [0_u8; 1];
    input
        .read_exact(&mut tag)
        .map_err(|_| wire_error(kind, "read"))?;
    Ok(tag[0])
}

fn write_frame(
    output: &mut impl Write,
    bytes: &[u8],
    ceiling: usize,
    kind: &'static str,
) -> Result<(), CuError> {
    if bytes.is_empty() || bytes.len() > ceiling || bytes.len() > u32::MAX as usize {
        return Err(CuError::new(
            format!("privilege_{kind}_size_invalid"),
            format!("privilege broker {kind} frame is empty or exceeds its byte budget"),
        ));
    }
    output
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .and_then(|()| output.write_all(bytes))
        .and_then(|()| output.flush())
        .map_err(|_| wire_error(kind, "write"))
}

fn wire_error(kind: &str, stage: &str) -> CuError {
    CuError::new(
        format!("privilege_{kind}_transport_incomplete"),
        format!("privilege broker {kind} {stage} was incomplete"),
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;

    #[test]
    fn request_frame_is_big_endian_bounded_and_exactly_one() {
        let payload = br#"{"schema":1}"#;
        let mut encoded = Vec::new();
        write_request(&mut encoded, payload).unwrap();
        assert_eq!(&encoded[..4], &(payload.len() as u32).to_be_bytes());
        assert_eq!(read_request(Cursor::new(&encoded)).unwrap(), payload);

        encoded.push(0);
        assert_eq!(
            read_request(Cursor::new(&encoded)).unwrap_err().code,
            "privilege_request_trailing_bytes"
        );
    }

    #[test]
    fn empty_oversized_and_truncated_frames_fail_closed() {
        assert_eq!(
            read_request(Cursor::new(0_u32.to_be_bytes()))
                .unwrap_err()
                .code,
            "privilege_request_size_invalid"
        );
        assert_eq!(
            read_request(Cursor::new(
                u32::try_from(MAX_REQUEST_BYTES + 1).unwrap().to_be_bytes()
            ))
            .unwrap_err()
            .code,
            "privilege_request_size_invalid"
        );
        let mut truncated = Vec::from(8_u32.to_be_bytes());
        truncated.extend_from_slice(b"short");
        assert_eq!(
            read_request(Cursor::new(truncated)).unwrap_err().code,
            "privilege_request_transport_incomplete"
        );
    }

    #[test]
    fn writer_propagates_short_transport_failure() {
        struct ShortWriter;
        impl Write for ShortWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "closed",
                ))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_request(ShortWriter, b"x").unwrap_err().code,
            "privilege_request_transport_incomplete"
        );
    }

    #[test]
    fn reply_round_trip_reuses_the_closed_reply_decoder() {
        let reply = PrivilegeApplyReplyV1::ConsentCanceled {
            protocol_version: crate::privilege_apply::PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "request-1".into(),
            contract_digest: "a".repeat(64),
            approval_digest: "b".repeat(64),
        };
        let mut encoded = Vec::new();
        write_reply(&mut encoded, &reply).unwrap();
        assert_eq!(read_reply(Cursor::new(encoded)).unwrap(), reply);
    }

    #[test]
    fn reader_does_not_consume_unbounded_input() {
        struct PanicAfterHeader {
            header: Cursor<[u8; 4]>,
        }
        impl Read for PanicAfterHeader {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                self.header.read(output)
            }
        }
        let error = read_request(PanicAfterHeader {
            header: Cursor::new(u32::MAX.to_be_bytes()),
        })
        .unwrap_err();
        assert_eq!(error.code, "privilege_request_size_invalid");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_request_phase_keeps_the_authenticated_stream_open() {
        let payload = br#"{"schema":1}"#;
        let mut encoded = Vec::new();
        write_request(&mut encoded, payload).unwrap();
        encoded.push(CLIENT_CONSENT_CANCELED);
        let mut cursor = Cursor::new(encoded);
        assert_eq!(read_macos_request(&mut cursor).unwrap(), payload);
        assert!(matches!(
            read_macos_broker_consent(&mut cursor).unwrap(),
            MacosBrokerConsent::Canceled
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_dispositions_are_closed_and_unknown_tags_fail() {
        for disposition in [
            MacosServerDisposition::ReplyReady,
            MacosServerDisposition::ConsentRequired,
        ] {
            let mut encoded = Vec::new();
            write_macos_server_disposition(&mut encoded, disposition).unwrap();
            assert_eq!(
                read_macos_server_disposition(Cursor::new(encoded)).unwrap(),
                disposition
            );
        }
        assert_eq!(
            read_macos_server_disposition(Cursor::new([255]))
                .unwrap_err()
                .code,
            "privilege_server_disposition_invalid"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_consent_reply_rejects_trailing_or_unknown_data() {
        let mut canceled = Vec::new();
        write_macos_client_consent(&mut canceled, MacosClientConsent::Canceled).unwrap();
        canceled.push(0);
        assert_eq!(
            match read_macos_broker_consent(Cursor::new(canceled)) {
                Ok(_) => panic!("trailing consent data must fail"),
                Err(error) => error.code,
            },
            "privilege_consent_trailing_bytes"
        );
        assert_eq!(
            match read_macos_broker_consent(Cursor::new([255])) {
                Ok(_) => panic!("unknown consent tag must fail"),
                Err(error) => error.code,
            },
            "privilege_client_consent_invalid"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_client_consent_variants_round_trip_without_proof_bytes() {
        for (client, expected) in [
            (MacosClientConsent::Canceled, "canceled"),
            (MacosClientConsent::Denied, "denied"),
            (MacosClientConsent::TimedOut, "timed-out"),
            (MacosClientConsent::Failed, "failed"),
        ] {
            let mut encoded = Vec::new();
            write_macos_client_consent(&mut encoded, client).unwrap();
            let actual = match read_macos_broker_consent(Cursor::new(encoded)).unwrap() {
                MacosBrokerConsent::Canceled => "canceled",
                MacosBrokerConsent::Denied => "denied",
                MacosBrokerConsent::TimedOut => "timed-out",
                MacosBrokerConsent::Failed => "failed",
                MacosBrokerConsent::Proof(_) => panic!("status reply cannot become a proof"),
            };
            assert_eq!(actual, expected);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_proof_wire_is_fixed_and_has_no_serializable_envelope() {
        use agenterm_platform::privilege_authorization::MacosAuthorizationProof;

        let bytes = [9_u8; MacosAuthorizationProof::WIRE_LENGTH];
        let mut proof = MacosAuthorizationProof::read_from(&mut bytes.as_slice()).unwrap();
        let mut encoded = Vec::new();
        write_macos_authorization_proof(&mut encoded, &mut proof).unwrap();
        assert_eq!(encoded.len(), 1 + MacosAuthorizationProof::WIRE_LENGTH);
        assert!(matches!(
            read_macos_broker_consent(Cursor::new(encoded)).unwrap(),
            MacosBrokerConsent::Proof(_)
        ));
    }
}
