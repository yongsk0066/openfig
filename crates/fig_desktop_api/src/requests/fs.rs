use camino::Utf8PathBuf;
use fig_os_shim::Env;
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    AppendToFileRequest,
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

// Reserve space for the protobuf response ID, nested message tags and length prefixes.
const MAX_FILE_BYTES: usize = fig_proto::MAX_MESSAGE_SIZE - 32;

fn read_bounded(reader: impl std::io::Read, size_hint: u64, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{
        Error,
        Read,
    };
    let too_large = || Error::other(format!("file exceeds the {limit}-byte read limit"));
    if size_hint > limit as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(size_hint as usize + 1);
    // The file may grow after metadata was read, or report zero length until read.
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(too_large());
    }
    Ok(bytes)
}

pub(crate) async fn read_file(request: ReadFileRequest, env: &Env) -> RequestResult {
    use fig_proto::fig::read_file_response::Type;
    let path = request.path.as_ref().ok_or("No path provided")?;
    let resolved_path = resolve_filepath(path, env).into_owned();
    let binary = request.is_binary_file();
    let kind = tokio::task::spawn_blocking(move || {
        let read = || -> std::io::Result<Type> {
            let file = std::fs::File::open(&*resolved_path)?;
            let hint = file.metadata()?.len();
            let bytes = read_bounded(file, hint, MAX_FILE_BYTES)?;
            if binary {
                Ok(Type::Data(bytes))
            } else {
                String::from_utf8(bytes).map(Type::Text).map_err(|_invalid_utf8| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "stream did not contain valid UTF-8")
                })
            }
        };
        read().map_err(|err| format!("Failed reading file at {resolved_path}: {err}"))
    })
    .await
    .map_err(|err| err.to_string())??;
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

    #[test]
    fn file_size_hint_cannot_bypass_read_limit() {
        assert_eq!(read_bounded(&b"abcd"[..], 0, 4).unwrap(), b"abcd");
        assert!(read_bounded(&b"abcde"[..], 1, 4).is_err());
        assert!(read_bounded(std::io::empty(), 5, 4).is_err());
        let mut endless = std::io::Cursor::new([0; 100]);
        assert!(read_bounded(&mut endless, 0, 4).is_err());
        assert_eq!(endless.position(), 5);
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
}
