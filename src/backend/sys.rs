//! System helpers: restarting the user's PipeWire services after config
//! changes. Uses `systemctl --user` when the services run under systemd;
//! otherwise (e.g. started by the compositor) it stops the running
//! processes and relaunches them with the same command line and environment.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::conf::Service;

/// A running process, captured so it can be started again identically.
struct Proc {
    pid: u32,
    argv: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
}

fn split_nul(b: Vec<u8>) -> Vec<Vec<u8>> {
    b.split(|c| *c == 0).filter(|s| !s.is_empty()).map(<[u8]>::to_vec).collect()
}

/// Our own processes whose executable name is `name`.
fn find(name: &str) -> Vec<Proc> {
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let Ok(rd) = std::fs::read_dir("/proc") else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let dir = e.path();
        if std::fs::metadata(&dir).map(|m| m.uid()).ok() != uid {
            continue;
        }
        if !std::fs::read_to_string(dir.join("comm")).is_ok_and(|c| c.trim() == name) {
            continue;
        }
        let (Ok(cmd), Ok(env)) = (std::fs::read(dir.join("cmdline")), std::fs::read(dir.join("environ"))) else { continue };
        let argv: Vec<OsString> = split_nul(cmd).into_iter().map(OsString::from_vec).collect();
        if argv.is_empty() {
            continue;
        }
        let env = split_nul(env)
            .into_iter()
            .filter_map(|kv| {
                let i = kv.iter().position(|c| *c == b'=')?;
                Some((OsString::from_vec(kv[..i].to_vec()), OsString::from_vec(kv[i + 1..].to_vec())))
            })
            .collect();
        out.push(Proc { pid, argv, env });
    }
    out
}

async fn systemd_active(unit: &str) -> bool {
    tokio::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

fn binary(s: Service) -> Option<&'static str> {
    match s {
        Service::PipeWire => Some("pipewire"),
        Service::PipeWirePulse => Some("pipewire-pulse"),
        Service::WirePlumber => Some("wireplumber"),
        Service::Clients => None,
    }
}

/// Stop every process named `name`; returns them so they can be relaunched.
async fn stop(name: &str) -> Result<Vec<Proc>, String> {
    let procs = find(name);
    for p in &procs {
        let _ = tokio::process::Command::new("kill").args(["-TERM", &p.pid.to_string()]).status().await;
    }
    for _ in 0..50 {
        if procs.iter().all(|p| !Path::new(&format!("/proc/{}", p.pid)).exists()) {
            return Ok(procs);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("{name} didn't stop"))
}

/// Start a captured process again, detached from audio-tui (it must outlive us).
fn relaunch(p: &Proc) -> Result<(), String> {
    std::process::Command::new("setsid")
        .arg("-f")
        .args(&p.argv)
        .env_clear()
        .envs(p.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("couldn't start {}: {e}", p.argv[0].to_string_lossy()))
        .and_then(|s| if s.success() { Ok(()) } else { Err(format!("couldn't start {}", p.argv[0].to_string_lossy())) })
}

/// Restart processes in dependency order without systemd.
async fn restart_procs(names: &[&str]) -> Result<String, String> {
    let mut stopped = Vec::new();
    for n in names.iter().rev() {
        stopped.push((*n, stop(n).await?));
    }
    stopped.reverse();
    let mut done = Vec::new();
    for (n, procs) in &stopped {
        for p in procs {
            relaunch(p)?;
        }
        if !procs.is_empty() {
            done.push(*n);
            // pipewire must be up before its clients connect.
            tokio::time::sleep(Duration::from_millis(if *n == "pipewire" { 1000 } else { 200 })).await;
        }
    }
    if done.is_empty() {
        return Err(format!("{} isn't running", names.join(", ")));
    }
    Ok(format!("Restarted {}", done.join(", ")))
}

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
    if !systemd_active(units[0]).await {
        let names: Vec<&str> = if services.contains(&Service::PipeWire) {
            vec!["pipewire", "wireplumber", "pipewire-pulse"]
        } else {
            let mut n: Vec<&str> = services.iter().filter_map(|s| binary(*s)).collect();
            n.dedup();
            n
        };
        return restart_procs(&names).await;
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
    if systemd_active("wireplumber.service").await {
        let run = |action: &'static str| async move {
            tokio::process::Command::new("systemctl").args(["--user", action, "wireplumber.service"]).status().await.map_err(|e| e.to_string())
        };
        let stopped = run("stop").await?;
        if !stopped.success() {
            return Err("couldn't stop wireplumber.service".into());
        }
        let r = f().map_err(|e| e.to_string());
        let _ = run("start").await;
        return r;
    }
    let procs = stop("wireplumber").await?;
    if procs.is_empty() {
        return Err("WirePlumber isn't running".into());
    }
    let r = f().map_err(|e| e.to_string());
    for p in &procs {
        relaunch(p)?;
    }
    r
}
