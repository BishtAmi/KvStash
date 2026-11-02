use kv_core::{MemoryBackend, StorageBackend};
use kv_proto::{Request, RequestFrame, Response, ResponseFrame};
use kv_transport::{read_frame, write_frame};
use std::io;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

fn handle(
    store: &MemoryBackend,
    request: Request,
) -> Result<Response, kv_core::BackendError> {
    match request {
        Request::Lookup(keys) => {
            let mut hits = 0u32;
            for key in keys {
                if store.contains(&key)? {
                    hits += 1;
                } else {
                    break;
                }
            }
            Ok(Response::Hits(hits))
        }
        Request::Put(entries) => {
            for (key, value) in entries {
                store.put(key, value)?;
            }
            Ok(Response::Ok)
        }
        Request::Get(keys) => {
            let chunks = keys
                .iter()
                .map(|key| store.get(key))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Response::Chunks(chunks))
        }
        Request::Delete(keys) => {
            let mut deleted = 0u32;
            for key in keys {
                if store.delete(&key)? {
                    deleted += 1;
                }
            }
            Ok(Response::Deleted(deleted))
        }
        Request::Stats => Ok(Response::Stats {
            used_bytes: store.used_bytes()?,
        }),
    }
}

async fn serve_connection(
    mut socket: TcpStream,
    store: Arc<MemoryBackend>,
) -> io::Result<()> {
    while let Some(body) = read_frame(&mut socket).await? {
        let request = match RequestFrame::decode(body) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!("invalid request frame: {error}");
                return Err(io::Error::new(io::ErrorKind::InvalidData, error));
            }
        };

        let request_id = request.request_id;
        let response = match handle(&store, request.request) {
            Ok(response) => response,
            Err(error) => Response::Error(error.to_string()),
        };
        let body = ResponseFrame {
            request_id,
            response,
        }
        .encode()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_frame(&mut socket, &body).await?;
    }
    Ok(())
}

fn parse_capacity() -> Result<u64, String> {
    let mut args = std::env::args().skip(1);
    let mut capacity = 1_u64 << 30;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--capacity-bytes" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--capacity-bytes requires an integer".to_owned())?;
                capacity = value
                    .parse()
                    .map_err(|_| format!("invalid capacity in bytes: {value}"))?;
            }
            "--help" | "-h" => {
                println!("kv-server [--capacity-bytes N]\nDefault capacity: 1 GiB");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(capacity)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let capacity = parse_capacity().map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidInput, error)
    })?;
    let store = Arc::new(MemoryBackend::new(capacity));
    let listener = TcpListener::bind("127.0.0.1:7070").await?;
    tracing::info!(
        address = "127.0.0.1:7070",
        capacity_bytes = capacity,
        "kv-server listening"
    );

    loop {
        let (socket, address) = listener.accept().await?;
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            if let Err(error) = serve_connection(socket, store).await {
                tracing::warn!(%address, %error, "connection ended");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::handle;
    use bytes::Bytes;
    use kv_core::{MemoryBackend, StorageBackend};
    use kv_proto::{Request, Response};
    use kv_types::ChunkKey;

    #[test]
    fn lookup_reports_only_the_leading_run_of_hits() {
        let store = MemoryBackend::new(10);
        let keys = [
            ChunkKey::from_bytes([1; 16]),
            ChunkKey::from_bytes([2; 16]),
            ChunkKey::from_bytes([3; 16]),
        ];
        store.put(keys[0], Bytes::new()).unwrap();
        store.put(keys[1], Bytes::new()).unwrap();
        store.put(keys[2], Bytes::new()).unwrap();
        store.delete(&keys[1]).unwrap();

        assert_eq!(
            handle(&store, Request::Lookup(keys.to_vec())).unwrap(),
            Response::Hits(1)
        );
    }
}
