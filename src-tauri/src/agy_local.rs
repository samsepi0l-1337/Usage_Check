//! Probe a running Antigravity app `language_server` for live Model Quota.
//!
//! When Antigravity.app is open it exposes Connect-RPC on a loopback HTTPS
//! port with `--csrf_token`. That is the same source as the in-app
//! "Model Quota" UI (Gemini / Claude+GPT); UsageCheck converts to used %.

use std::process::Command;

use serde_json::Value;

use usage_core::fetch::agy::{parse_agy_quota_summary, parse_agy_user_status, AgyQuota};

#[derive(Clone, Debug)]
struct LocalServer {
    csrf: String,
    ports: Vec<u16>,
}

fn is_antigravity_app_language_server(cmd: &str) -> bool {
    let lower = cmd.to_ascii_lowercase();
    if !(lower.contains("language_server") || lower.contains("language-server")) {
        return false;
    }
    // Prefer the desktop app LS (rich RetrieveUserQuotaSummary). Skip IDE /
    // CLI so a thinner payload does not mask the app.
    if lower.contains("antigravity-ide") || lower.contains("antigravity_cli") {
        return false;
    }
    lower.contains("antigravity")
        && (lower.contains("--app_data_dir antigravity")
            || lower.contains("/antigravity.app/")
            || lower.contains("override_ide_name antigravity"))
}

fn csrf_from_cmdline(cmd: &str) -> Option<String> {
    let mut parts = cmd.split_whitespace();
    while let Some(p) = parts.next() {
        if p == "--csrf_token" {
            return parts.next().map(str::to_string);
        }
        if let Some(rest) = p.strip_prefix("--csrf_token=") {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn discover_servers() -> Vec<LocalServer> {
    let Ok(output) = Command::new("ps").args(["-ax", "-o", "pid=,command="]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((pid_s, cmd)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid_s.trim().parse::<u32>() else {
            continue;
        };
        let cmd = cmd.trim();
        if !is_antigravity_app_language_server(cmd) {
            continue;
        }
        let Some(csrf) = csrf_from_cmdline(cmd) else {
            continue;
        };
        let ports = listen_ports(pid);
        if ports.is_empty() {
            continue;
        }
        out.push(LocalServer { csrf, ports });
    }
    out
}

fn listen_ports(pid: u32) -> Vec<u16> {
    let Ok(output) = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()])
        .output()
    else {
        return Vec::new();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = Vec::new();
    for line in stdout.lines().skip(1) {
        // NAME column like 127.0.0.1:58539
        let Some(addr) = line.split_whitespace().last() else {
            continue;
        };
        if let Some((_, port_s)) = addr.rsplit_once(':') {
            if let Ok(port) = port_s.parse::<u16>() {
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    ports
}

fn local_client() -> Result<reqwest::Client, ()> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|_| ())
}

async fn post_rpc(client: &reqwest::Client, port: u16, csrf: &str, method: &str) -> Option<Value> {
    let url = format!(
        "https://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/{method}"
    );
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .header("X-Codeium-Csrf-Token", csrf)
        .body("{}")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

async fn fetch_from_server(client: &reqwest::Client, server: &LocalServer) -> Option<AgyQuota> {
    for &port in &server.ports {
        let Some(summary) = post_rpc(client, port, &server.csrf, "RetrieveUserQuotaSummary").await
        else {
            continue;
        };
        let mut quota = parse_agy_quota_summary(&summary);
        if quota.pools.is_empty() {
            continue;
        }
        if let Some(status) = post_rpc(client, port, &server.csrf, "GetUserStatus").await {
            let (email, plan) = parse_agy_user_status(&status);
            quota.email = email;
            quota.plan = plan;
        }
        return Some(quota);
    }
    None
}

/// Returns live Antigravity Model Quota when the desktop app language_server
/// is reachable. `None` when Antigravity is not running or the probe fails.
pub async fn fetch_local_quota() -> Option<AgyQuota> {
    let servers = discover_servers();
    if servers.is_empty() {
        return None;
    }
    let client = local_client().ok()?;
    for server in &servers {
        if let Some(q) = fetch_from_server(&client, server).await {
            return Some(q);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_app_language_server_cmdline() {
        let cmd = "/Applications/Antigravity.app/Contents/Resources/bin/language_server \
            --standalone --override_ide_name antigravity --csrf_token abc-123 \
            --app_data_dir antigravity";
        assert!(is_antigravity_app_language_server(cmd));
        assert_eq!(csrf_from_cmdline(cmd).as_deref(), Some("abc-123"));
    }

    #[test]
    fn skips_ide_language_server() {
        let cmd = ".../Antigravity IDE.app/.../language_server --app_data_dir antigravity-ide \
            --csrf_token x";
        assert!(!is_antigravity_app_language_server(cmd));
    }
}
