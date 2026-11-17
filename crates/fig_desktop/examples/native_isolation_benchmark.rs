//! Alternating direct/isolated icon calls. This measures query cost, not key-to-pixel latency.
use std::path::PathBuf;
use std::time::{
    Duration,
    Instant,
};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use fig_desktop::native::{
    Icon,
    Process,
    Reply,
    Request,
};
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;

fn summary(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    serde_json::json!({"n":samples.len(),"p50Ms":samples[samples.len()/2],"p95Ms":samples[samples.len()*95/100],"maxMs":samples.last()})
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let executable = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("pass fig_native_worker path"))?;
    let mut process = Process::new(executable);
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("main thread required"))?;
    let request = || Request::Icon(Icon::Path("/Applications/Ghostty.app".into()));
    let cold = Instant::now();
    process.call(request(), Duration::from_secs(5)).await?;
    let cold_ms = cold.elapsed().as_secs_f64() * 1000.0;
    let direct_cold = Instant::now();
    anyhow::ensure!(
        autoreleasepool(|_| macos_utils::image::png_for_path(mtm, std::path::Path::new("/Applications/Ghostty.app")))
            .is_some(),
        "missing direct warmup icon"
    );
    let direct_cold_ms = direct_cold.elapsed().as_secs_f64() * 1000.0;
    let mut direct = Vec::new();
    let mut isolated = Vec::new();
    for iteration in 0..60 {
        let mut native_bytes = None;
        let mut isolated_bytes = None;
        for isolate in if iteration % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            if isolate {
                let Reply::Icon(Some(data)) = process.call(request(), Duration::from_secs(2)).await? else {
                    anyhow::bail!("missing icon");
                };
                isolated_bytes = Some(STANDARD.decode(data)?);
                isolated.push(start.elapsed().as_secs_f64() * 1000.0);
            } else {
                native_bytes = autoreleasepool(|_| {
                    macos_utils::image::png_for_path(mtm, std::path::Path::new("/Applications/Ghostty.app"))
                });
                direct.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let a =
            image::load_from_memory(&native_bytes.ok_or_else(|| anyhow::anyhow!("missing direct icon"))?)?.to_rgba8();
        let b = image::load_from_memory(&isolated_bytes.ok_or_else(|| anyhow::anyhow!("missing isolated icon"))?)?
            .to_rgba8();
        anyhow::ensure!(a == b, "icon pixels changed");
    }
    process.shutdown().await;
    println!(
        "{}",
        serde_json::json!({"coldIsolatedMs":cold_ms,"coldDirectMs":direct_cold_ms,"directMainThread":summary(direct),"isolatedRoundTrip":summary(isolated),"pixelEquivalent":true})
    );
    Ok(())
}
