//! Protocol buffer definitions

pub mod fig;
pub mod fig_common;
pub mod figterm;
pub mod hooks;
pub mod local;
pub(crate) mod proto;
pub mod remote_hooks;
pub mod util;
use std::fmt::Debug;
use std::mem::size_of;
use std::num::TryFromIntError;
use std::sync::LazyLock;

use bytes::{
    Buf,
    Bytes,
    BytesMut,
};
pub use prost;
use prost::{
    DecodeError,
    Message,
};
use prost_reflect::DescriptorPool;
pub use prost_reflect::{
    DynamicMessage,
    ReflectMessage,
};
use serde::Serialize;
use thiserror::Error;

pub mod remote {
    pub use crate::proto::remote::*;

    /// Version of the pty binary <-> desktop protocol in `proto/remote.proto`, sent in
    /// `Hostbound.Handshake.protocol_version`.
    ///
    /// The desktop refuses a handshake whose version is not this one, and the pty binary then runs
    /// the shell without hooks instead of talking a protocol the other side may misread — the
    /// case of a `<shell> (figterm)` copy that was already running when the app was rebuilt.
    /// Builds that predate the field send 0 and are refused as well.
    ///
    /// Bump it with any change to `remote.proto`, or to a message it carries, that an older peer
    /// would misread. Adding an optional field both sides may ignore does not need a bump.
    pub const PROTOCOL_VERSION: u32 = 1;
}

// This is not used explicitly, but it must be here for the derive
// impls on the protos for dynamic message
static DESCRIPTOR_POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(include_bytes!(concat!(env!("OUT_DIR"), "/file_descriptor_set.bin")).as_ref()).unwrap()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigMessageType {
    Protobuf,
    Json,
    MessagePack,
}

impl FigMessageType {
    const fn header(&self) -> &'static [u8] {
        match self {
            FigMessageType::Protobuf => b"fig-pbuf",
            FigMessageType::Json => b"fig-json",
            FigMessageType::MessagePack => b"fig-mpak",
        }
    }
}

/// A fig message
///
/// The format of a fig message is:
///
///   - The header `\x1b@`
///   - The type of the message (must be 8 bytes)
///     - `fig-pbuf` - Protocol Buffer
///     - `fig-json` - Json
///     - `fig-mpak` - MessagePack
///   - The length of the remainder of the message encoded as a big endian u64
///   - The message, encoded as protobuf, json-protobuf, or messagepack-protobuf
#[derive(Debug, Clone)]
pub struct FigMessage {
    inner: Bytes,
    message_type: FigMessageType,
}

#[derive(Debug)]
pub enum FigMessageComponent {
    Header,
    BodySize,
    Body,
}

#[derive(Debug, Error)]
pub enum FigMessageParseError {
    /// The missing component and the needed bytes
    #[error("incomplete message, missing {0:?}")]
    Incomplete(FigMessageComponent, usize),
    #[error("invalid message header {0} (raw type {1})")]
    InvalidHeader(String, String),
    #[error("invalid message type")]
    InvalidMessageType([u8; 8]),
    #[error("failed to convert int: {0}")]
    TryFromInt(#[from] TryFromIntError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum FigMessageDecodeError {
    #[error("name is not a valid protobuf: {0}")]
    NameNotValid(String),
    #[error(transparent)]
    ProstDecode(#[from] DecodeError),
    #[error(transparent)]
    JsonDecode(#[from] serde_json::Error),
    #[error(transparent)]
    RmpDecode(#[from] rmp_serde::decode::Error),
}

#[derive(Debug, Error)]
pub enum FigMessageEncodeError {
    #[error(transparent)]
    IntError(#[from] std::num::TryFromIntError),
    #[error(transparent)]
    JsonEncode(#[from] serde_json::Error),
    #[error(transparent)]
    RmpEncode(#[from] rmp_serde::encode::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

impl FigMessage {
    pub fn json(json: impl Serialize) -> Result<Bytes, FigMessageEncodeError> {
        FigMessage::encode(FigMessageType::Json, serde_json::to_vec(&json)?.into())
    }

    fn encode_buf(&self, dst: &mut BytesMut) -> Result<(), FigMessageEncodeError> {
        let body = &self.inner;
        let message_type = self.message_type;

        let message_len: u64 = body.len().try_into()?;
        let message_len_be = message_len.to_be_bytes();

        dst.reserve(b"\x1b@".len() + message_type.header().len() + message_len_be.len() + body.len());
        dst.extend_from_slice(b"\x1b@");
        dst.extend_from_slice(message_type.header());
        dst.extend_from_slice(&message_len_be);
        dst.extend_from_slice(body);

        Ok(())
    }

    pub fn to_encoded(&self) -> Result<Bytes, FigMessageEncodeError> {
        let mut inner: BytesMut = BytesMut::new();
        self.encode_buf(&mut inner)?;
        Ok(inner.freeze())
    }

    fn encode(message_type: FigMessageType, body: Bytes) -> Result<Bytes, FigMessageEncodeError> {
        let msg = Self {
            inner: body,
            message_type,
        };

        let mut inner: BytesMut = BytesMut::new();
        msg.encode_buf(&mut inner)?;

        Ok(inner.freeze())
    }

    pub fn parse(src: &mut impl bytes::Buf) -> Result<(usize, FigMessage), FigMessageParseError> {
        if src.remaining() < 10 {
            return Err(FigMessageParseError::Incomplete(
                FigMessageComponent::Header,
                10 - src.remaining(),
            ));
        }

        let mut header = [0; 2];
        src.copy_to_slice(&mut header);
        if header[0] != b'\x1b' || header[1] != b'@' {
            let mut message_type_buf = [0; 8];
            src.copy_to_slice(&mut message_type_buf);
            return Err(FigMessageParseError::InvalidHeader(
                hex::encode(header),
                hex::encode(message_type_buf),
            ));
        }

        let mut message_type_buf = [0; 8];
        src.copy_to_slice(&mut message_type_buf);
        let message_type = match &message_type_buf {
            b"fig-pbuf" => FigMessageType::Protobuf,
            b"fig-json" => FigMessageType::Json,
            b"fig-mpak" => FigMessageType::MessagePack,
            _ => return Err(FigMessageParseError::InvalidMessageType(message_type_buf)),
        };

        if src.remaining() < size_of::<u64>() {
            return Err(FigMessageParseError::Incomplete(
                FigMessageComponent::BodySize,
                size_of::<u64>() - src.remaining(),
            ));
        }

        let len: usize = src.get_u64().try_into()?;

        if src.remaining() < len {
            return Err(FigMessageParseError::Incomplete(
                FigMessageComponent::Body,
                len - src.remaining(),
            ));
        }

        let mut inner = vec![0; len];
        src.copy_to_slice(&mut inner);

        let message_len = 10 + size_of::<u64>() + len;

        Ok((message_len, FigMessage {
            inner: Bytes::from(inner),
            message_type,
        }))
    }

    pub fn decode<T>(self) -> Result<T, FigMessageDecodeError>
    where
        T: Message + ReflectMessage + Default,
    {
        match self.message_type {
            FigMessageType::Protobuf => Ok(T::decode(self.inner)?),
            FigMessageType::Json => Ok(DynamicMessage::deserialize(
                T::default().descriptor(),
                &mut serde_json::Deserializer::from_slice(self.inner.as_ref()),
            )?
            .transcode_to()?),
            FigMessageType::MessagePack => Ok(DynamicMessage::deserialize(
                T::default().descriptor(),
                &mut rmp_serde::Deserializer::from_read_ref(self.inner.as_ref()),
            )?
            .transcode_to()?),
        }
    }
}

impl std::ops::Deref for FigMessage {
    type Target = Bytes;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// A trait for types that can be converted to a FigProtobuf
pub trait FigProtobufEncodable: Debug + Send + Sync {
    /// Encodes a protobuf message into a fig message
    fn encode_fig_protobuf(&self) -> Result<Bytes, FigMessageEncodeError>;
}

// prost 0.14 dropped the Debug supertrait from Message, so it has to be spelled out here.
impl<T: Message + Debug> FigProtobufEncodable for T {
    fn encode_fig_protobuf(&self) -> Result<Bytes, FigMessageEncodeError> {
        FigMessage::encode(FigMessageType::Protobuf, self.encode_to_vec().into())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn test_message() -> local::LocalMessage {
        let hook = hooks::new_event_hook("event-name", Some("payload".to_string()), vec!["app".to_string()]);
        hooks::hook_to_message(hook)
    }

    #[test]
    fn test_to_fig_pbuf() {
        let message = test_message();
        assert_eq!(&message.encode_fig_protobuf().unwrap()[..10], b"\x1b@fig-pbuf");
    }

    #[test]
    fn json_round_trip() {
        let message = test_message();
        let json = serde_json::to_vec(&message.transcode_to_dynamic()).unwrap();

        let msg = FigMessage {
            inner: Bytes::from(json),
            message_type: FigMessageType::Json,
        };

        assert_eq!(&msg.to_encoded().unwrap()[..10], b"\x1b@fig-json");

        let decoded_message: local::LocalMessage = msg.decode().unwrap();

        assert_eq!(message, decoded_message);
    }

    #[test]
    fn json_decode() {
        let msg = FigMessage {
            inner: Bytes::from(
                serde_json::to_vec(&json!({
                    "hook": {
                        "event": {
                          "eventName": "event-name",
                          "payload": "payload",
                          "apps": ["app"]
                        }
                    }
                }))
                .unwrap(),
            ),
            message_type: FigMessageType::Json,
        };

        let decoded_message: local::LocalMessage = msg.decode().unwrap();

        let hook = match decoded_message.r#type.unwrap() {
            local::local_message::Type::Hook(hook) => hook,
            local::local_message::Type::Command(_) => panic!(),
        };

        let event = match hook.hook.unwrap() {
            local::hook::Hook::Event(event) => event,
            other => panic!("expected an event hook, got {other:?}"),
        };

        assert_eq!(event.event_name, "event-name");
        assert_eq!(event.payload.as_deref(), Some("payload"));
        assert_eq!(event.apps, vec!["app".to_string()]);
    }

    #[test]
    fn rmp_round_trip() {
        let message = test_message();
        let mpack = rmp_serde::to_vec(&message.transcode_to_dynamic()).unwrap();

        let msg = FigMessage {
            inner: Bytes::from(mpack),
            message_type: FigMessageType::MessagePack,
        };

        assert_eq!(&msg.to_encoded().unwrap()[..10], b"\x1b@fig-mpak");

        let decoded_message: local::LocalMessage = msg.decode().unwrap();

        assert_eq!(message, decoded_message);
    }
}
