use std::borrow::Cow;

use fig_proto::ReflectMessage;

pub mod fs;
pub mod history;
pub mod install;
pub mod other;
pub mod platform;
pub mod settings;
pub mod state;

pub type ServerOriginatedSubMessage = fig_proto::fig::server_originated_message::Submessage;
pub type RequestResult = Result<Box<ServerOriginatedSubMessage>, Error>;

pub trait RequestResultImpl {
    fn success() -> Self;
    fn error(msg: impl Into<String>) -> Self;
    fn deprecated(message: impl ReflectMessage) -> Self;
    fn unimplemented(message: impl ReflectMessage) -> Self;
}

impl RequestResultImpl for RequestResult {
    fn success() -> Self {
        RequestResult::Ok(Box::new(ServerOriginatedSubMessage::Success(true)))
    }

    fn error(msg: impl Into<String>) -> Self {
        RequestResult::Ok(Box::new(ServerOriginatedSubMessage::Error(msg.into())))
    }

    fn deprecated(message: impl ReflectMessage) -> Self {
        RequestResult::error(format!("{} is deprecated", message.descriptor().name()))
    }

    fn unimplemented(message: impl ReflectMessage) -> Self {
        RequestResult::error(format!("{} is unimplemented", message.descriptor().name()))
    }
}

#[derive(Debug)]
pub enum Error {
    Custom(Cow<'static, str>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Custom(v) => f.write_str(v),
        }
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Custom(s.into())
    }
}

impl From<&'static str> for Error {
    fn from(s: &'static str) -> Self {
        Error::Custom(s.into())
    }
}
