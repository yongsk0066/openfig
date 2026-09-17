use std::collections::HashMap;
use std::env::{
    consts,
    var,
};

use camino::Utf8PathBuf;
use fig_util::{
    APP_BUNDLE_ID,
    CLI_BINARY_NAME,
    DATA_DIR_NAME,
    PRODUCT_NAME,
    URL_SCHEMA,
    directories,
};
use serde::Serialize;
use serde_json::json;
use which::which;

const DEFAULT_THEMES: &[&str] = &["light", "dark", "system"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Constants {
    // Brand names, injected so the web packages never hardcode one. Serialized (camelCase)
    // as productName / cliBinaryName / bundleId / urlSchema / dataDirName.
    product_name: &'static str,
    cli_binary_name: &'static str,
    bundle_id: &'static str,
    url_schema: &'static str,
    data_dir_name: &'static str,
    version: &'static str,
    cli: Option<Utf8PathBuf>,
    bundle_path: Option<Utf8PathBuf>,
    remote: Option<String>,
    home: Option<Utf8PathBuf>,
    fig_dot_dir: Option<Utf8PathBuf>,
    fig_data_dir: Option<Utf8PathBuf>,
    backups_dir: Option<Utf8PathBuf>,
    logs_dir: Option<Utf8PathBuf>,
    user: String,
    default_path: Option<String>,
    themes_folder: Option<Utf8PathBuf>,
    themes: Vec<String>,
    os: &'static str,
    arch: &'static str,
    env: HashMap<String, String>,
    new_uri_format: bool,
    support_api_proto: bool,
    api_proto_url: String,
    macos_version: String,
}

impl Constants {
    fn new(support_api_proto: bool) -> Self {
        let themes_folder = directories::themes_dir()
            .ok()
            .and_then(|dir| Utf8PathBuf::try_from(dir).ok());

        let themes: Vec<String> = themes_folder
            .as_ref()
            .and_then(|path| {
                std::fs::read_dir(path).ok().map(|dir| {
                    dir.filter_map(|file| {
                        file.ok().and_then(|file| {
                            file.file_name()
                                .to_str()
                                .map(|name| name.strip_suffix(".json").unwrap_or(name))
                                .map(String::from)
                        })
                    })
                    .chain(DEFAULT_THEMES.iter().map(|s| (*s).to_owned()))
                    .collect()
                })
            })
            .unwrap_or_else(|| DEFAULT_THEMES.iter().map(|s| (*s).to_owned()).collect());

        Self {
            product_name: PRODUCT_NAME,
            cli_binary_name: CLI_BINARY_NAME,
            bundle_id: APP_BUNDLE_ID,
            url_schema: URL_SCHEMA,
            data_dir_name: DATA_DIR_NAME,
            version: env!("CARGO_PKG_VERSION"),
            cli: which(CLI_BINARY_NAME)
                .ok()
                .and_then(|exe| Utf8PathBuf::try_from(exe).ok()),
            bundle_path: None,
            remote: None,
            home: directories::home_dir_utf8().ok(),
            fig_dot_dir: directories::fig_data_dir_utf8().ok(),
            fig_data_dir: directories::fig_data_dir_utf8().ok(),
            backups_dir: directories::backups_dir_utf8().ok(),
            logs_dir: directories::logs_dir_utf8().ok(),
            user: whoami::username().unwrap_or_else(|_| "unknown".into()),
            default_path: var("PATH").ok(),
            themes_folder,
            themes,
            os: consts::OS,
            arch: consts::ARCH,
            env: std::env::vars().collect(),
            new_uri_format: true,
            support_api_proto,
            api_proto_url: "api://localhost".to_string(),
            macos_version: macos_utils::os::OperatingSystemVersion::get().to_string(),
        }
    }
}

impl Constants {
    fn script(&self) -> String {
        format!("fig.constants = {};", json!(self))
    }
}

pub fn javascript_init(support_api_proto: bool) -> String {
    [
        r#"if (!window.fig || !window.fig.quiet) console.log("[fig] declaring constants...");"#.into(),
        "if (!window.fig) window.fig = {};".into(),
        "if (!window.fig.constants) fig.constants = {};".into(),
        Constants::new(support_api_proto).script(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web packages read these exact keys off `window.fig.constants`; a rename here
    /// silently empties every product name in the dashboard and autocomplete UIs.
    #[test]
    fn brand_constants_json_keys() {
        let value = serde_json::to_value(Constants::new(true)).unwrap();
        for key in ["productName", "cliBinaryName", "bundleId", "urlSchema", "dataDirName"] {
            assert!(value.get(key).is_some_and(|v| v.is_string()), "missing key {key}");
        }
    }
}
