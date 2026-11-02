use bytes::{Buf, BufMut, Bytes, BytesMut};
use kv_types::ChunkKey;
use thiserror::Error;

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BATCH_ITEMS: usize = 65_536;

const OP_LOOKUP: u8 = 1;
const OP_PUT: u8 = 2;
const OP_GET: u8 = 3;
const OP_DELETE: u8 = 4;
const OP_STATS: u8 = 5;
const OP_HITS: u8 = 0x81;
const OP_OK: u8 = 0x82;
const OP_CHUNKS: u8 = 0x83;
const OP_DELETED: u8 = 0x84;
const OP_STATS_RESPONSE: u8 = 0x85;
const OP_ERROR: u8 = 0xff;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("truncated message")]
    Truncated,
    #[error("unknown operation {0:#x}")]
    UnknownOperation(u8),
    #[error("invalid value {0}")]
    InvalidValue(&'static str),
    #[error("message exceeds configured limits")]
    LimitExceeded,
    #[error("invalid UTF-8 error message")]
    InvalidUtf8,
    #[error("trailing bytes after message")]
    TrailingBytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Lookup(Vec<ChunkKey>),
    Put(Vec<(ChunkKey, Bytes)>),
    Get(Vec<ChunkKey>),
    Delete(Vec<ChunkKey>),
    Stats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Hits(u32),
    Ok,
    Chunks(Vec<Option<Bytes>>),
    Deleted(u32),
    Stats { used_bytes: u64 },
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestFrame {
    pub request_id: u64,
    pub request: Request,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseFrame {
    pub request_id: u64,
    pub response: Response,
}

fn check_count(count: usize) -> Result<u32, ProtoError> {
    if count > MAX_BATCH_ITEMS {
        return Err(ProtoError::LimitExceeded);
    }
    u32::try_from(count).map_err(|_| ProtoError::LimitExceeded)
}

fn put_blob(buf: &mut BytesMut, value: &[u8]) -> Result<(), ProtoError> {
    let len = u32::try_from(value.len()).map_err(|_| ProtoError::LimitExceeded)?;
    let new_len = buf
        .len()
        .checked_add(4)
        .and_then(|current| current.checked_add(value.len()))
        .ok_or(ProtoError::LimitExceeded)?;
    if new_len > MAX_FRAME_BYTES {
        return Err(ProtoError::LimitExceeded);
    }
    buf.put_u32(len);
    buf.extend_from_slice(value);
    Ok(())
}

fn take_u32(buf: &mut Bytes) -> Result<u32, ProtoError> {
    if buf.remaining() < 4 {
        return Err(ProtoError::Truncated);
    }
    Ok(buf.get_u32())
}

fn take_u64(buf: &mut Bytes) -> Result<u64, ProtoError> {
    if buf.remaining() < 8 {
        return Err(ProtoError::Truncated);
    }
    Ok(buf.get_u64())
}

fn take_key(buf: &mut Bytes) -> Result<ChunkKey, ProtoError> {
    if buf.remaining() < ChunkKey::LENGTH {
        return Err(ProtoError::Truncated);
    }
    let mut bytes = [0; ChunkKey::LENGTH];
    buf.copy_to_slice(&mut bytes);
    Ok(ChunkKey::from_bytes(bytes))
}

fn take_blob(buf: &mut Bytes) -> Result<Bytes, ProtoError> {
    let len = take_u32(buf)? as usize;
    if len > buf.remaining() {
        return Err(ProtoError::Truncated);
    }
    Ok(buf.split_to(len))
}

fn take_count(buf: &mut Bytes) -> Result<usize, ProtoError> {
    let count = take_u32(buf)? as usize;
    if count > MAX_BATCH_ITEMS {
        return Err(ProtoError::LimitExceeded);
    }
    Ok(count)
}

fn finish(buf: Bytes) -> Result<(), ProtoError> {
    if buf.has_remaining() {
        Err(ProtoError::TrailingBytes)
    } else {
        Ok(())
    }
}

impl RequestFrame {
    pub fn encode(&self) -> Result<Bytes, ProtoError> {
        let mut buf = BytesMut::new();
        match &self.request {
            Request::Lookup(keys) | Request::Get(keys) | Request::Delete(keys) => {
                let op = match &self.request {
                    Request::Lookup(_) => OP_LOOKUP,
                    Request::Get(_) => OP_GET,
                    Request::Delete(_) => OP_DELETE,
                    _ => unreachable!(),
                };
                buf.put_u8(op);
                buf.put_u64(self.request_id);
                buf.put_u32(check_count(keys.len())?);
                for key in keys {
                    buf.extend_from_slice(key.as_bytes());
                }
            }
            Request::Put(entries) => {
                buf.put_u8(OP_PUT);
                buf.put_u64(self.request_id);
                buf.put_u32(check_count(entries.len())?);
                for (key, value) in entries {
                    buf.extend_from_slice(key.as_bytes());
                    put_blob(&mut buf, value)?;
                }
            }
            Request::Stats => {
                buf.put_u8(OP_STATS);
                buf.put_u64(self.request_id);
            }
        }
        if buf.len() > MAX_FRAME_BYTES {
            return Err(ProtoError::LimitExceeded);
        }
        Ok(buf.freeze())
    }

    pub fn decode(mut buf: Bytes) -> Result<Self, ProtoError> {
        if buf.len() > MAX_FRAME_BYTES {
            return Err(ProtoError::LimitExceeded);
        }
        if !buf.has_remaining() {
            return Err(ProtoError::Truncated);
        }
        let op = buf.get_u8();
        let request_id = take_u64(&mut buf)?;
        let request = match op {
            OP_LOOKUP | OP_GET | OP_DELETE => {
                let count = take_count(&mut buf)?;
                if count > buf.remaining() / ChunkKey::LENGTH {
                    return Err(ProtoError::Truncated);
                }
                let mut keys = Vec::with_capacity(count);
                for _ in 0..count {
                    keys.push(take_key(&mut buf)?);
                }
                match op {
                    OP_LOOKUP => Request::Lookup(keys),
                    OP_GET => Request::Get(keys),
                    _ => Request::Delete(keys),
                }
            }
            OP_PUT => {
                let count = take_count(&mut buf)?;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    entries.push((take_key(&mut buf)?, take_blob(&mut buf)?));
                }
                Request::Put(entries)
            }
            OP_STATS => Request::Stats,
            other => return Err(ProtoError::UnknownOperation(other)),
        };
        finish(buf)?;
        Ok(Self {
            request_id,
            request,
        })
    }
}

impl ResponseFrame {
    pub fn encode(&self) -> Result<Bytes, ProtoError> {
        let mut buf = BytesMut::new();
        match &self.response {
            Response::Hits(count) => {
                buf.put_u8(OP_HITS);
                buf.put_u64(self.request_id);
                buf.put_u32(*count);
            }
            Response::Ok => {
                buf.put_u8(OP_OK);
                buf.put_u64(self.request_id);
            }
            Response::Chunks(chunks) => {
                buf.put_u8(OP_CHUNKS);
                buf.put_u64(self.request_id);
                buf.put_u32(check_count(chunks.len())?);
                for chunk in chunks {
                    match chunk {
                        Some(value) => {
                            buf.put_u8(1);
                            put_blob(&mut buf, value)?;
                        }
                        None => buf.put_u8(0),
                    }
                }
            }
            Response::Deleted(count) => {
                buf.put_u8(OP_DELETED);
                buf.put_u64(self.request_id);
                buf.put_u32(*count);
            }
            Response::Stats { used_bytes } => {
                buf.put_u8(OP_STATS_RESPONSE);
                buf.put_u64(self.request_id);
                buf.put_u64(*used_bytes);
            }
            Response::Error(message) => {
                buf.put_u8(OP_ERROR);
                buf.put_u64(self.request_id);
                put_blob(&mut buf, message.as_bytes())?;
            }
        }
        if buf.len() > MAX_FRAME_BYTES {
            return Err(ProtoError::LimitExceeded);
        }
        Ok(buf.freeze())
    }

    pub fn decode(mut buf: Bytes) -> Result<Self, ProtoError> {
        if buf.len() > MAX_FRAME_BYTES {
            return Err(ProtoError::LimitExceeded);
        }
        if !buf.has_remaining() {
            return Err(ProtoError::Truncated);
        }
        let op = buf.get_u8();
        let request_id = take_u64(&mut buf)?;
        let response = match op {
            OP_HITS => Response::Hits(take_u32(&mut buf)?),
            OP_OK => Response::Ok,
            OP_CHUNKS => {
                let count = take_count(&mut buf)?;
                let mut chunks = Vec::with_capacity(count);
                for _ in 0..count {
                    if !buf.has_remaining() {
                        return Err(ProtoError::Truncated);
                    }
                    chunks.push(match buf.get_u8() {
                        0 => None,
                        1 => Some(take_blob(&mut buf)?),
                        _ => return Err(ProtoError::InvalidValue("chunk presence flag")),
                    });
                }
                Response::Chunks(chunks)
            }
            OP_DELETED => Response::Deleted(take_u32(&mut buf)?),
            OP_STATS_RESPONSE => Response::Stats {
                used_bytes: take_u64(&mut buf)?,
            },
            OP_ERROR => {
                let message = take_blob(&mut buf)?;
                Response::Error(
                    String::from_utf8(message.to_vec()).map_err(|_| ProtoError::InvalidUtf8)?,
                )
            }
            other => return Err(ProtoError::UnknownOperation(other)),
        };
        finish(buf)?;
        Ok(Self {
            request_id,
            response,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_preserves_id_and_payload() {
        let frame = RequestFrame {
            request_id: 42,
            request: Request::Put(vec![(
                ChunkKey::from_bytes([7; ChunkKey::LENGTH]),
                Bytes::from_static(b"hello"),
            )]),
        };
        assert_eq!(RequestFrame::decode(frame.encode().unwrap()).unwrap(), frame);
    }

    #[test]
    fn response_roundtrip_preserves_misses_and_empty_values() {
        let frame = ResponseFrame {
            request_id: 11,
            response: Response::Chunks(vec![None, Some(Bytes::new())]),
        };
        assert_eq!(ResponseFrame::decode(frame.encode().unwrap()).unwrap(), frame);
    }

    #[test]
    fn rejects_trailing_bytes_and_invalid_presence_flag() {
        let mut encoded = ResponseFrame {
            request_id: 1,
            response: Response::Ok,
        }
        .encode()
        .unwrap()
        .to_vec();
        encoded.push(0);
        assert!(matches!(
            ResponseFrame::decode(Bytes::from(encoded)),
            Err(ProtoError::TrailingBytes)
        ));

        let mut invalid = BytesMut::new();
        invalid.put_u8(OP_CHUNKS);
        invalid.put_u64(1);
        invalid.put_u32(1);
        invalid.put_u8(2);
        assert!(matches!(
            ResponseFrame::decode(invalid.freeze()),
            Err(ProtoError::InvalidValue(_))
        ));
    }

    #[test]
    fn rejects_unbounded_batch_count_before_allocating() {
        let mut encoded = BytesMut::new();
        encoded.put_u8(OP_LOOKUP);
        encoded.put_u64(1);
        encoded.put_u32((MAX_BATCH_ITEMS as u32) + 1);
        assert!(matches!(
            RequestFrame::decode(encoded.freeze()),
            Err(ProtoError::LimitExceeded)
        ));
    }
}
