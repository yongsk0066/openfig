//! All arbitrary filesystem access and command spawning happens inside this disposable process.
use std::path::PathBuf;

use fig_util::env_var::PROCESS_LAUNCHED_BY_FIG;
use tokio::process::Command;

use super::{
    Job,
    MAX_FRAME,
    MAX_OUTPUT,
    Output,
    Run,
};

async fn read_output(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    reader
        .take((MAX_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|err| err.to_string())?;
    if bytes.len() > MAX_OUTPUT {
        return Err("Generator output exceeds 1 MiB per stream".into());
    }
    Ok(bytes)
}

fn directory(path: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(path?).canonicalize().ok()?;
    path.is_dir().then_some(path)
}

/// The command's output, or why there is none.
async fn run(request: Run) -> Result<Output, String> {
    let cwd = directory(request.working_directory.as_deref())
        .or_else(|| directory(request.shell_cwd.as_deref()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));

    // A relative executable is looked up on the `PATH` set here, not the desktop's.
    let mut cmd = Command::new(&request.executable);
    cmd.current_dir(cwd).args(&request.arguments).kill_on_drop(true);

    let shell_env = request.shell_env;
    if !shell_env.is_empty() {
        cmd.env_clear().envs(shell_env);
    }
    for key in ["LS_COLORS", "CLICOLOR_FORCE", "CLICOLOR", "COLORTERM"] {
        cmd.env_remove(key);
    }
    cmd.envs([
        (PROCESS_LAUNCHED_BY_FIG, "1"),
        ("HISTFILE", ""),
        ("HISTCONTROL", "ignoreboth"),
        ("TERM", "xterm-256color"),
        ("NO_COLOR", "1"),
    ]);
    for (key, value) in request.env {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed running executable ({}): {err}", request.executable))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (stdout, stderr, status) = tokio::try_join!(read_output(stdout), read_output(stderr), async {
        child.wait().await.map_err(|err| err.to_string())
    })?;
    Ok(Output::Run {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code: status.code().unwrap_or(1),
    })
}

/// Bound both entry count and raw name bytes before allocating the response envelope.
fn directory_names(path: &std::path::Path, classify: bool) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut bytes = 0usize;
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if classify {
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
                name.push('/');
            }
        }
        bytes += name.len();
        if names.len() >= 65_536 || bytes > MAX_OUTPUT {
            return Err("Directory listing exceeds 65536 entries or 1 MiB of names".into());
        }
        names.push(name);
    }
    Ok(names)
}

pub async fn serve() -> std::io::Result<()> {
    use std::io::{
        Read,
        Write,
    };
    let mut input = std::io::stdin();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > MAX_OUTPUT {
        return Err(std::io::Error::other("I/O request exceeds 1 MiB"));
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    let job: Job = serde_json::from_slice(&bytes)?;
    let response = match job {
        Job::Run(request) => run(request).await,
        Job::Directory { path, classify } => directory_names(&path, classify)
            .map(Output::Directory)
            .map_err(|e| format!("Failed listing directory in {}: {e}", path.display())),
    };
    let bytes = serde_json::to_vec(&response)?;
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::other("I/O response exceeds 16 MiB"));
    }
    let mut output = std::io::stdout();
    output.write_all(&(bytes.len() as u32).to_be_bytes())?;
    output.write_all(&bytes)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut names = directory_names(dir.path(), true).unwrap();
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
            directory_names(dir.path(), false)
                .unwrap()
                .contains(&"folder".to_owned())
        );
        assert!(directory_names(&dir.path().join("missing"), true).is_err());
        assert!(directory_names(&dir.path().join("file"), true).is_err());
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
                    directory_names(dir.path(), true).unwrap()
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
