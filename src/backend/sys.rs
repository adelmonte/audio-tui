//! System helpers: restarting the user's PipeWire services after config
//! changes. Runs `systemctl --user` (what the Arch packages use).

use super::conf::Service;

pub async fn restart(services: &[Service]) -> Result<String, String> {
    // pipewire.service restarts pipewire-pulse and wireplumber with it.
    let units: Vec<&str> = if services.contains(&Service::PipeWire) {
        vec!["pipewire.service", "pipewire-pulse.service", "wireplumber.service"]
    } else {
        let mut u: Vec<&str> = services.iter().filter_map(|s| s.unit()).collect();
        u.sort();
        u.dedup();
        u
    };
    if units.is_empty() {
        return Ok("Nothing to restart (apps pick up client rules when they next start)".into());
    }
    let out = tokio::process::Command::new("systemctl")
        .arg("--user")
        .arg("restart")
        .args(&units)
        .output()
        .await
        .map_err(|e| format!("couldn't run systemctl: {e}"))?;
    if out.status.success() {
        Ok(format!("Restarted {}", units.join(", ")))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Stop WirePlumber, run `f`, start it again. Used for editing its state
/// files, which WirePlumber would otherwise overwrite from memory.
pub async fn with_wireplumber_stopped<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> std::io::Result<T>,
{
    let run = |action: &'static str| async move {
        tokio::process::Command::new("systemctl").args(["--user", action, "wireplumber.service"]).status().await.map_err(|e| e.to_string())
    };
    let stopped = run("stop").await?;
    if !stopped.success() {
        return Err("couldn't stop wireplumber.service".into());
    }
    let r = f().map_err(|e| e.to_string());
    let _ = run("start").await;
    r
}
