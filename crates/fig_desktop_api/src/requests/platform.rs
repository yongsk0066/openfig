use fig_proto::fig::{
    GetPlatformInfoRequest,
    GetPlatformInfoResponse,
    Os,
};

use super::{
    RequestResult,
    ServerOriginatedSubMessage,
};

pub(crate) async fn get_platform_info<Ctx>(_request: GetPlatformInfoRequest, _ctx: &Ctx) -> RequestResult {
    let response = ServerOriginatedSubMessage::GetPlatformInfoResponse(GetPlatformInfoResponse {
        os: Os::Macos.into(),
        desktop_environment: None,
        display_server_protocol: None,
        app_bundle_type: None,
    });
    Ok(response.into())
}
