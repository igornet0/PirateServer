//! Interactive host shell over WebSocket (JWT). Linux/macOS only; gated by `CONTROL_API_HOST_TERMINAL`.

use crate::error::ApiError;
use crate::{check_api_bearer_with_query, ApiState, StreamAuthQuery};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::Receiver as StdReceiver;
use std::sync::mpsc::Sender as StdSender;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

/// GET `/api/v1/host-terminal/ws?access_token=…`
pub async fn api_host_terminal_ws(
    ws: WebSocketUpgrade,
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<StreamAuthQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_api_bearer_with_query(&s, &headers, q.access_token.as_deref())?;
    if !s.host_terminal_enabled {
        return Err(ApiError::service_unavailable(
            "CONTROL_API_HOST_TERMINAL is not enabled",
        ));
    }
    #[cfg(unix)]
    {
        if !s.host_terminal_shell.is_absolute() {
            return Err(ApiError::bad_request(
                "CONTROL_API_HOST_TERMINAL_SHELL must be an absolute path",
            ));
        }
        let shell = s.host_terminal_shell.clone();
        let session = Duration::from_secs(s.host_terminal_session_secs);
        Ok(ws.on_upgrade(move |socket| host_terminal_ws_task(socket, shell, session)))
    }
    #[cfg(not(unix))]
    {
        Err(ApiError::service_unavailable(
            "host terminal is only supported on Unix",
        ))
    }
}

#[cfg(unix)]
async fn host_terminal_ws_task(
    mut socket: WebSocket,
    shell: std::path::PathBuf,
    session: Duration,
) {
    let session_id = Uuid::new_v4();
    let t0 = std::time::Instant::now();
    tracing::info!(%session_id, "host terminal session start");

    let (tx_to_pty, rx_to_pty): (StdSender<Vec<u8>>, StdReceiver<Vec<u8>>) =
        std::sync::mpsc::channel();
    let (tx_from_pty, mut rx_from_pty) = mpsc::unbounded_channel::<Vec<u8>>();

    let shell_thread = shell.clone();
    let pump = std::thread::spawn(move || {
        if let Err(e) = run_pty_bridge(session_id, shell_thread, rx_to_pty, tx_from_pty) {
            tracing::warn!(%session_id, error = %e, "host terminal PTY bridge ended");
        }
    });

    let deadline = tokio::time::Instant::now() + session;
    let mut sleep = Box::pin(tokio::time::sleep_until(deadline));
    loop {
        tokio::select! {
            _ = &mut sleep => {
                tracing::info!(%session_id, "host terminal session timeout");
                break;
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if tx_to_pty.send(b.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(t))) => {
                        if tx_to_pty.send(t.into_bytes()).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = socket.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                }
            }
            chunk = rx_from_pty.recv() => {
                match chunk {
                    Some(b) => {
                        if socket.send(Message::Binary(b.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    drop(tx_to_pty);
    let close_start = std::time::Instant::now();
    let _ = socket.close().await;

    // Never block a Tokio worker on `JoinHandle::join` — bounded wait on blocking pool.
    const PUMP_JOIN_TIMEOUT: Duration = Duration::from_secs(4);
    match tokio::time::timeout(
        PUMP_JOIN_TIMEOUT,
        tokio::task::spawn_blocking(move || pump.join()),
    )
    .await
    {
        Ok(Ok(Ok(()))) => {
            tracing::info!(
                %session_id,
                close_ms = %close_start.elapsed().as_millis(),
                total_ms = %t0.elapsed().as_millis(),
                "host terminal session cleanup finished"
            );
        }
        Ok(Ok(Err(e))) => {
            tracing::warn!(
                %session_id,
                error = ?e,
                "host terminal pump thread panicked"
            );
        }
        Ok(Err(e)) => {
            tracing::warn!(%session_id, error = %e, "host terminal spawn_blocking join failed");
        }
        Err(_) => {
            tracing::warn!(
                %session_id,
                timeout_sec = %PUMP_JOIN_TIMEOUT.as_secs(),
                "host terminal pump join timed out — PTY thread may still be unwinding; API workers are not blocked"
            );
        }
    }
}

#[cfg(unix)]
fn kill_process_group(session_id: Uuid, pid: u32, phase: &str) {
    use libc::{kill, SIGKILL, SIGTERM};
    let pg = -(pid as i32);
    unsafe {
        let r = kill(pg, SIGTERM);
        if r != 0 {
            tracing::debug!(
                %session_id,
                phase,
                pid,
                err = ?std::io::Error::last_os_error(),
                "killpg SIGTERM"
            );
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    unsafe {
        let r = kill(pg, SIGKILL);
        if r != 0 {
            tracing::debug!(
                %session_id,
                phase,
                pid,
                err = ?std::io::Error::last_os_error(),
                "killpg SIGKILL"
            );
        }
    }
}

#[cfg(unix)]
fn wait_child_bounded(
    session_id: Uuid,
    child: &mut dyn portable_pty::Child,
    label: &str,
) {
    for i in 0..40 {
        match portable_pty::Child::try_wait(child) {
            Ok(Some(st)) => {
                tracing::debug!(%session_id, label, attempt = i, status = %st, "child exited");
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                tracing::debug!(%session_id, label, error = %e, "try_wait error");
                return;
            }
        }
    }
    tracing::warn!(%session_id, label, "child wait bounded timeout; continuing teardown");
}

#[cfg(unix)]
fn join_read_thread_bounded(session_id: Uuid, read_thread: std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let joiner = std::thread::spawn(move || {
        let _ = read_thread.join();
        let _ = tx.send(());
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(()) => {
            let _ = joiner.join();
        }
        Err(_) => {
            tracing::warn!(
                %session_id,
                "host terminal read_thread join timed out; detaching joiner"
            );
            drop(joiner);
        }
    }
}

#[cfg(unix)]
fn run_pty_bridge(
    session_id: Uuid,
    shell: std::path::PathBuf,
    rx: StdReceiver<Vec<u8>>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
) -> Result<(), String> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    let shell_path = shell
        .to_str()
        .ok_or_else(|| "invalid shell path UTF-8".to_string())?;
    if !Path::new(shell_path).is_absolute() {
        return Err("shell must be absolute".into());
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let mut cmd = CommandBuilder::new(shell_path);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell: {e}"))?;

    let pid = child
        .process_id()
        .ok_or_else(|| "spawned child has no process id".to_string())?;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("pty reader: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("pty writer: {e}"))?;

    let tx_out = tx.clone();
    let read_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx_out.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(%session_id, error = %e, "pty read");
                    break;
                }
            }
        }
    });

    const RX_POLL: Duration = Duration::from_millis(200);
    loop {
        match rx.recv_timeout(RX_POLL) {
            Ok(data) => {
                if let Err(e) = writer.write_all(&data) {
                    tracing::debug!(%session_id, error = %e, "pty write");
                    break;
                }
                let _ = writer.flush();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    tracing::debug!(%session_id, pid, "host terminal teardown");
    // EOF to slave; then kill the whole session / process group (setsid child is group leader).
    drop(writer);
    std::thread::sleep(Duration::from_millis(50));
    kill_process_group(session_id, pid, "teardown");
    wait_child_bounded(session_id, &mut *child, "after_killpg");
    join_read_thread_bounded(session_id, read_thread);
    wait_child_bounded(session_id, &mut *child, "final_reap");
    tracing::debug!(%session_id, "host terminal PTY bridge complete");
    Ok(())
}
