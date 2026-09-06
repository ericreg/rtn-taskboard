use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 768 * 1024;
pub const BODY_CHUNK_BYTES: usize = 512 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_PATH_BYTES: usize = 8 * 1024;
pub const REQUEST_TOPIC: &str = "taskboard/http/request";
pub const RESPONSE_TOPIC: &str = "taskboard/http/response";
pub const FRAME_FORMAT: &str = "application/vnd.taskboard.tunnel-v1+postcard";

pub type RequestId = [u8; 16];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    pub version: u16,
    pub frame: Frame,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Frame {
    RequestStart {
        request_id: RequestId,
        method: String,
        path_and_query: String,
        headers: Vec<Header>,
        client_ip: String,
    },
    RequestChunk {
        request_id: RequestId,
        sequence: u64,
        bytes: Vec<u8>,
    },
    RequestEnd {
        request_id: RequestId,
        chunks: u64,
    },
    RequestCancel {
        request_id: RequestId,
    },
    ResponseStart {
        request_id: RequestId,
        status: u16,
        headers: Vec<Header>,
    },
    ResponseChunk {
        request_id: RequestId,
        sequence: u64,
        bytes: Vec<u8>,
    },
    ResponseEnd {
        request_id: RequestId,
        chunks: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid tunnel frame")]
    Invalid,
    #[error("tunnel frame exceeds its limit")]
    TooLarge,
}

impl Envelope {
    pub fn new(frame: Frame) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            frame,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let bytes = postcard::to_stdvec(self).map_err(|_| Error::Invalid)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::TooLarge);
        }
        let envelope: Self = postcard::from_bytes(bytes).map_err(|_| Error::Invalid)?;
        envelope.validate()?;
        if envelope.encode()?.as_slice() != bytes {
            return Err(Error::Invalid);
        }
        Ok(envelope)
    }

    pub fn request_id(&self) -> RequestId {
        match &self.frame {
            Frame::RequestStart { request_id, .. }
            | Frame::RequestChunk { request_id, .. }
            | Frame::RequestEnd { request_id, .. }
            | Frame::RequestCancel { request_id }
            | Frame::ResponseStart { request_id, .. }
            | Frame::ResponseChunk { request_id, .. }
            | Frame::ResponseEnd { request_id, .. } => *request_id,
        }
    }

    fn validate(&self) -> Result<(), Error> {
        if self.version != PROTOCOL_VERSION {
            return Err(Error::Invalid);
        }
        match &self.frame {
            Frame::RequestStart {
                method,
                path_and_query,
                headers,
                client_ip,
                ..
            } => {
                if method.is_empty()
                    || method.len() > 16
                    || !method.bytes().all(|byte| byte.is_ascii_uppercase())
                    || !path_and_query.starts_with("/api/v1")
                    || path_and_query.len() > MAX_PATH_BYTES
                    || client_ip.len() > 64
                {
                    return Err(Error::Invalid);
                }
                validate_headers(headers)
            }
            Frame::RequestChunk { bytes, .. } | Frame::ResponseChunk { bytes, .. } => {
                if bytes.is_empty() || bytes.len() > BODY_CHUNK_BYTES {
                    Err(Error::TooLarge)
                } else {
                    Ok(())
                }
            }
            Frame::ResponseStart {
                status, headers, ..
            } => {
                if !(100..=599).contains(status) {
                    return Err(Error::Invalid);
                }
                validate_headers(headers)
            }
            Frame::RequestEnd { .. } | Frame::RequestCancel { .. } | Frame::ResponseEnd { .. } => {
                Ok(())
            }
        }
    }
}

fn validate_headers(headers: &[Header]) -> Result<(), Error> {
    if headers.len() > MAX_HEADERS
        || headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
            > MAX_HEADER_BYTES
        || headers.iter().any(|header| {
            header.name.is_empty()
                || header.name.len() > 128
                || !header
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
                || header.value.len() > 16 * 1024
        })
    {
        Err(Error::TooLarge)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_reject_invalid_input() {
        let envelope = Envelope::new(Frame::RequestChunk {
            request_id: [7; 16],
            sequence: 3,
            bytes: vec![1, 2, 3],
        });
        let bytes = envelope.encode().unwrap();
        assert_eq!(Envelope::decode(&bytes).unwrap(), envelope);
        assert!(Envelope::decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(
            Envelope::new(Frame::RequestChunk {
                request_id: [0; 16],
                sequence: 0,
                bytes: vec![0; BODY_CHUNK_BYTES + 1],
            })
            .encode()
            .is_err()
        );
    }
}
