use bytes::Bytes;
use kv_proto::{ProtoError, Request, RequestFrame, Response, ResponseFrame};
use kv_transport::{read_frame, write_frame};
use kv_types::ChunkKey;
use std::io;
use tokio::net::{TcpStream, ToSocketAddrs};

pub struct Client {
    socket: TcpStream,
    next_request_id: u64,
}

impl Client {
    pub async fn connect(address: impl ToSocketAddrs) -> io::Result<Self> {
        Ok(Self {
            socket: TcpStream::connect(address).await?,
            next_request_id: 1,
        })
    }

    async fn call(&mut self, request: Request) -> io::Result<Response> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("request ID space exhausted"))?;
        let body = RequestFrame {
            request_id,
            request,
        }
        .encode()
        .map_err(invalid_data)?;
        write_frame(&mut self.socket, &body).await?;
        let body = read_frame(&mut self.socket)
            .await?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "server closed"))?;
        let frame = ResponseFrame::decode(body).map_err(invalid_data)?;
        if frame.request_id != request_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "response ID mismatch: expected {request_id}, got {}",
                    frame.request_id
                ),
            ));
        }
        match frame.response {
            Response::Error(message) => Err(io::Error::other(message)),
            response => Ok(response),
        }
    }

    pub async fn lookup(&mut self, keys: Vec<ChunkKey>) -> io::Result<u32> {
        match self.call(Request::Lookup(keys)).await? {
            Response::Hits(count) => Ok(count),
            other => Err(unexpected_response(other)),
        }
    }

    pub async fn put(&mut self, entries: Vec<(ChunkKey, Bytes)>) -> io::Result<()> {
        match self.call(Request::Put(entries)).await? {
            Response::Ok => Ok(()),
            other => Err(unexpected_response(other)),
        }
    }

    pub async fn get(&mut self, keys: Vec<ChunkKey>) -> io::Result<Vec<Option<Bytes>>> {
        match self.call(Request::Get(keys)).await? {
            Response::Chunks(chunks) => Ok(chunks),
            other => Err(unexpected_response(other)),
        }
    }

    pub async fn delete(&mut self, keys: Vec<ChunkKey>) -> io::Result<u32> {
        match self.call(Request::Delete(keys)).await? {
            Response::Deleted(count) => Ok(count),
            other => Err(unexpected_response(other)),
        }
    }

    pub async fn used_bytes(&mut self) -> io::Result<u64> {
        match self.call(Request::Stats).await? {
            Response::Stats { used_bytes } => Ok(used_bytes),
            other => Err(unexpected_response(other)),
        }
    }
}

fn invalid_data(error: ProtoError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn unexpected_response(response: Response) -> io::Error {
    io::Error::other(format!("unexpected server response: {response:?}"))
}
