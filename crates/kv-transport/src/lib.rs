use bytes::Bytes;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    if body.is_empty() || body.len() > kv_proto::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid frame length",
        ));
    }
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame length overflow"))?;
    writer.write_u32(len).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

/// `Ok(None)` means EOF before any header byte; a partial header is an error.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Bytes>> {
    let mut header = [0; 4];
    match reader.read(&mut header[..1]).await? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    reader.read_exact(&mut header[1..]).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > kv_proto::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).await?;
    Ok(Some(Bytes::from(body)))
}

#[cfg(test)]
mod tests {
    use super::read_frame;

    #[tokio::test]
    async fn clean_eof_and_partial_frames_are_distinguished() {
        let mut clean = &b""[..];
        assert!(read_frame(&mut clean).await.unwrap().is_none());

        let mut partial_header = &b"\x00"[..];
        assert!(read_frame(&mut partial_header).await.is_err());

        let mut partial_body = &b"\x00\x00\x00\x02\x01"[..];
        assert!(read_frame(&mut partial_body).await.is_err());
    }
}

