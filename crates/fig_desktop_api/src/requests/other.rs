use fig_proto::fig::server_originated_message::Submessage as ServerSubmessage;
use fig_proto::fig::{
    OpenInExternalApplicationRequest,
    PingRequest,
    PingResponse,
};
use macos_utils::open_url;

use super::{
    RequestResult,
    RequestResultImpl,
};

pub(crate) async fn open_in_external_application(request: OpenInExternalApplicationRequest) -> RequestResult {
    match request.url {
        Some(url) => match open_url(&url) {
            Ok(_) => RequestResult::success(),
            Err(err) => RequestResult::error(format!("Failed to open url {url}: {err}")),
        },
        None => RequestResult::error("No url provided to open"),
    }
}

pub(crate) fn ping(_request: PingRequest) -> RequestResult {
    RequestResult::Ok(Box::new(ServerSubmessage::PingResponse(PingResponse {})))
}
