use std::borrow::Cow;
use std::io::ErrorKind;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::Arc;

use fig_os_shim::Context;
use tokio::sync::OnceCell;
use tracing::info;
use wry::http::header::CONTENT_TYPE;
use wry::http::{
    Request,
    Response,
    StatusCode,
};

use super::util::{
    res_400,
    res_404,
    res_500,
};
use crate::webview::WindowId;

fn relativize(path: &Path) -> &Path {
    path.strip_prefix("/").unwrap_or(path)
}

pub trait Scope {
    const PATH: &'static str;
}

pub struct Dashboard;

impl Scope for Dashboard {
    const PATH: &'static str = "dashboard";
}

pub struct Autocomplete;

impl Scope for Autocomplete {
    const PATH: &'static str = "autocomplete";
}

/// The canonicalized bundle resources directory.
///
/// It was recomputed on every request — `current_exe()` plus a `canonicalize()` — for something
/// that cannot change while the process runs, and the autocomplete bundle alone is ~570 KB
/// served from this handler on every reload.
static RESOURCES_PATH: OnceCell<Option<PathBuf>> = OnceCell::const_new();

async fn resources_path(ctx: &Context) -> Option<&'static Path> {
    RESOURCES_PATH
        .get_or_init(|| async {
            match fig_util::directories::resources_path_ctx(ctx) {
                Ok(path) => tokio::fs::canonicalize(path).await.ok(),
                Err(err) => {
                    tracing::error!(%err, "failed to resolve the bundle resources dir");
                    None
                },
            }
        })
        .await
        .as_deref()
}

/// What a request path resolves to inside `root`.
enum Resolved {
    File(PathBuf),
    /// No such file, and the request looks like a page rather than an asset.
    Index,
    NotFound,
    /// The path escaped `root`.
    Escaped,
    Failed(std::io::Error),
}

/// Resolve `uri_path` inside `root`, rejecting anything that escapes it.
///
/// `root` is already canonical (see [`resources_path`]); previously only the candidate side was
/// canonicalized, so a symlinked resources directory made every request look like an escape and
/// a symlink *inside* the directory pointing out of it was not caught at all.
async fn resolve_in_root(root: &Path, uri_path: &Path) -> Resolved {
    let looks_like_a_page = uri_path.extension().is_none();

    let mut path = match tokio::fs::canonicalize(root.join(relativize(uri_path))).await {
        Ok(path) => path,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return if looks_like_a_page {
                Resolved::Index
            } else {
                Resolved::NotFound
            };
        },
        Err(err) => return Resolved::Failed(err),
    };

    // dont allow escaping the resources dir
    if !path.starts_with(root) {
        return Resolved::Escaped;
    }

    match tokio::fs::metadata(&path).await {
        Ok(metadata) => {
            if metadata.is_dir() {
                path.push("index.html");
            }
        },
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return if looks_like_a_page {
                Resolved::Index
            } else {
                Resolved::NotFound
            };
        },
        Err(err) => return Resolved::Failed(err),
    }

    Resolved::File(path)
}

fn mime_for(path: &Path, content: &[u8]) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => mime::TEXT_HTML_UTF_8.as_ref(),
        Some("css") => mime::TEXT_CSS_UTF_8.as_ref(),
        Some("js") => mime::APPLICATION_JAVASCRIPT_UTF_8.as_ref(),
        Some("json") => mime::APPLICATION_JSON.as_ref(),
        Some("svg") => mime::IMAGE_SVG.as_ref(),
        Some("png") => mime::IMAGE_PNG.as_ref(),
        Some("jpg" | "jpeg") => mime::IMAGE_JPEG.as_ref(),
        Some("woff2") => mime::FONT_WOFF2.as_ref(),
        Some("woff") => mime::FONT_WOFF.as_ref(),
        Some("txt" | "text") => mime::TEXT_PLAIN.as_ref(),
        _ => match infer::get(content) {
            Some(mime) => mime.mime_type(),
            // https://developer.mozilla.org/en-US/docs/Web/HTTP/Basics_of_HTTP/MIME_types/Common_types
            None => mime::APPLICATION_OCTET_STREAM.as_ref(),
        },
    }
}

async fn serve(root: &Path, uri_path: &Path) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    let path = match resolve_in_root(root, uri_path).await {
        Resolved::File(path) => path,
        Resolved::Index => root.join("index.html"),
        Resolved::NotFound => return res_404(),
        Resolved::Escaped => return res_400(),
        Resolved::Failed(err) => return res_500(err),
    };

    info!("serving resource: {}", path.display());

    // `tokio::fs`, not `std::fs`: this is an async fn on a shared runtime worker and the
    // autocomplete bundle is ~570 KB.
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return res_404(),
        Err(err) => return res_500(err),
    };

    let mime = mime_for(&path, &content);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime)
        .body(content.into())?)
}

/// handle `figresource://localhost/`
pub async fn handle<S: Scope>(
    ctx: Arc<Context>,
    request: Request<Vec<u8>>,
    _: WindowId,
) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    if request.uri().host() != Some("localhost") {
        return res_400();
    }

    let Some(root) = resources_path(&ctx).await else {
        return res_500(std::io::Error::new(
            ErrorKind::NotFound,
            "the bundle resources directory could not be resolved",
        ));
    };

    serve(&root.join(S::PATH), Path::new(request.uri().path())).await
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    async fn root() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = tokio::fs::canonicalize(dir.path()).await.unwrap();
        tokio::fs::write(root.join("index.html"), b"<html>").await.unwrap();
        tokio::fs::write(root.join("app.js"), b"console.log(1)").await.unwrap();
        (dir, root)
    }

    #[tokio::test]
    async fn a_file_is_served_with_its_mime_type() {
        let (_dir, root) = root().await;
        let response = serve(&root, Path::new("/app.js")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            mime::APPLICATION_JAVASCRIPT_UTF_8.as_ref()
        );
        assert_eq!(response.body().as_ref(), b"console.log(1)");
    }

    /// A path with no extension is a client-side route, and falls back to the SPA entry point.
    #[tokio::test]
    async fn an_unknown_page_falls_back_to_index_html() {
        let (_dir, root) = root().await;
        let response = serve(&root, Path::new("/settings/keybindings")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body().as_ref(), b"<html>");
    }

    #[tokio::test]
    async fn a_missing_asset_is_not_found() {
        let (_dir, root) = root().await;
        let response = serve(&root, Path::new("/nope.js")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// The webview can ask for any path it likes; `..` must not walk out of the resources dir.
    #[tokio::test]
    async fn escaping_the_resources_root_is_rejected() {
        let (_dir, root) = root().await;
        let outside = root.parent().unwrap().join("outside.js");
        tokio::fs::write(&outside, b"secret").await.unwrap();

        let escape = Path::new("/../").join(outside.file_name().unwrap());
        let response = serve(&root, &escape).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_ne!(response.body().as_ref(), b"secret");

        tokio::fs::remove_file(outside).await.ok();
    }

    /// And neither may a symlink that lives inside the root but points out of it — the old code
    /// canonicalized only the candidate and compared it against an *un*canonicalized root.
    #[tokio::test]
    async fn a_symlink_out_of_the_resources_root_is_rejected() {
        let (_dir, root) = root().await;
        let outside = root.parent().unwrap().join("symlink-target.js");
        tokio::fs::write(&outside, b"secret").await.unwrap();
        tokio::fs::symlink(&outside, root.join("link.js")).await.unwrap();

        let response = serve(&root, Path::new("/link.js")).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_ne!(response.body().as_ref(), b"secret");

        tokio::fs::remove_file(outside).await.ok();
    }
}
