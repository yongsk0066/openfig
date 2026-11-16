use camino::Utf8PathBuf;
use fig_os_shim::Env;
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    AppendToFileRequest,
    ContentsOfDirectoryRequest,
    ContentsOfDirectoryResponse,
    DestinationOfSymbolicLinkRequest,
    DestinationOfSymbolicLinkResponse,
    ReadFileRequest,
    ReadFileResponse,
    WriteFileRequest,
};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use super::{
    RequestResult,
    RequestResultImpl,
};
use crate::util::{
    build_filepath,
    resolve_filepath,
};

pub(crate) async fn read_file(request: ReadFileRequest, env: &Env) -> RequestResult {
    use fig_proto::fig::read_file_response::Type;
    let path = request.path.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env);
    let kind = if request.is_binary_file() {
        Type::Data(
            tokio::fs::read(&*resolved_path)
                .await
                .map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))?,
        )
    } else {
        Type::Text(
            tokio::fs::read_to_string(&*resolved_path)
                .await
                .map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))?,
        )
    };
    let response = ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse { r#type: Some(kind) });

    Ok(response.into())
}

pub(crate) async fn write_file(request: WriteFileRequest, env: &Env) -> RequestResult {
    use fig_proto::fig::write_file_request::Data;
    let path = request.path.as_ref().ok_or_else(|| "No path provided".to_string())?;
    let data = request.data.ok_or("No data provided")?;
    let resolved_path = resolve_filepath(path, env);
    match data {
        Data::Binary(data) => tokio::fs::write(&*resolved_path, data)
            .await
            .map_err(|err| format!("Failed writing to file at {resolved_path}: {err}"))?,
        Data::Text(data) => tokio::fs::write(&*resolved_path, data.as_bytes())
            .await
            .map_err(|err| format!("Failed writing to file at {resolved_path}: {err}"))?,
    }

    RequestResult::success()
}

pub(crate) async fn append_to_file(request: AppendToFileRequest, env: &Env) -> RequestResult {
    use fig_proto::fig::append_to_file_request::Data;
    let path = request.path.as_ref().ok_or_else(|| "No path provided".to_string())?;
    let data = request.data.ok_or("No data provided")?;
    let resolved_path = resolve_filepath(path, env);
    let mut file = OpenOptions::new()
        .append(true)
        .open(&*resolved_path)
        .await
        .map_err(|err| format!("Failed opening file at {resolved_path}: {err}"))?;

    let bytes = match &data {
        Data::Binary(data) => data.as_slice(),
        Data::Text(data) => data.as_bytes(),
    };
    // `write` is one `write(2)`, and tokio's `File` also caps a single write at 2 MiB, so a larger
    // append used to report success after writing only part of the data. `File` hands the bytes
    // to a blocking task and returns before they reach the file; `flush` waits for that write, so
    // its error is reported here and a read that follows this response sees the appended data.
    file.write_all(bytes)
        .await
        .map_err(|err| format!("Failed writing to file at {resolved_path}: {err}"))?;
    file.flush()
        .await
        .map_err(|err| format!("Failed writing to file at {resolved_path}: {err}"))?;

    RequestResult::success()
}

pub(crate) async fn destination_of_symbolic_link(
    request: DestinationOfSymbolicLinkRequest,
    env: &Env,
) -> RequestResult {
    let path = request.path.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env);
    let real_path: Utf8PathBuf = tokio::fs::canonicalize(&*resolved_path)
        .await
        .map_err(|err| format!("Failed resolving symlink at {resolved_path}: {err}"))?
        .try_into()
        .map_err(|err| format!("Path is not Utf8: {err}"))?;

    let response = ServerOriginatedSubMessage::DestinationOfSymbolicLinkResponse(DestinationOfSymbolicLinkResponse {
        destination: Some(build_filepath(real_path)),
    });

    Ok(response.into())
}

/// Structured names preserve embedded newlines; metadata follows symlinks like ls -L.
fn list_directory(path: &std::path::Path, classify: bool) -> std::io::Result<Vec<String>> {
    std::fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if classify {
                let kind = entry.file_type()?;
                if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
                    name.push('/');
                }
            }
            Ok(name)
        })
        .collect()
}

pub(crate) async fn contents_of_directory(request: ContentsOfDirectoryRequest, env: &Env) -> RequestResult {
    let path = request.directory.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env);
    let resolved_path = resolved_path.into_owned();
    let display_path = resolved_path.to_string();
    let classify = request.append_directory_separator();
    // One blocking-pool job, rather than one async filesystem round-trip per entry.
    let contents = tokio::task::spawn_blocking(move || list_directory(resolved_path.as_std_path(), classify))
        .await
        .map_err(|err| format!("Directory worker failed: {err}"))?
        .map_err(|err| format!("Failed listing directory in {display_path}: {err}"))?;

    let response =
        ServerOriginatedSubMessage::ContentsOfDirectoryResponse(ContentsOfDirectoryResponse { file_names: contents });

    Ok(response.into())
}

pub(crate) async fn create_directory_request(
    request: fig_proto::fig::CreateDirectoryRequest,
    env: &Env,
) -> RequestResult {
    let path = request.path.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env);
    if request.recursive() {
        tokio::fs::create_dir_all(&*resolved_path)
            .await
            .map_err(|err| format!("Failed to create dir: {err}"))?;
    } else {
        tokio::fs::create_dir(&*resolved_path)
            .await
            .map_err(|err| format!("Failed to create dir: {err}"))?;
    }

    RequestResult::success()
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use fig_proto::fig::{
        AppendToFileRequest,
        FilePath,
        ReadFileRequest,
        WriteFileRequest,
    };

    use super::*;

    fn temp_file(dir: &tempfile::TempDir, name: &str) -> Utf8PathBuf {
        Utf8PathBuf::try_from(dir.path().join(name)).unwrap()
    }

    fn file_path(path: &Utf8PathBuf) -> Option<FilePath> {
        Some(FilePath {
            path: path.to_string(),
            relative_to: None,
            expand_tilde_in_path: None,
        })
    }

    fn error_message(result: RequestResult) -> String {
        match result {
            Err(err) => err.to_string(),
            Ok(response) => panic!("expected an error, got {response:?}"),
        }
    }

    #[tokio::test]
    async fn test_read_file() {
        use fig_proto::fig::read_file_response::Type;

        let dir = tempfile::tempdir().unwrap();
        let env = Env::new();

        let path = temp_file(&dir, "test.txt");
        let content = "Hello, World!";
        std::fs::write(&path, content).unwrap();

        let mut request = ReadFileRequest {
            path: Some(FilePath {
                path: path.to_string(),
                relative_to: None,
                expand_tilde_in_path: None,
            }),
            ..Default::default()
        };

        let result = read_file(request.clone(), &env).await.unwrap();
        let text = match *result {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Text(text)),
            }) => text,
            _ => panic!("Unexpected response type"),
        };
        assert_eq!(content, text);

        request.is_binary_file = Some(true);

        let result = read_file(request, &env).await.unwrap();
        let data = match *result {
            ServerOriginatedSubMessage::ReadFileResponse(ReadFileResponse {
                r#type: Some(Type::Data(data)),
            }) => data,
            _ => panic!("Unexpected response type"),
        };
        assert_eq!(content.as_bytes(), &data[..]);
    }

    #[tokio::test]
    async fn test_write_file() {
        use fig_proto::fig::write_file_request::Data;

        let dir = tempfile::tempdir().unwrap();
        let env = Env::new();

        let path = temp_file(&dir, "test.txt");
        let content = "Hello, World!";

        let mut request = WriteFileRequest {
            path: Some(FilePath {
                path: path.to_string(),
                relative_to: None,
                expand_tilde_in_path: None,
            }),
            data: Some(Data::Text(content.to_string())),
        };

        write_file(request.clone(), &env).await.unwrap();

        let read_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, read_content);

        request.data = Some(Data::Binary(content.as_bytes().to_vec()));
        write_file(request, &env).await.unwrap();

        let read_content = std::fs::read(&path).unwrap();
        assert_eq!(content.as_bytes(), &read_content[..]);
    }

    /// A request with a path but no `data` used to hit `request.data.unwrap()` and panic the
    /// handler instead of answering with an error.
    #[tokio::test]
    async fn write_file_without_data_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_file(&dir, "test.txt");

        let request = WriteFileRequest {
            path: file_path(&path),
            data: None,
        };
        assert_eq!(
            error_message(write_file(request, &Env::new()).await),
            "No data provided"
        );
        assert!(!path.exists(), "a request without data must not create the file");
    }

    #[tokio::test]
    async fn append_to_file_without_data_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_file(&dir, "test.txt");
        std::fs::write(&path, "unchanged").unwrap();

        let request = AppendToFileRequest {
            path: file_path(&path),
            data: None,
        };
        assert_eq!(
            error_message(append_to_file(request, &Env::new()).await),
            "No data provided"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "unchanged");
    }

    #[tokio::test]
    async fn append_to_file_appends_text_and_binary() {
        use fig_proto::fig::append_to_file_request::Data;

        let dir = tempfile::tempdir().unwrap();
        let env = Env::new();
        let path = temp_file(&dir, "test.txt");
        std::fs::write(&path, "a").unwrap();

        for data in [Data::Text("b".into()), Data::Binary(b"c".to_vec())] {
            let request = AppendToFileRequest {
                path: file_path(&path),
                data: Some(data),
            };
            append_to_file(request, &env).await.unwrap();
        }

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
    }

    /// tokio's `File::write` takes at most 2 MiB per call, so with `write` instead of `write_all`
    /// this append reported success after writing two thirds of the data.
    #[tokio::test]
    async fn append_to_file_writes_everything() {
        use fig_proto::fig::append_to_file_request::Data;

        let dir = tempfile::tempdir().unwrap();
        let path = temp_file(&dir, "test.bin");
        std::fs::write(&path, b"").unwrap();

        let data: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let request = AppendToFileRequest {
            path: file_path(&path),
            data: Some(Data::Binary(data.clone())),
        };
        append_to_file(request, &Env::new()).await.unwrap();

        let written = std::fs::read(&path).unwrap();
        assert_eq!(written.len(), data.len());
        assert!(written == data);
    }
    #[test]
    fn structured_directory_preserves_names_and_follows_directory_links() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        for name in ["file", ".hidden", "line\nbreak", "한글"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        symlink("folder", dir.path().join("link-dir")).unwrap();
        symlink("file", dir.path().join("link-file")).unwrap();
        symlink("missing", dir.path().join("broken")).unwrap();
        let mut names = list_directory(dir.path(), true).unwrap();
        names.sort();
        let mut expected = vec![
            "folder/",
            "file",
            ".hidden",
            "line\nbreak",
            "한글",
            "link-dir/",
            "link-file",
            "broken",
        ];
        expected.sort();
        assert_eq!(names, expected);
        assert!(
            list_directory(dir.path(), false)
                .unwrap()
                .contains(&"folder".to_owned())
        );
        assert!(list_directory(&dir.path().join("missing"), true).is_err());
        assert!(list_directory(&dir.path().join("file"), true).is_err());
    }

    /// Same fixture and output contract, alternating old subprocess/new native calls.
    /// Excludes transport and React, and does not assert noisy timing thresholds.
    #[test]
    #[ignore = "release directory backend performance comparison"]
    fn directory_backend_benchmark() {
        use std::time::Instant;
        let dir = tempfile::tempdir().unwrap();
        for i in 0..1000 {
            if i % 10 == 0 {
                std::fs::create_dir(dir.path().join(format!("entry-{i}"))).unwrap();
            } else {
                std::fs::write(dir.path().join(format!("entry-{i}")), "").unwrap();
            }
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        for iteration in 0..60 {
            for native in if iteration % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let start = Instant::now();
                let mut names = if native {
                    list_directory(dir.path(), true).unwrap()
                } else {
                    let output = std::process::Command::new("/bin/ls")
                        .arg("-1ApL")
                        .current_dir(dir.path())
                        .output()
                        .unwrap();
                    assert!(output.status.success());
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .lines()
                        .map(str::to_owned)
                        .collect()
                };
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                names.sort();
                assert_eq!(names.len(), 1000);
                assert_eq!(names.iter().filter(|name| name.ends_with('/')).count(), 100);
                if iteration >= 10 {
                    if native {
                        after.push(elapsed);
                    } else {
                        before.push(elapsed);
                    }
                }
            }
        }
        for (label, mut samples) in [("ls-before", before), ("native-after", after)] {
            samples.sort_by(f64::total_cmp);
            println!(
                "directory-backend {label} n={} p50={:.3}ms p95={:.3}ms max={:.3}ms",
                samples.len(),
                (samples[24] + samples[25]) / 2.0,
                samples[47],
                samples[49]
            );
        }
    }
}
