//! The Docker panel: containers of a host, their actions and logs, over a
//! cached SSH connection per host.

use omnyssh_core::ssh::docker::{self, Action};
use tauri::State;

use crate::dto::{ContainerDto, DockerActionDto, DockerListDto};
use crate::error::CommandError;
use crate::state::GuiState;

fn err(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
    }
}

async fn session(
    state: &GuiState,
    host_name: &str,
) -> Result<omnyssh_core::ssh::session::SshSession, CommandError> {
    let host = state
        .host_by_name(host_name)
        .ok_or_else(|| err(format!("unknown host '{host_name}'")))?;
    state.docker_session(&host).await.map_err(err)
}

/// Every container on `host_name`, running ones with live CPU and memory.
#[tauri::command]
#[specta::specta]
pub async fn docker_list(
    state: State<'_, GuiState>,
    host_name: String,
) -> Result<DockerListDto, CommandError> {
    let ssh = session(&state, &host_name).await?;
    let (containers, sudo) = docker::list(&ssh)
        .await
        .map_err(|e| err(format!("{e:#}")))?;
    Ok(DockerListDto {
        containers: containers.iter().map(ContainerDto::from).collect(),
        sudo,
    })
}

/// Start / stop / restart / pause / unpause / remove a container.
#[tauri::command]
#[specta::specta]
pub async fn docker_action(
    state: State<'_, GuiState>,
    host_name: String,
    id: String,
    action: DockerActionDto,
) -> Result<(), CommandError> {
    let ssh = session(&state, &host_name).await?;
    let action = match action {
        DockerActionDto::Start => Action::Start,
        DockerActionDto::Stop => Action::Stop,
        DockerActionDto::Restart => Action::Restart,
        DockerActionDto::Pause => Action::Pause,
        DockerActionDto::Unpause => Action::Unpause,
        DockerActionDto::Remove => Action::Remove,
    };
    docker::act(&ssh, &id, action)
        .await
        .map_err(|e| err(format!("{e:#}")))
}

/// The last `tail` lines of a container's log.
#[tauri::command]
#[specta::specta]
pub async fn docker_logs(
    state: State<'_, GuiState>,
    host_name: String,
    id: String,
    tail: u32,
) -> Result<String, CommandError> {
    let ssh = session(&state, &host_name).await?;
    docker::logs(&ssh, &id, tail)
        .await
        .map_err(|e| err(format!("{e:#}")))
}

/// What a terminal types to open a shell inside the container.
#[tauri::command]
#[specta::specta]
pub fn docker_shell_command(id: String, sudo: bool) -> Result<String, CommandError> {
    docker::exec_shell_command(&id, sudo).map_err(|e| err(format!("{e:#}")))
}
