// src/bot_runner.rs
use bevy::prelude::*;
use bevy_tokio_tasks::TokioTasksRuntime;
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Resource, Default, Clone, Debug)]
pub struct BotProcessStatus {
    pub player_bot_running: bool,
    pub opponent_bot_running: bool,
    pub player_bot_output: Vec<String>,
    pub opponent_bot_output: Vec<String>,
    pub player_bot_error: Option<String>,
    pub opponent_bot_error: Option<String>,
}

#[derive(Event)]
pub struct StartBotProcessesEvent {
    pub player_bot_command: Option<String>,
    pub opponent_bot_command: Option<String>,
}

/// System to handle starting bot processes when the event is triggered
pub fn bot_process_system(
    mut events: EventReader<StartBotProcessesEvent>,
    runtime: Res<TokioTasksRuntime>,
    mut bot_status: ResMut<BotProcessStatus>,
) {
    for event in events.read() {
        // Start player bot if command is provided
        if let Some(cmd) = &event.player_bot_command {
            if cmd.is_empty() {
                continue;
            }

            println!("[bot_runner] Starting player bot: {}", cmd);
            bot_status.player_bot_running = true;
            bot_status.player_bot_error = None;
            bot_status.player_bot_output.clear();

            let cmd_clone = cmd.clone();
            runtime.spawn_background_task(|mut ctx| async move {
                let result = run_bot_command(&cmd_clone, true).await;

                ctx.run_on_main_thread(move |world| {
                    let Some(mut status) = world.world.get_resource_mut::<BotProcessStatus>() else {
                        return;
                    };

                    status.player_bot_running = false;
                    match result {
                        Ok(output) => {
                            println!("[bot_runner] Player bot completed successfully");
                            status.player_bot_output = output;
                        }
                        Err(e) => {
                            eprintln!("[bot_runner] Player bot failed: {}", e);
                            status.player_bot_error = Some(e);
                        }
                    }
                }).await;
            });
        }

        // Start opponent bot if command is provided
        if let Some(cmd) = &event.opponent_bot_command {
            if cmd.is_empty() {
                continue;
            }

            println!("[bot_runner] Starting opponent bot: {}", cmd);
            bot_status.opponent_bot_running = true;
            bot_status.opponent_bot_error = None;
            bot_status.opponent_bot_output.clear();

            let cmd_clone = cmd.clone();
            runtime.spawn_background_task(|mut ctx| async move {
                let result = run_bot_command(&cmd_clone, false).await;

                ctx.run_on_main_thread(move |world| {
                    let Some(mut status) = world.world.get_resource_mut::<BotProcessStatus>() else {
                        return;
                    };

                    status.opponent_bot_running = false;
                    match result {
                        Ok(output) => {
                            println!("[bot_runner] Opponent bot completed successfully");
                            status.opponent_bot_output = output;
                        }
                        Err(e) => {
                            eprintln!("[bot_runner] Opponent bot failed: {}", e);
                            status.opponent_bot_error = Some(e);
                        }
                    }
                }).await;
            });
        }
    }
}

/// Run a bash command asynchronously and capture output
async fn run_bot_command(command: &str, is_player: bool) -> Result<Vec<String>, String> {
    let bot_type = if is_player { "player" } else { "opponent" };
    println!("[bot_runner] Executing {} bot command: {}", bot_type, command);

    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn {} bot process: {}", bot_type, e))?;

    let stdout = child.stdout.take()
        .ok_or_else(|| format!("Failed to capture stdout for {} bot", bot_type))?;
    let stderr = child.stderr.take()
        .ok_or_else(|| format!("Failed to capture stderr for {} bot", bot_type))?;

    let mut output_lines = Vec::new();

    // Spawn task to read stdout
    let stdout_handle = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut collected = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            println!("[bot_runner:{}:stdout] {}", bot_type, line);
            collected.push(line);
        }
        collected
    });

    // Spawn task to read stderr
    let stderr_handle = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut collected = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[bot_runner:{}:stderr] {}", bot_type, line);
            collected.push(line);
        }
        collected
    });

    // Wait for process to complete
    let status = child.wait().await
        .map_err(|e| format!("Failed to wait for {} bot process: {}", bot_type, e))?;

    // Collect output
    let stdout_lines = stdout_handle.await
        .map_err(|e| format!("Failed to join stdout task: {}", e))?;
    let stderr_lines = stderr_handle.await
        .map_err(|e| format!("Failed to join stderr task: {}", e))?;

    output_lines.extend(stdout_lines);
    output_lines.extend(stderr_lines);

    if !status.success() {
        return Err(format!("{} bot process exited with status: {:?}", bot_type, status));
    }

    println!("[bot_runner] {} bot process completed with status: {:?}", bot_type, status);
    Ok(output_lines)
}

