use std::future::Future;
use std::time::Duration;

use tokio::net::{
    UnixListener,
    UnixStream,
};
use tokio::task::JoinSet;

/// The listener owns every connection task. Dropping it closes its clients as well.
/// Handlers supply protocol-specific handshake and partial-message deadlines.
pub async fn serve_connections<F, H>(listener: UnixListener, capacity: usize, uid: u32, handler: H) -> !
where
    H: Fn(UnixStream) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            result = clients.join_next(), if !clients.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::warn!(%error, "IPC connection handler stopped unexpectedly");
                }
            }
            incoming = listener.accept() => {
                match incoming {
                    Ok((stream, _)) => {
                        if clients.len() < capacity && stream.peer_cred().is_ok_and(|peer| peer.uid() == uid) {
                            clients.spawn(handler(stream));
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "IPC accept failed; retrying");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use tokio::io::{
        AsyncReadExt,
        AsyncWriteExt,
    };

    use super::*;

    #[tokio::test]
    async fn limits_clients_reuses_capacity_and_closes_clients_with_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ipc.sock");
        let uid = directory.path().metadata().unwrap().uid();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(serve_connections(listener, 2, uid, |mut client| async move {
            client.write_all(b"!").await.unwrap();
            let _ = client.read_u8().await;
        }));
        let mut first = UnixStream::connect(&path).await.unwrap();
        assert_eq!(first.read_u8().await.unwrap(), b'!');
        let mut second = UnixStream::connect(&path).await.unwrap();
        assert_eq!(second.read_u8().await.unwrap(), b'!');
        let mut excess = UnixStream::connect(&path).await.unwrap();
        assert!(excess.read_u8().await.is_err());
        drop(first);
        let mut replacement = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let mut stream = UnixStream::connect(&path).await.unwrap();
                if stream.read_u8().await.is_ok() {
                    break stream;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        server.abort();
        let _ = server.await;
        assert!(second.read_u8().await.is_err());
        assert!(replacement.read_u8().await.is_err());
    }

    #[tokio::test]
    async fn rejects_foreign_uid_before_dispatch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ipc.sock");
        let uid = directory.path().metadata().unwrap().uid();
        let server = tokio::spawn(serve_connections(
            UnixListener::bind(&path).unwrap(),
            1,
            uid.wrapping_add(1),
            |_| async {
                panic!("foreign peer dispatched");
            },
        ));
        let mut client = UnixStream::connect(path).await.unwrap();
        assert!(client.read_u8().await.is_err());
        server.abort();
        let _ = server.await;
    }
}
