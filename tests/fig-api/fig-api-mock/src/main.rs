use clap::Parser;
use fig_desktop_api::handler::DesktopRequest;
use fig_desktop_api::requests::{
    RequestResult,
    RequestResultImpl,
};
use fig_os_shim::Env;

#[derive(Parser)]
enum Cli {
    Request {
        request_b64: String,
        #[arg(long)]
        cwd: Option<String>,
    },
    Init,
}

/// The mock's stand-in for the desktop app: notification subscriptions succeed, everything else
/// that needs the app is unimplemented.
async fn desktop(_message_id: i64, request: DesktopRequest) -> RequestResult {
    match request {
        DesktopRequest::Notification(_) => RequestResult::success(),
        DesktopRequest::InsertText(request) => RequestResult::unimplemented(request),
        DesktopRequest::RunProcess(request) => RequestResult::unimplemented(request),
        DesktopRequest::UpdateApplicationProperties(request) => RequestResult::unimplemented(request),
        DesktopRequest::PositionWindow(request) => RequestResult::unimplemented(request),
        DesktopRequest::DragWindow(request) => RequestResult::unimplemented(request),
        DesktopRequest::Onboarding(request) => RequestResult::unimplemented(request),
    }
}

#[tokio::main]
async fn main() {
    match Cli::parse() {
        Cli::Request { request_b64, cwd } => {
            if let Some(cwd) = cwd {
                std::env::set_current_dir(cwd).unwrap();
            }

            let request = fig_desktop_api::handler::request_from_b64(&request_b64).unwrap();
            let response = fig_desktop_api::handler::api_request(request, &Env::new(), desktop)
                .await
                .unwrap();
            let response_b64 = fig_desktop_api::handler::response_to_b64(response);
            println!("{response_b64}");
        },
        Cli::Init => {
            println!("{}", fig_desktop_api::init_script::javascript_init(false));
        },
    }
}
