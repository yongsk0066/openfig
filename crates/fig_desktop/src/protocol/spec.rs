use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::Arc;

use fig_os_shim::Context;
use parking_lot::Mutex;
use serde::{
    Deserialize,
    Serialize,
};
use tokio::sync::OnceCell;
use tracing::{
    debug,
    error,
};
use wry::http::header::CONTENT_TYPE;
use wry::http::{
    HeaderValue,
    Request,
    Response,
    StatusCode,
};

use crate::webview::WindowId;

const TEXT_JAVASCRIPT: HeaderValue = HeaderValue::from_static("text/javascript");
const APPLICATION_JSON: HeaderValue = HeaderValue::from_static("application/json");

fn res_404() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "text/plain")
        .body(b"Not Found".as_ref().into())
        .unwrap()
}

fn res_ok(bytes: Vec<u8>, content_type: HeaderValue) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .body(bytes.into())
        .unwrap()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecIndex {
    /// Defaulted so that a root listing only one of the two keys still contributes the other,
    /// instead of being discarded whole.
    #[serde(default)]
    completions: Vec<String>,
    #[serde(default)]
    diff_versioned_completions: Vec<String>,
}

/// The spec directories to search, in priority order: the user override first, then the
/// directory bundled into the app resources.
///
/// Canonicalized once, here, rather than per request inside `resolve_in_root`. Startup alone
/// makes ~60 `<name>.js` requests and each one used to re-resolve both roots
/// (`autocomplete_specs_dir` + a `metadata`, `current_exe` + a `canonicalize`) before touching
/// the file it wanted. Neither can change while the process runs.
static SPEC_ROOTS: OnceCell<Vec<PathBuf>> = OnceCell::const_new();

async fn spec_roots(ctx: &Context) -> &'static [PathBuf] {
    SPEC_ROOTS
        .get_or_init(|| async {
            let mut roots = Vec::new();

            match fig_util::directories::autocomplete_specs_dir() {
                Ok(dir) => {
                    if tokio::fs::metadata(&dir).await.is_ok_and(|meta| meta.is_dir()) {
                        roots.push(dir);
                    }
                },
                Err(err) => error!(%err, "failed to resolve the user spec dir"),
            }

            match fig_util::directories::resources_path_ctx(ctx) {
                Ok(resources) => roots.push(resources.join("specs")),
                Err(err) => error!(%err, "failed to resolve the bundled spec dir"),
            }

            let mut canonical = Vec::with_capacity(roots.len());
            for root in roots {
                match tokio::fs::canonicalize(&root).await {
                    Ok(root) => canonical.push(root),
                    Err(err) => debug!(%err, root = %root.display(), "spec root is not readable"),
                }
            }
            canonical
        })
        .await
}

/// The union of every root's `index.json`, sorted and deduplicated.
///
/// The index is authoritative for the JS loader: for any name it lists, `publicSpecExists`
/// returns true and the loader imports `spec://localhost/<name>.js` *instead of* falling back to
/// `~/.fig/autocomplete/build` (`packages/autocomplete-parser/src/loadSpec.ts`). A name in the
/// union that no root can actually serve therefore fails to load outright rather than degrading
/// — every root must list only the specs it ships.
///
/// `None` means no root had a parseable `index.json`; the caller answers 404 so the loader's
/// index cache resets and retries, instead of caching "no specs exist" for the session.
async fn index_json_uncached(roots: &[PathBuf]) -> Option<SpecIndex> {
    let mut completions = BTreeSet::new();
    let mut diff_versioned_completions = BTreeSet::new();
    let mut any = false;

    for root in roots {
        let path = root.join("index.json");

        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                if err.kind() != ErrorKind::NotFound {
                    error!(%err, path = %path.display(), "failed to read spec index");
                }
                continue;
            },
        };

        match serde_json::from_slice::<SpecIndex>(&bytes) {
            Ok(index) => {
                any = true;
                completions.extend(index.completions);
                diff_versioned_completions.extend(index.diff_versioned_completions);
            },
            Err(err) => error!(%err, path = %path.display(), "failed to parse spec index"),
        }
    }

    any.then(|| SpecIndex {
        completions: completions.into_iter().collect(),
        diff_versioned_completions: diff_versioned_completions.into_iter().collect(),
    })
}

/// The serialized index, rebuilt only when a root's `index.json` changes.
///
/// The bundled index is 21 KB / 737 entries and was read, parsed and re-serialized on every
/// `spec://localhost/index.json` request. The key is the roots plus each index file's
/// modification time, so editing a spec directory in dev still takes effect and the tests below
/// — which each use their own temp roots — never see each other's entries.
type IndexKey = Vec<(PathBuf, Option<std::time::SystemTime>)>;
/// `None` for the body means "no root had a parseable index", which is cached too.
type CachedIndex = (IndexKey, Option<Arc<Vec<u8>>>);
static INDEX_CACHE: Mutex<Option<CachedIndex>> = Mutex::new(None);

async fn index_key(roots: &[PathBuf]) -> IndexKey {
    let mut key = Vec::with_capacity(roots.len());
    for root in roots {
        let path = root.join("index.json");
        let modified = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|meta| meta.modified().ok());
        key.push((path, modified));
    }
    key
}

async fn index_json(roots: &[PathBuf]) -> Option<Arc<Vec<u8>>> {
    let key = index_key(roots).await;

    if let Some((cached_key, cached)) = INDEX_CACHE.lock().as_ref()
        && *cached_key == key
    {
        return cached.clone();
    }

    let index = index_json_uncached(roots)
        .await
        .and_then(|index| serde_json::to_vec(&index).ok())
        .map(Arc::new);

    *INDEX_CACHE.lock() = Some((key, index.clone()));
    index
}

fn relativize(path: &Path) -> &Path {
    path.strip_prefix("/").unwrap_or(path)
}

/// Resolves a request path inside `root`, rejecting anything that escapes it.
///
/// Both sides are canonicalized before the prefix check, so a symlink inside `root` that points
/// outside of it is rejected too.
async fn resolve_in_root(root: &Path, uri_path: &Path) -> Option<PathBuf> {
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let path = tokio::fs::canonicalize(root.join(relativize(uri_path))).await.ok()?;

    // dont allow escaping the spec dir
    if !path.starts_with(&root) || !tokio::fs::metadata(&path).await.is_ok_and(|meta| meta.is_file()) {
        return None;
    }

    Some(path)
}

async fn handle_roots(roots: &[PathBuf], uri_path: &str) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    if uri_path == "/index.json" {
        return match index_json(roots).await {
            Some(index) => Ok(res_ok((*index).clone(), APPLICATION_JSON)),
            None => Ok(res_404()),
        };
    }

    if !uri_path.ends_with(".js") {
        return Ok(res_404());
    }

    let uri_path = Path::new(uri_path);
    for root in roots {
        let Some(path) = resolve_in_root(root, uri_path).await else {
            continue;
        };

        match tokio::fs::read(&path).await {
            Ok(content) => return Ok(res_ok(content, TEXT_JAVASCRIPT)),
            Err(err) => error!(%err, path = %path.display(), "failed to read spec"),
        }
    }

    Ok(res_404())
}

// handle `spec://localhost/spec.js`
pub async fn handle(
    ctx: Arc<Context>,
    request: Request<Vec<u8>>,
    _: WindowId,
) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    handle_roots(spec_roots(&ctx).await, request.uri().path()).await
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn write(root: &TempDir, name: &str, contents: &str) {
        let path = root.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn roots(user: &TempDir, bundled: &TempDir) -> Vec<PathBuf> {
        vec![user.path().to_owned(), bundled.path().to_owned()]
    }

    fn body<'a>(response: &'a Response<Cow<'static, [u8]>>) -> &'a [u8] {
        response.body().as_ref()
    }

    #[tokio::test]
    async fn index_json_is_the_union_of_every_root() {
        let user = TempDir::new().unwrap();
        let bundled = TempDir::new().unwrap();

        write(
            &user,
            "index.json",
            r#"{"completions":["git","mine"],"diffVersionedCompletions":["az"]}"#,
        );
        write(
            &bundled,
            "index.json",
            r#"{"completions":["cd","git"],"diffVersionedCompletions":["az","fig"]}"#,
        );

        let response = handle_roots(&roots(&user, &bundled), "/index.json").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let index: SpecIndex = serde_json::from_slice(body(&response)).unwrap();
        assert_eq!(index.completions, ["cd", "git", "mine"]);
        assert_eq!(index.diff_versioned_completions, ["az", "fig"]);
    }

    /// A root that omits `diffVersionedCompletions` still contributes its completions.
    #[tokio::test]
    async fn a_root_missing_a_key_is_not_discarded() {
        let user = TempDir::new().unwrap();
        let bundled = TempDir::new().unwrap();

        write(&user, "index.json", r#"{"completions":["mine"]}"#);
        write(
            &bundled,
            "index.json",
            r#"{"completions":["cd"],"diffVersionedCompletions":["az"]}"#,
        );

        let response = handle_roots(&roots(&user, &bundled), "/index.json").await.unwrap();
        let index: SpecIndex = serde_json::from_slice(body(&response)).unwrap();
        assert_eq!(index.completions, ["cd", "mine"]);
        assert_eq!(index.diff_versioned_completions, ["az"]);
    }

    #[tokio::test]
    async fn the_first_root_holding_a_spec_wins() {
        let user = TempDir::new().unwrap();
        let bundled = TempDir::new().unwrap();

        write(&user, "git.js", "user git");
        write(&bundled, "git.js", "bundled git");
        write(&bundled, "cd.js", "bundled cd");
        write(&bundled, "az/index.js", "bundled az");

        let roots = roots(&user, &bundled);

        let response = handle_roots(&roots, "/git.js").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], TEXT_JAVASCRIPT);
        assert_eq!(body(&response), b"user git");

        let response = handle_roots(&roots, "/cd.js").await.unwrap();
        assert_eq!(body(&response), b"bundled cd");

        let response = handle_roots(&roots, "/az/index.js").await.unwrap();
        assert_eq!(body(&response), b"bundled az");
    }

    #[tokio::test]
    async fn an_unknown_spec_is_not_found() {
        let user = TempDir::new().unwrap();
        let bundled = TempDir::new().unwrap();

        write(&bundled, "cd.js", "bundled cd");
        write(
            &bundled,
            "index.json",
            r#"{"completions":["cd"],"diffVersionedCompletions":[]}"#,
        );

        let roots = roots(&user, &bundled);

        assert_eq!(
            handle_roots(&roots, "/nope.js").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            handle_roots(&roots, "/cd").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        let response = handle_roots(&roots, "/index.json").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let index: SpecIndex = serde_json::from_slice(body(&response)).unwrap();
        assert_eq!(index.completions, ["cd"]);
    }

    /// With no readable index anywhere, 404 rather than an empty index: the JS loader caches a
    /// successful index response for the life of the webview, so an empty 200 would disable
    /// every global spec until the next restart.
    #[tokio::test]
    async fn an_unreadable_index_is_not_found() {
        let user = TempDir::new().unwrap();
        let bundled = TempDir::new().unwrap();

        assert_eq!(
            handle_roots(&roots(&user, &bundled), "/index.json")
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            handle_roots(&[], "/index.json").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        // a root whose index.json is unparseable counts as unreadable too
        write(&bundled, "index.json", "{not json");
        assert_eq!(
            handle_roots(&roots(&user, &bundled), "/index.json")
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn escaping_the_spec_root_is_rejected() {
        let user = TempDir::new().unwrap();
        let bundled = TempDir::new().unwrap();

        write(&bundled, "specs/cd.js", "bundled cd");
        write(&bundled, "secret.js", "not a spec");

        let roots = vec![user.path().to_owned(), bundled.path().join("specs")];

        assert_eq!(handle_roots(&roots, "/cd.js").await.unwrap().status(), StatusCode::OK);
        assert_eq!(
            handle_roots(&roots, "/../secret.js").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            handle_roots(&roots, "/../../etc/passwd.js").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    /// A symlink inside a root that resolves outside of it is rejected by the prefix check,
    /// not merely by the target being absent.
    #[tokio::test]
    async fn a_symlink_out_of_the_spec_root_is_rejected() {
        let bundled = TempDir::new().unwrap();

        write(&bundled, "specs/cd.js", "bundled cd");
        write(&bundled, "secret.js", "not a spec");
        std::os::unix::fs::symlink(bundled.path().join("secret.js"), bundled.path().join("specs/leak.js")).unwrap();

        let roots = vec![bundled.path().join("specs")];

        assert_eq!(handle_roots(&roots, "/cd.js").await.unwrap().status(), StatusCode::OK);
        assert_eq!(
            handle_roots(&roots, "/leak.js").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
}
