use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::Path;
use std::sync::{
    Arc,
    LazyLock,
};

use dashmap::DashMap;
use fig_os_shim::Context;
use image::{
    GenericImage,
    GenericImageView,
    Rgba,
};
use percent_encoding::percent_decode_str;
use tracing::{
    debug,
    trace,
    warn,
};
use url::Url;
use wry::http::header::CONTENT_TYPE;
use wry::http::{
    Request,
    Response,
    StatusCode,
};

use crate::platform::PlatformState;
use crate::protocol::util::{
    parse_hex_rgb,
    scale_u8,
};
use crate::webview::WindowId;

const DEFAULT_ICON: &str = "template";

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub enum AssetSpecifier<'a> {
    Named(Cow<'a, str>),
    PathBased(Cow<'a, Path>),
}

impl AssetSpecifier<'_> {
    /// A `'static` copy, for use as a cache key.
    fn to_owned_key(&self) -> AssetSpecifier<'static> {
        match self {
            AssetSpecifier::Named(name) => AssetSpecifier::Named(Cow::Owned(name.clone().into_owned())),
            AssetSpecifier::PathBased(path) => AssetSpecifier::PathBased(Cow::Owned(path.clone().into_owned())),
        }
    }
}

static ASSETS: LazyLock<HashMap<AssetSpecifier<'static>, Arc<Cow<'static, [u8]>>>> = LazyLock::new(|| {
    let mut map = HashMap::new();

    macro_rules! load_assets {
        ($($name: expr),*) => {
            $(
                let bytes = include_bytes!(concat!(env!("AUTOCOMPLETE_ICONS_PROCESSED"), "/", $name, ".png"));
                map.insert(
                    AssetSpecifier::Named($name.into()),
                    Arc::new(bytes.as_ref().into()),
                );
            )*
        };
    }

    load_assets! {
        "alert", "android", "apple", "asterisk", "aws", "azure", "box", "carrot", "characters", "command",
        "commandkey", "commit", "cpu", "database", "discord", "docker", "file", "firebase", "flag", "folder",
        "gcloud", "gear", "git", "github", "gitlab", "gradle", "heroku", "invite", "kubernetes", "netlify",
        "node", "npm", "okteto", "option", "package", "slack", "string", "symlink", "template", "twitter",
        "vercel", "yarn"
    }

    map
});

pub type ProcessedAsset = (Arc<Cow<'static, [u8]>>, AssetKind);

#[derive(Debug, Clone)]
pub enum AssetKind {
    Png,
    Svg,
}

impl AssetKind {
    pub fn mime(&self) -> mime::Mime {
        match self {
            AssetKind::Png => mime::IMAGE_PNG,
            AssetKind::Svg => mime::IMAGE_SVG,
        }
    }
}

async fn resolve_asset(asset: &AssetSpecifier<'_>, fallback: Option<&str>) -> (Arc<Cow<'static, [u8]>>, AssetKind) {
    match &asset {
        AssetSpecifier::Named(_) => match ASSETS.get(asset).map(|asset| (asset.clone(), AssetKind::Png)) {
            Some(asset) => Some(asset),
            None => PlatformState::icon_lookup(asset).await,
        },
        AssetSpecifier::PathBased(_) => PlatformState::icon_lookup(asset).await,
    }
    .or_else(|| match fallback {
        Some(fallback) => ASSETS
            .get(&AssetSpecifier::Named(fallback.into()))
            .map(|asset| (asset.clone(), AssetKind::Png)),
        None => None,
    })
    .unwrap_or_else(|| {
        ASSETS
            .get(&AssetSpecifier::Named(DEFAULT_ICON.into()))
            .map(|asset| (asset.clone(), AssetKind::Png))
            .unwrap()
    })
}

fn build_asset_response(data: Cow<'static, [u8]>, asset_kind: AssetKind) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, asset_kind.mime().essence_str())
        .body(data)
        .unwrap()
}

/// Resolved assets, keyed by what was asked for.
///
/// The function below was already named `cached_asset_response` and had no cache. Everything it
/// answers is derived from a file path or a compiled-in blob, and a `PathBased` lookup costs a
/// LaunchServices query plus a resize, a TIFF encode, a TIFF decode and a PNG encode — repeated
/// for every icon of every suggestion, every time the list is rebuilt. The fallback chain in
/// `macos.rs` calls in here up to four times for a single icon.
static ASSET_CACHE: LazyLock<DashMap<AssetCacheKey, ProcessedAsset>> = LazyLock::new(DashMap::new);

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
struct AssetCacheKey {
    asset: AssetSpecifier<'static>,
    fallback: Option<String>,
}

async fn cached_asset_response(asset: &AssetSpecifier<'_>, fallback: Option<&str>) -> Response<Cow<'static, [u8]>> {
    trace!(?asset, "building response for asset");

    let key = AssetCacheKey {
        asset: asset.to_owned_key(),
        fallback: fallback.map(ToOwned::to_owned),
    };

    if let Some(hit) = ASSET_CACHE.get(&key) {
        let (data, asset_kind) = hit.value();
        return build_asset_response((**data).clone(), asset_kind.clone());
    }

    let resolved = resolve_asset(asset, fallback).await;
    let response = build_asset_response((*resolved.0).clone(), resolved.1.clone());
    ASSET_CACHE.insert(key, resolved);
    response
}

async fn build_default() -> Response<Cow<'static, [u8]>> {
    cached_asset_response(&AssetSpecifier::Named(DEFAULT_ICON.into()), None).await
}

pub async fn handle(
    _ctx: Arc<Context>,
    request: Request<Vec<u8>>,
    _: WindowId,
) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    debug!(uri =% request.uri(), "fig:// protocol request");
    let url = Url::parse(&request.uri().to_string())?;
    let domain = url.domain();
    let pairs: HashMap<_, _> = url.query_pairs().collect();

    let res = match domain {
        Some("template") => {
            let query_pairs: HashMap<Cow<'_, str>, Cow<'_, str>> = url.query_pairs().collect();

            let asset = ASSETS.get(&AssetSpecifier::Named("template".into())).unwrap();
            let mut image = image::load_from_memory_with_format(asset, image::ImageFormat::Png).unwrap();

            if let Some(color) = query_pairs.get("color")
                && let Some(rgb) = parse_hex_rgb(color)
            {
                for y in 0..image.height() {
                    for x in 0..image.width() {
                        let Rgba([r, g, b, a]) = image.get_pixel(x, y);
                        image.put_pixel(
                            x,
                            y,
                            Rgba([scale_u8(r, rgb[0]), scale_u8(g, rgb[1]), scale_u8(b, rgb[2]), a]),
                        );
                    }
                }
            }

            let mut png_bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut png_bytes, image::ImageFormat::Png).unwrap();
            Some(build_asset_response(png_bytes.into_inner().into(), AssetKind::Png))
        },
        Some("icon" | "asset") => match pairs.get("asset").or_else(|| pairs.get("type")) {
            Some(name) => Some(cached_asset_response(&AssetSpecifier::Named(Cow::Borrowed(name)), None).await),
            None => None,
        },
        Some("path") => {
            let decoded_str = &*percent_decode_str(url.path()).decode_utf8().inspect_err(|err| {
                warn!(%err, "Failed to decode fig url");
            })?;

            let path_str = shellexpand::tilde(&decoded_str);
            let path = Path::new(path_str.as_ref());

            let fallback = match tokio::fs::metadata(path).await {
                Ok(meta) => {
                    if meta.is_dir() {
                        Some("folder")
                    } else if meta.is_file() {
                        Some("file")
                    } else if meta.is_symlink() {
                        Some("symlink")
                    } else {
                        warn!(%path_str, "Unknown file type");
                        None
                    }
                },
                Err(err) => {
                    warn!(%path_str, %err, "Failed to stat path");
                    Some(if path_str.ends_with('/') { "folder" } else { "file" })
                },
            };

            Some(cached_asset_response(&AssetSpecifier::PathBased(Cow::Borrowed(path)), fallback).await)
        },
        _ => None,
    };

    match res {
        Some(res) => Ok(res),
        None => Ok(build_default().await),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second request for the same asset must be answered from the cache, not by resolving it
    /// again — the `PathBased` path is a LaunchServices query plus three codec passes.
    #[tokio::test]
    async fn the_same_asset_is_only_resolved_once() {
        let asset = AssetSpecifier::Named("git".into());
        let key = AssetCacheKey {
            asset: asset.to_owned_key(),
            fallback: None,
        };
        ASSET_CACHE.remove(&key);

        let first = cached_asset_response(&asset, None).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert!(
            ASSET_CACHE.contains_key(&key),
            "the first request must populate the cache"
        );

        // Overwrite the cached bytes: if the second request re-resolves, it will not see them.
        let marker: Arc<Cow<'static, [u8]>> = Arc::new(Cow::Owned(b"cached".to_vec()));
        ASSET_CACHE.insert(key.clone(), (marker, AssetKind::Png));

        let second = cached_asset_response(&asset, None).await;
        assert_eq!(second.body().as_ref(), b"cached");

        ASSET_CACHE.remove(&key);
    }

    /// Different fallbacks give different answers for the same name, so the key has to carry it.
    #[test]
    fn the_fallback_is_part_of_the_key() {
        let asset = AssetSpecifier::Named("some-cli".into());
        let with_folder = AssetCacheKey {
            asset: asset.to_owned_key(),
            fallback: Some("folder".into()),
        };
        let with_file = AssetCacheKey {
            asset: asset.to_owned_key(),
            fallback: Some("file".into()),
        };

        assert_ne!(with_folder, with_file);
    }
}
