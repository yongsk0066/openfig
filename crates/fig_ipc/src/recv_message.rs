use std::io;

use async_trait::async_trait;
use fig_proto::prost::Message;
use fig_proto::{
    FigMessage,
    ReflectMessage,
};
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
};

use crate::BufferedReader;
use crate::error::RecvError;

/// Spare capacity ensured before every `read_buf` call.
const READ_CHUNK: usize = 8192;
const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[async_trait]
pub trait RecvMessage {
    async fn recv_message<R>(&mut self) -> Result<Option<R>, RecvError>
    where
        R: Message + ReflectMessage + Default;
}

#[async_trait]
impl<T> RecvMessage for BufferedReader<T>
where
    T: AsyncRead + Unpin + Send,
{
    async fn recv_message<M>(&mut self) -> Result<Option<M>, RecvError>
    where
        M: Message + ReflectMessage + Default,
    {
        if self.failed {
            return Err(io::Error::other("IPC connection is retired").into());
        }
        if !self.buffer.is_empty() && self.frame_started.is_none() {
            self.frame_started = Some(tokio::time::Instant::now());
        }
        loop {
            // Try to parse the message until the buffer is a valid message
            let mut cursor = io::Cursor::new(&self.buffer);
            match FigMessage::parse(&mut cursor) {
                // If the parsed message is valid, return it
                Ok((len, message)) => {
                    let _ = self.buffer.split_to(len);
                    self.frame_started = (!self.buffer.is_empty()).then(tokio::time::Instant::now);
                    return match message.decode() {
                        Ok(message) => Ok(Some(message)),
                        Err(error) => {
                            self.failed = true;
                            Err(error.into())
                        },
                    };
                },
                // If the message is incomplete, read more into the buffer
                Err(fig_proto::FigMessageParseError::Incomplete(_, _)) => {
                    // `read_buf` only fills the buffer's spare capacity, and `BytesMut`'s default
                    // growth hands out 64 bytes at a time — without this a 13 KB message needs
                    // nine `read` syscalls instead of two.
                    self.buffer.reserve(READ_CHUNK);
                    let mut chunk = (&mut self.inner).take(READ_CHUNK as u64);
                    let read = chunk.read_buf(&mut self.buffer);
                    let result = if let Some(start) = self.frame_started {
                        tokio::time::timeout_at(start + FRAME_TIMEOUT, read)
                            .await
                            .unwrap_or_else(|_| {
                                Err(io::Error::new(
                                    io::ErrorKind::TimedOut,
                                    "IPC frame assembly deadline exceeded",
                                ))
                            })
                    } else {
                        read.await
                    };
                    let bytes = match result {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            self.failed = true;
                            return Err(error.into());
                        },
                    };
                    if bytes > 0 && self.frame_started.is_none() {
                        self.frame_started = Some(tokio::time::Instant::now());
                    }

                    // If the buffer is empty, we've reached EOF
                    if bytes == 0 {
                        if self.buffer.is_empty() {
                            return Ok(None);
                        } else {
                            self.failed = true;
                            return Err(RecvError::Io(io::Error::from(io::ErrorKind::UnexpectedEof)));
                        }
                    }
                },
                // On any other error, return the error
                Err(err) => {
                    // A malformed frame retires this connection; searching payload bytes for
                    // another header could reinterpret untrusted data as a command.
                    self.failed = true;
                    return Err(err.into());
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::SendMessage;

    fn mock(initial: Vec<u8>) -> BufferedReader<Cursor<Vec<u8>>> {
        let size = initial.len();
        let mut inner = Cursor::new(initial);
        inner.set_position(size as u64);
        BufferedReader::new(inner)
    }

    fn test_message_small() -> fig_proto::local::LocalMessage {
        fig_proto::hooks::hook_to_message(fig_proto::hooks::new_event_hook("event", None, vec![]))
    }

    fn test_message_large() -> fig_proto::local::LocalMessage {
        fig_proto::hooks::hook_to_message(fig_proto::hooks::new_event_hook(
            "event",
            Some("A".repeat(10000)),
            vec![],
        ))
    }

    #[tokio::test]
    async fn single_message_small() {
        let mut mock = mock(vec![]);
        mock.send_message(test_message_small()).await.unwrap();
        mock.inner.set_position(0);
        assert_eq!(mock.recv_message().await.unwrap(), Some(test_message_small()));
    }

    #[tokio::test]
    async fn single_message_large() {
        let mut mock = mock(vec![]);
        mock.send_message(test_message_large()).await.unwrap();
        mock.inner.set_position(0);
        assert_eq!(mock.recv_message().await.unwrap(), Some(test_message_large()));
    }

    #[tokio::test]
    async fn mutlti_message_small() {
        let mut mock = mock(vec![]);
        for _ in 0..500 {
            mock.send_message(test_message_small()).await.unwrap();
        }
        mock.inner.set_position(0);
        for _ in 0..500 {
            assert_eq!(mock.recv_message().await.unwrap(), Some(test_message_small()));
        }
        assert_eq!(mock.read(&mut [0u8]).await.unwrap(), 0);
        assert_eq!(mock.buffer.len(), 0);
    }

    #[tokio::test]
    async fn mutlti_message_large() {
        let mut mock = mock(vec![]);
        for _ in 0..500 {
            mock.send_message(test_message_large()).await.unwrap();
        }
        mock.inner.set_position(0);
        for _ in 0..500 {
            assert_eq!(mock.recv_message().await.unwrap(), Some(test_message_large()));
        }
        assert_eq!(mock.read(&mut [0u8]).await.unwrap(), 0);
        assert_eq!(mock.buffer.len(), 0);
    }

    #[tokio::test]
    async fn invalid_header() {
        let mut mock = mock(vec![b'f', b'o', b'o']);
        mock.inner.set_position(0);
        assert!(mock.recv_message::<fig_proto::local::LocalMessage>().await.is_err());
    }
}

#[cfg(test)]
mod framing_limits {
    use std::time::Duration;

    use fig_proto::local::LocalMessage;
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn idle_connections_are_valid_but_a_cancelled_partial_frame_keeps_its_deadline() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut reader = BufferedReader::new(reader);
        // An established idle stream has no partial-frame deadline.
        assert!(
            tokio::time::timeout(Duration::from_secs(10), reader.recv_message::<LocalMessage>())
                .await
                .is_err()
        );
        writer.write_all(b"\x1b").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), reader.recv_message::<LocalMessage>())
                .await
                .is_err()
        );
        let start = tokio::time::Instant::now();
        let error = reader.recv_message::<LocalMessage>().await.unwrap_err();
        assert!(matches!(error, RecvError::Io(ref e) if e.kind() == io::ErrorKind::TimedOut));
        assert_eq!(start.elapsed(), Duration::from_secs(4));
        assert!(reader.failed);
        assert!(reader.recv_message::<LocalMessage>().await.is_err());
    }

    #[tokio::test]
    async fn oversized_length_is_rejected_without_waiting_for_its_body() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut reader = BufferedReader::new(reader);
        writer.write_all(b"\x1b@fig-pbuf").await.unwrap();
        writer.write_u64(u64::MAX).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), reader.recv_message::<LocalMessage>())
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error,
            RecvError::Parse(fig_proto::FigMessageParseError::TooLarge)
        ));
        assert!(reader.failed);
        assert_eq!(reader.buffer.len(), 18);
    }
}
