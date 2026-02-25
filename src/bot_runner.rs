// src/bot_runner.rs
use bevy::prelude::*;
use bevy_tokio_tasks::TokioTasksRuntime;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::app_settings::get_data_dir;
use crate::proxy_channel::ProxyReadySignal;
use crate::ProxyReadyResource;

/// Format a SystemTime as a simple timestamp string
fn format_timestamp() -> String {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("{}s", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}

#[derive(Resource, Default, Clone, Debug)]
pub struct BotProcessStatus {
    pub player_bot_running: bool,
    pub opponent_bot_running: bool,
    pub player_bot_output: Vec<String>,
    pub opponent_bot_output: Vec<String>,
    pub player_bot_error: Option<String>,
    pub opponent_bot_error: Option<String>,
}

#[derive(Message)]
pub struct StartBotProcessesEvent {
    pub player_bot_command: Option<String>,
    pub opponent_bot_command: Option<String>,
    pub player_name: Option<String>,
    pub opponent_name: Option<String>,
    /// Base listen port for proxies. Player1 = base, Player2 = base+1.
    pub listen_port: u16,
}

/// Get log file path for a bot
fn get_bot_log_path(name: &str) -> PathBuf {
    let logs_dir = get_data_dir().join("logs");
    std::fs::create_dir_all(&logs_dir).ok();
    logs_dir.join(format!("{}.log", name))
}

/// System to handle starting bot processes when the event is triggered
pub fn bot_process_system(
    mut events: MessageReader<StartBotProcessesEvent>,
    runtime: Res<TokioTasksRuntime>,
    mut bot_status: ResMut<BotProcessStatus>,
    proxy_ready: Res<ProxyReadyResource>,
) {
    for event in events.read() {
        // Clone the ready signal for use in spawned tasks
        let ready_signal = proxy_ready.0.clone();

        // Start player bot if command is provided
        if let Some(cmd) = &event.player_bot_command {
            if cmd.is_empty() {
                continue;
            }
            
            let player_name = event.player_name.clone().unwrap_or_else(|| "player".to_string());
            let log_path = get_bot_log_path(&player_name);
            let player_port = event.listen_port;
            
            println!("[bot_runner] Queueing player bot '{}': {}", player_name, cmd);
            println!("[bot_runner] Logging to: {:?}", log_path);
            bot_status.player_bot_running = true;
            bot_status.player_bot_error = None;
            bot_status.player_bot_output.clear();

            let cmd_clone = cmd.clone();
            let player_name_clone = player_name.clone();
            let ready_signal_clone = ready_signal.clone();
            runtime.spawn_background_task(move |mut ctx| async move {
                // Wait for Player1 proxy to be ready (count >= 1)
                if let Some(signal) = &ready_signal_clone {
                    println!("[bot_runner] Waiting for Player1 proxy before starting '{}'...", player_name_clone);
                    signal.wait_for_count(1).await;
                    println!("[bot_runner] Player1 proxy ready, starting '{}'", player_name_clone);
                }

                let result = run_bot_command(&cmd_clone, &player_name_clone, &log_path, player_port).await;

                ctx.run_on_main_thread(move |world| {
                    let Some(mut status) = world.world.get_resource_mut::<BotProcessStatus>() else {
                        return;
                    };
                    
                    status.player_bot_running = false;
                    match result {
                        Ok(output) => {
                            println!("[bot_runner] Player bot '{}' completed successfully", player_name_clone);
                            status.player_bot_output = output;
                        }
                        Err(e) => {
                            eprintln!("[bot_runner] Player bot '{}' failed: {}", player_name_clone, e);
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
            
            let opponent_name = event.opponent_name.clone().unwrap_or_else(|| "opponent".to_string());
            let log_path = get_bot_log_path(&opponent_name);
            let opponent_port = event.listen_port + 1;
            
            println!("[bot_runner] Queueing opponent bot '{}': {}", opponent_name, cmd);
            println!("[bot_runner] Logging to: {:?}", log_path);
            bot_status.opponent_bot_running = true;
            bot_status.opponent_bot_error = None;
            bot_status.opponent_bot_output.clear();

            let cmd_clone = cmd.clone();
            let opponent_name_clone = opponent_name.clone();
            let ready_signal_clone = ready_signal.clone();
            runtime.spawn_background_task(move |mut ctx| async move {
                // Wait for Player2 proxy to be ready (count >= 2, both proxies)
                if let Some(signal) = &ready_signal_clone {
                    println!("[bot_runner] Waiting for Player2 proxy before starting '{}'...", opponent_name_clone);
                    signal.wait_for_count(2).await;
                    println!("[bot_runner] Player2 proxy ready, starting '{}'", opponent_name_clone);
                }

                let result = run_bot_command(&cmd_clone, &opponent_name_clone, &log_path, opponent_port).await;

                ctx.run_on_main_thread(move |world| {
                    let Some(mut status) = world.world.get_resource_mut::<BotProcessStatus>() else {
                        return;
                    };
                    
                    status.opponent_bot_running = false;
                    match result {
                        Ok(output) => {
                            println!("[bot_runner] Opponent bot '{}' completed successfully", opponent_name_clone);
                            status.opponent_bot_output = output;
                        }
                        Err(e) => {
                            eprintln!("[bot_runner] Opponent bot '{}' failed: {}", opponent_name_clone, e);
                            status.opponent_bot_error = Some(e);
                        }
                    }
                }).await;
            });
        }
    }
}

/// Run a bash command asynchronously, capture output and write to log file.
/// Replaces `{port}` in command with the proxy port, and sets `SC2_PROXY_PORT` env var.
async fn run_bot_command(command: &str, bot_name: &str, log_path: &PathBuf, proxy_port: u16) -> Result<Vec<String>, String> {
    // Replace {port} placeholder in command with actual proxy port.
    let command = command.replace("{port}", &proxy_port.to_string());
    println!("[bot_runner] Executing '{}' bot command: {} (port={})", bot_name, command, proxy_port);

    // Wait for proxies to be ready (give them 2 seconds to start listening)
    //println!("[bot_runner] Waiting 2 seconds for proxy to be ready...");
    //tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Create/truncate log file
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)
        .map_err(|e| format!("Failed to create log file {:?}: {}", log_path, e))?;
    
    let log_file = Arc::new(Mutex::new(log_file));
    
    // Write header to log
    {
        let mut file = log_file.lock().unwrap();
        let timestamp = format_timestamp();
        writeln!(file, "=== Bot '{}' started at {} ===", bot_name, timestamp).ok();
        writeln!(file, "Command: {}", command).ok();
        writeln!(file, "").ok();
    }

    let mut child = Command::new("bash")
        .arg("-c")
        .arg(&command)
        .env("SC2_PROXY_PORT", proxy_port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}' bot process: {}", bot_name, e))?;

    println!("[bot_runner] '{}' process spawned with PID: {:?}", bot_name, child.id());

    let stdout = child.stdout.take()
        .ok_or_else(|| format!("Failed to capture stdout for '{}' bot", bot_name))?;
    let stderr = child.stderr.take()
        .ok_or_else(|| format!("Failed to capture stderr for '{}' bot", bot_name))?;

    let mut output_lines = Vec::new();

    // Spawn task to read stdout
    let bot_name_stdout = bot_name.to_string();
    let log_file_stdout = Arc::clone(&log_file);
    let stdout_handle = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut collected = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            println!("[{}:stdout] {}", bot_name_stdout, line);
            // Write to log file
            if let Ok(mut file) = log_file_stdout.lock() {
                writeln!(file, "[stdout] {}", line).ok();
            }
            collected.push(line);
        }
        collected
    });

    // Spawn task to read stderr
    let bot_name_stderr = bot_name.to_string();
    let log_file_stderr = Arc::clone(&log_file);
    let stderr_handle = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut collected = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[{}:stderr] {}", bot_name_stderr, line);
            // Write to log file
            if let Ok(mut file) = log_file_stderr.lock() {
                writeln!(file, "[stderr] {}", line).ok();
            }
            collected.push(line);
        }
        collected
    });

    // Wait for process to complete
    let status = child.wait().await
        .map_err(|e| format!("Failed to wait for '{}' bot process: {}", bot_name, e))?;

    // Collect output
    let stdout_lines = stdout_handle.await
        .map_err(|e| format!("Failed to join stdout task: {}", e))?;
    let stderr_lines = stderr_handle.await
        .map_err(|e| format!("Failed to join stderr task: {}", e))?;

    output_lines.extend(stdout_lines);
    output_lines.extend(stderr_lines);

    // Write footer to log
    {
        let mut file = log_file.lock().unwrap();
        let timestamp = format_timestamp();
        writeln!(file, "").ok();
        writeln!(file, "=== Bot '{}' finished at {} with status: {:?} ===", bot_name, timestamp, status).ok();
    }

    if !status.success() {
        return Err(format!("'{}' bot process exited with status: {:?}", bot_name, status));
    }
    
    println!("[bot_runner] '{}' bot process completed with status: {:?}", bot_name, status);
    Ok(output_lines)
}

