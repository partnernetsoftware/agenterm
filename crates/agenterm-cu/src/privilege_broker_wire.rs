//! Bounded one-request-per-connection framing for the system privilege broker.

use std::io::{Read, Write};

use crate::{
    CuError,
    privilege_apply::{
        MAX_REPLY_BYTES, MAX_REQUEST_BYTES, PrivilegeApplyReplyV1, parse_apply_reply,
    },
};

const LENGTH_BYTES: usize = 4;

pub(crate) fn read_request(mut input: impl Read) -> Result<Vec<u8>, CuError> {
    read_frame(&mut input, MAX_REQUEST_BYTES, "request")
}

pub(crate) fn write_request(mut output: impl Write, bytes: &[u8]) -> Result<(), CuError> {
    write_frame(&mut output, bytes, MAX_REQUEST_BYTES, "request")
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
    let mut trailing = [0_u8; 1];
    match input.read(&mut trailing) {
        Ok(0) => Ok(bytes),
        Ok(_) => Err(CuError::new(
            format!("privilege_{kind}_trailing_bytes"),
            format!("privilege broker {kind} connection carried more than one frame"),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(bytes),
        Err(_) => Err(wire_error(kind, "trailer")),
    }
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
}
