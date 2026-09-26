//! Docker on a host, over SSH: list containers with live CPU/memory, start /
//! stop / restart / pause / remove them, and read their logs. Uses the `docker`
//! CLI on the server; when the login user is not in the `docker` group, it
//! retries once with passwordless `sudo`.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;

use crate::ssh::remote_fs::quote;
use crate::ssh::session::{ScriptOutput, SshSession};

const LIST_BUDGET: Duration = Duration::from_secs(20);
const ACTION_BUDGET: Duration = Duration::from_secs(120);

/// One container, merged from `docker ps` and `docker stats`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, `restarting`, `created`, `dead`.
    pub state: String,
    /// Human status, e.g. `Up 3 hours (healthy)`.
    pub status: String,
    pub ports: String,
    pub created: String,
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

/// What can be done to a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Remove,
}

impl Action {
    fn args(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::Remove => "rm -f",
        }
    }
}

/// A container id or name as `docker` prints them — nothing that could be
/// read as an option or shell syntax.
fn checked_ref(id: &str) -> Result<&str> {
    if id.is_empty()
        || id.starts_with('-')
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        bail!("invalid container reference: {id:?}");
    }
    Ok(id)
}

/// Run `docker <args>`, retrying with `sudo -n` when the socket refuses us.
/// Returns the output and whether sudo was needed.
async fn docker(ssh: &SshSession, args: &str, budget: Duration) -> Result<(ScriptOutput, bool)> {
    let plain = ssh.run_script(&format!("docker {args}"), budget).await?;
    if plain.ok() {
        return Ok((plain, false));
    }
    let err = plain.error_text().to_lowercase();
    if err.contains("command not found") || err.contains("docker: not found") {
        bail!("Docker is not installed on this server");
    }
    if err.contains("permission denied") {
        let sudo = ssh
            .run_script(&format!("sudo -n docker {args}"), budget)
            .await?;
        if sudo.ok() {
            return Ok((sudo, true));
        }
        if sudo.error_text().contains("password") {
            bail!("This user may not use Docker: add it to the docker group, or allow passwordless sudo");
        }
        return Err(anyhow!(sudo.error_text()));
    }
    Err(anyhow!(plain.error_text()))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PsLine {
    #[serde(rename = "ID")]
    id: String,
    names: String,
    image: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    ports: String,
    #[serde(default)]
    created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StatsLine {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "CPUPerc", default)]
    cpu: String,
    #[serde(default)]
    mem_usage: String,
}

/// Parse `docker ps -a --format '{{json .}}'` output (one object per line).
pub fn parse_ps(out: &str) -> Vec<Container> {
    out.lines()
        .filter_map(|l| serde_json::from_str::<PsLine>(l.trim()).ok())
        .map(|p| {
            // Older daemons omit State; derive it from the status text.
            let state = if p.state.is_empty() {
                let s = p.status.to_lowercase();
                if s.starts_with("up") && s.contains("paused") {
                    "paused"
                } else if s.starts_with("up") {
                    "running"
                } else if s.starts_with("restarting") {
                    "restarting"
                } else if s.starts_with("created") {
                    "created"
                } else {
                    "exited"
                }
                .to_string()
            } else {
                p.state
            };
            Container {
                id: p.id,
                name: p.names,
                image: p.image,
                state,
                status: p.status,
                ports: p.ports,
                created: p.created_at,
                cpu: None,
                memory: None,
            }
        })
        .collect()
}

/// Fold `docker stats --no-stream --format '{{json .}}'` into `containers`.
pub fn merge_stats(containers: &mut [Container], out: &str) {
    let stats: HashMap<String, StatsLine> = out
        .lines()
        .filter_map(|l| serde_json::from_str::<StatsLine>(l.trim()).ok())
        .map(|s| (s.name.clone(), s))
        .collect();
    for c in containers.iter_mut() {
        let hit = stats.get(&c.name).or_else(|| {
            stats
                .values()
                .find(|s| !s.id.is_empty() && c.id.starts_with(&s.id))
        });
        if let Some(s) = hit {
            c.cpu = (!s.cpu.is_empty()).then(|| s.cpu.clone());
            c.memory = (!s.mem_usage.is_empty()).then(|| s.mem_usage.clone());
        }
    }
}

/// All containers, running ones with their CPU and memory. Also says whether
/// `sudo` was needed (the UI prefixes `docker exec` the same way).
pub async fn list(ssh: &SshSession) -> Result<(Vec<Container>, bool)> {
    let (ps, sudo) = docker(ssh, "ps -a --no-trunc --format '{{json .}}'", LIST_BUDGET).await?;
    let mut containers = parse_ps(&ps.stdout);
    if containers.iter().any(|c| c.state == "running") {
        if let Ok((stats, _)) =
            docker(ssh, "stats --no-stream --format '{{json .}}'", LIST_BUDGET).await
        {
            merge_stats(&mut containers, &stats.stdout);
        }
    }
    containers.sort_by(|a, b| {
        (a.state != "running")
            .cmp(&(b.state != "running"))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok((containers, sudo))
}

/// Start / stop / restart / pause / unpause / remove a container.
pub async fn act(ssh: &SshSession, id: &str, action: Action) -> Result<()> {
    let id = checked_ref(id)?;
    docker(
        ssh,
        &format!("{} {}", action.args(), quote(id)),
        ACTION_BUDGET,
    )
    .await?;
    Ok(())
}

/// The last `tail` lines of a container's log (stdout and stderr interleaved).
pub async fn logs(ssh: &SshSession, id: &str, tail: u32) -> Result<String> {
    let id = checked_ref(id)?;
    let (out, _) = docker(
        ssh,
        &format!(
            "logs --timestamps --tail {} {} 2>&1",
            tail.clamp(1, 20_000),
            quote(id)
        ),
        LIST_BUDGET,
    )
    .await?;
    Ok(out.stdout)
}

/// The command a terminal types to open a shell inside a container: bash when
/// the image has it, sh otherwise.
pub fn exec_shell_command(id: &str, sudo: bool) -> Result<String> {
    let id = checked_ref(id)?;
    Ok(format!(
        "{}docker exec -it {} sh -c 'command -v bash >/dev/null 2>&1 && exec bash || exec sh'",
        if sudo { "sudo " } else { "" },
        quote(id)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = r#"{"Command":"\"nginx\"","CreatedAt":"2026-09-01 10:00:00 +0000 UTC","ID":"abc123def456","Image":"nginx:1.27","Labels":"","Names":"web","Ports":"0.0.0.0:80->80/tcp","State":"running","Status":"Up 3 hours"}
{"ID":"0011223344","Image":"postgres:16","Names":"db","Ports":"","Status":"Exited (0) 2 days ago","CreatedAt":"x"}
not json
"#;

    #[test]
    fn parses_ps_and_merges_stats() {
        let mut cs = parse_ps(PS);
        assert_eq!(cs.len(), 2);
        assert_eq!(
            (cs[0].name.as_str(), cs[0].state.as_str()),
            ("web", "running")
        );
        // Missing State is derived from Status.
        assert_eq!(cs[1].state, "exited");
        merge_stats(
            &mut cs,
            r#"{"ID":"abc123def456","Name":"web","CPUPerc":"1.25%","MemUsage":"20MiB / 1.9GiB","MemPerc":"1%"}"#,
        );
        assert_eq!(cs[0].cpu.as_deref(), Some("1.25%"));
        assert_eq!(cs[0].memory.as_deref(), Some("20MiB / 1.9GiB"));
        assert_eq!(cs[1].cpu, None);
    }

    #[test]
    fn container_refs_cannot_smuggle_options_or_syntax() {
        assert!(checked_ref("web_1.prod-2").is_ok());
        for bad in ["", "-rf", "a b", "x;rm", "$(id)", "a/b"] {
            assert!(checked_ref(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            exec_shell_command("web", true).unwrap(),
            "sudo docker exec -it 'web' sh -c 'command -v bash >/dev/null 2>&1 && exec bash || exec sh'"
        );
    }
}
