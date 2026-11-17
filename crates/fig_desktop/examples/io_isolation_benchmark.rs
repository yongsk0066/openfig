//! Alternating direct backend/isolated round trips, not terminal-to-display latency.
use std::path::{
    Path,
    PathBuf,
};
use std::time::Instant;

use fig_shell_ipc::process::{
    Request,
    Service,
    Shell,
};

fn summary(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    serde_json::json!({"n":samples.len(),"p50Ms":samples[samples.len()/2],"p95Ms":samples[samples.len()*95/100]})
}

fn direct_directory(path: &Path) -> std::io::Result<Vec<String>> {
    std::fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let mut name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type()?;
            if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
                name.push('/');
            }
            Ok(name)
        })
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let executable = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("pass fig_io_worker path"))?;
    let service = Service::new(executable);
    let fixture = tempfile::tempdir()?;
    for i in 0..1000 {
        let path = fixture.path().join(format!("entry-{i}"));
        if i % 10 == 0 {
            std::fs::create_dir(path)?;
        } else {
            std::fs::write(path, "")?;
        }
    }
    let mut report = serde_json::Map::new();
    for directory in [true, false] {
        let mut direct = Vec::new();
        let mut isolated = Vec::new();
        for iteration in 0..60 {
            let mut expected = None;
            for isolate in if iteration % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let start = Instant::now();
                let mut output = match (directory, isolate) {
                    (true, false) => direct_directory(fixture.path())?,
                    (true, true) => service
                        .directory(fixture.path().to_owned(), true)
                        .await
                        .map_err(anyhow::Error::msg)?,
                    (false, false) => {
                        let cwd = fixture.path().canonicalize()?;
                        anyhow::ensure!(cwd.is_dir());
                        let result = tokio::process::Command::new("/bin/pwd")
                            .current_dir(cwd)
                            .output()
                            .await?;
                        anyhow::ensure!(result.status.success());
                        vec![String::from_utf8(result.stdout)?]
                    },
                    (false, true) => {
                        let result = service
                            .run(
                                Request {
                                    executable: "/bin/pwd".into(),
                                    arguments: vec![],
                                    working_directory: Some(fixture.path().to_string_lossy().into_owned()),
                                    env: vec![],
                                    timeout: None,
                                },
                                Shell { env: vec![], cwd: None },
                            )
                            .await
                            .map_err(anyhow::Error::msg)?;
                        anyhow::ensure!(result.exit_code == 0);
                        vec![result.stdout]
                    },
                };
                if iteration >= 10 {
                    (if isolate { &mut isolated } else { &mut direct }).push(start.elapsed().as_secs_f64() * 1000.0);
                }
                output.sort();
                if let Some(expected) = &expected {
                    anyhow::ensure!(expected == &output, "outputs differ");
                }
                expected = Some(output);
            }
        }
        report.insert(
            if directory { "directory1000" } else { "pwd" }.into(),
            serde_json::json!({"direct":summary(direct),"isolated":summary(isolated)}),
        );
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
