use std::sync::LazyLock;

use tracing::error;
use url::Url;

/// The bundled resources served by the `figresource://` protocol handler.
pub static RESOURCE_URL: LazyLock<Url> = LazyLock::new(|| Url::parse("figresource://localhost").unwrap());

/// The URL for one of the two windows, honouring the env var and the `developer.*.host` setting.
fn url(env_var: &str, dev_host_setting: &str) -> Url {
    if let Ok(url) = std::env::var(env_var) {
        match Url::parse(&url) {
            Ok(url) => return url,
            Err(err) => error!(%err, %env_var, "Failed to parse the URL override"),
        }
    }

    if let Some(dev_url) = fig_settings::settings::get_string_opt(dev_host_setting) {
        match Url::parse(&dev_url) {
            Ok(url) => return url,
            Err(err) => error!(%err, setting = %dev_host_setting, "Failed to parse the developer host"),
        }
    }

    RESOURCE_URL.clone()
}

pub fn autocomplete() -> Url {
    url("AUTOCOMPLETE_URL", "developer.autocomplete.host")
}

pub fn dashboard() -> Url {
    url("DASHBOARD_URL", "developer.dashboard.host")
}
