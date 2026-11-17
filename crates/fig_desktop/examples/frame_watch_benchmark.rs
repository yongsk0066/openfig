//! Read-only native-query comparison. Does not move windows or restart applications.
use std::path::PathBuf;
use std::time::{
    Duration,
    Instant,
};

use fig_desktop::native::frame_watch::FrameWatch;
use fig_desktop::native::{
    FocusTarget,
    Process,
    QUERY_TIMEOUT,
    Reply,
    Request,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let executable = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("worker path required"))?);
    let pid: i32 = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("terminal PID required"))?
        .to_string_lossy()
        .parse()?;
    let mut process = Process::new(executable);
    let Reply::Activation {
        window: Some(window_id),
        ..
    } = process.call(Request::Activation(pid), QUERY_TIMEOUT).await?
    else {
        anyhow::bail!("terminal window unavailable");
    };
    let target = FocusTarget {
        pid,
        window_id,
        generation: 1,
    };
    process.call(Request::Frame(target), QUERY_TIMEOUT).await?;
    let mut results = Vec::new();
    // Reverse order on the second pair to reduce simple warm-cache/order bias.
    for adaptive in [false, true, true, false] {
        let started = Instant::now();
        let mut watch = FrameWatch::default();
        let mut times = Vec::new();
        let mut missing = 0;
        for tick in 0..100 {
            let at = started + Duration::from_millis(tick * 50);
            tokio::time::sleep_until(at.into()).await;
            if adaptive && !watch.due(at, true, true, false) {
                continue;
            }
            let query = Instant::now();
            if !matches!(
                process.call(Request::Frame(target), QUERY_TIMEOUT).await?,
                Reply::Frame(Some(_))
            ) {
                missing += 1;
            }
            times.push(query.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        results.push(serde_json::json!({
            "adaptive": adaptive, "scheduledSeconds": 5, "queries": times.len(),
            "missingFrames": missing, "queryRoundTripSumMs": times.iter().sum::<f64>(),
            "queryP50Ms": times[times.len()/2], "queryP95Ms": times[times.len()*95/100]
        }));
    }
    process.shutdown().await;
    println!("{}", serde_json::json!({"target": target, "idleVisible": results}));
    Ok(())
}
