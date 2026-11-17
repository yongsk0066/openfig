use std::borrow::Cow;
use std::collections::{
    HashMap,
    VecDeque,
};
use std::hash::Hash;
use std::path::Path;
use std::sync::{
    Arc,
    LazyLock,
};

use fig_desktop::native::{
    Icon,
    Icons,
};
use image::{
    GenericImage,
    GenericImageView,
    Rgba,
};
use percent_encoding::percent_decode_str;
use tokio::sync::Mutex;
use tracing::{
    debug,
    trace,
    warn,
};
use url::Url;
use wry::http::header::{
    CONTENT_TYPE,
    HeaderValue,
};
use wry::http::{
    Request,
    Response,
};

use crate::protocol::util::{
    parse_hex_rgb,
    scale_u8,
};
use crate::webview::WindowId;

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

const TEMPLATE_BYTES: &[u8] = include_bytes!(concat!(env!("AUTOCOMPLETE_ICONS_PROCESSED"), "/template.png"));
const MAX_CACHED_ASSETS: usize = 512;
async fn native_asset(asset: &AssetSpecifier<'_>, native: Option<&Icons>) -> Option<ProcessedAsset> {
    let icon = match asset {
        AssetSpecifier::Named(name) => Icon::Named(name.to_string()),
        AssetSpecifier::PathBased(path) => Icon::Path(path.to_path_buf()),
    };
    let png = native?.render(icon).await?;
    Some((Arc::new(Cow::Owned(png)), AssetKind::Png))
}

fn fallback_asset(fallback: Option<&str>) -> ProcessedAsset {
    fallback
        .and_then(|name| ASSETS.get(&AssetSpecifier::Named(name.into())).cloned())
        .map_or_else(
            || (Arc::new(Cow::Borrowed(TEMPLATE_BYTES)), AssetKind::Png),
            |data| (data, AssetKind::Png),
        )
}

fn build_asset_response(data: Cow<'static, [u8]>, asset_kind: AssetKind) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::new(data);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(match asset_kind {
            AssetKind::Png => "image/png",
            AssetKind::Svg => "image/svg+xml",
        }),
    );
    response
}

/// Bounded FIFO of resolved bytes. Failed/overloaded native requests are never cached.
#[derive(Default)]
struct AssetCache {
    entries: HashMap<AssetCacheKey, ProcessedAsset>,
    order: VecDeque<AssetCacheKey>,
}
impl AssetCache {
    fn insert(&mut self, key: AssetCacheKey, value: ProcessedAsset) {
        if !self.entries.contains_key(&key) {
            while self.entries.len() >= MAX_CACHED_ASSETS {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                } else {
                    self.entries.clear();
                    break;
                }
            }
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, value);
    }
}
static ASSET_CACHE: LazyLock<Mutex<AssetCache>> = LazyLock::new(|| Mutex::new(AssetCache::default()));

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
struct AssetCacheKey {
    asset: AssetSpecifier<'static>,
    fallback: Option<String>,
}

async fn cached_asset_response(
    asset: &AssetSpecifier<'_>,
    fallback: Option<&str>,
    native: Option<&Icons>,
) -> Response<Cow<'static, [u8]>> {
    trace!(?asset, "building response for asset");

    let key = AssetCacheKey {
        asset: asset.to_owned_key(),
        fallback: fallback.map(ToOwned::to_owned),
    };

    if let Some((data, kind)) = ASSET_CACHE.lock().await.entries.get(&key).cloned() {
        return build_asset_response((*data).clone(), kind);
    }
    let resolved = match ASSETS.get(asset) {
        Some(data) => Some((data.clone(), AssetKind::Png)),
        None => native_asset(asset, native).await,
    };
    let resolved = match resolved {
        Some(value) => {
            ASSET_CACHE.lock().await.insert(key, value.clone());
            value
        },
        None => fallback_asset(fallback),
    };
    build_asset_response((*resolved.0).clone(), resolved.1)
}

pub async fn handle(
    request: Request<Vec<u8>>,
    _: WindowId,
    native: Option<&Icons>,
) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    debug!(uri =% request.uri(), "fig:// protocol request");
    let url = Url::parse(&request.uri().to_string())?;
    let domain = url.domain();
    let pairs: HashMap<_, _> = url.query_pairs().collect();

    let res = match domain {
        Some("template") => {
            let query_pairs: HashMap<Cow<'_, str>, Cow<'_, str>> = url.query_pairs().collect();

            let mut image = image::load_from_memory_with_format(TEMPLATE_BYTES, image::ImageFormat::Png)?;

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
            image.write_to(&mut png_bytes, image::ImageFormat::Png)?;
            Some(build_asset_response(png_bytes.into_inner().into(), AssetKind::Png))
        },
        Some("icon" | "asset") => match pairs.get("asset").or_else(|| pairs.get("type")) {
            Some(name) => Some(cached_asset_response(&AssetSpecifier::Named(Cow::Borrowed(name)), None, native).await),
            None => None,
        },
        Some("path") => {
            let decoded_str = &*percent_decode_str(url.path()).decode_utf8().inspect_err(|err| {
                warn!(%err, "Failed to decode fig url");
            })?;

            let path_str = shellexpand::tilde(&decoded_str);
            let path = Path::new(path_str.as_ref());

            // Do not stat paths on Tokio's non-cancelable filesystem pool. The native helper
            // resolves real file/folder/symlink icons; an unavailable helper uses a lexical hint.
            let fallback = Some(if path_str.ends_with('/') { "folder" } else { "file" });

            Some(cached_asset_response(&AssetSpecifier::PathBased(Cow::Borrowed(path)), fallback, native).await)
        },
        _ => None,
    };

    match res {
        Some(res) => Ok(res),
        None => Ok(build_asset_response(Cow::Borrowed(TEMPLATE_BYTES), AssetKind::Png)),
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

        let first = cached_asset_response(&asset, None, None).await;
        assert_eq!(first.status(), wry::http::StatusCode::OK);
        assert!(
            ASSET_CACHE.lock().await.entries.contains_key(&key),
            "the first request must populate the cache"
        );

        // Overwrite the cached bytes: if the second request re-resolves, it will not see them.
        let marker: Arc<Cow<'static, [u8]>> = Arc::new(Cow::Owned(b"cached".to_vec()));
        ASSET_CACHE.lock().await.insert(key.clone(), (marker, AssetKind::Png));

        let second = cached_asset_response(&asset, None, None).await;
        assert_eq!(second.body().as_ref(), b"cached");
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
    #[test]
    fn cache_capacity_and_updates_preserve_fifo_membership() {
        let mut cache = AssetCache::default();
        let key = |n| AssetCacheKey {
            asset: AssetSpecifier::Named(format!("icon-{n}").into()),
            fallback: None,
        };
        for n in 0..MAX_CACHED_ASSETS {
            cache.insert(key(n), fallback_asset(None));
        }
        cache.insert(key(0), fallback_asset(None));
        assert_eq!(cache.order.len(), MAX_CACHED_ASSETS);
        cache.insert(key(MAX_CACHED_ASSETS), fallback_asset(None));
        assert!(!cache.entries.contains_key(&key(0)));
        assert!(cache.entries.contains_key(&key(MAX_CACHED_ASSETS)));
        assert_eq!(cache.entries.len(), MAX_CACHED_ASSETS);
    }

    #[tokio::test]
    async fn unavailable_native_loop_returns_fallback_without_caching_failure() {
        let asset = AssetSpecifier::Named("missing-native-test-type".into());
        let key = AssetCacheKey {
            asset: asset.to_owned_key(),
            fallback: None,
        };
        let response = cached_asset_response(&asset, None, None).await;
        assert_eq!(response.body().as_ref(), TEMPLATE_BYTES);
        assert!(!ASSET_CACHE.lock().await.entries.contains_key(&key));
    }
}
