use std::path::PathBuf;

use anyhow::{Context, Result};

#[cfg(unix)]
const ENDPOINT_FILENAME: &str = "ui-sync.sock";
#[cfg(windows)]
const ENDPOINT_FILENAME: &str = "ui-sync.port";
#[cfg(all(not(unix), not(windows)))]
const ENDPOINT_FILENAME: &str = "ui-sync.sock";

/// Unix: domain socket path. Windows: port file (`ui-sync.port` holds the TCP port).
pub fn socket_path() -> Result<PathBuf> {
    Ok(crate::data_dir::run_dir()?.join(ENDPOINT_FILENAME))
}

fn sync_response_for_line(
    line: &str,
    mut publish: impl FnMut(super::events::UiMutationEvent),
) -> &'static [u8] {
    use super::events::UiMutationEnvelope;

    match serde_json::from_str::<UiMutationEnvelope>(line) {
        Ok(envelope) if envelope.version == UiMutationEnvelope::VERSION => {
            publish(envelope.event);
            br#"{"ok":true}"#.as_slice()
        }
        Ok(_) => br#"{"ok":false,"error":"unsupported version"}"#.as_slice(),
        Err(_) if line.trim().is_empty() => br#"{"ok":false,"error":"empty request"}"#.as_slice(),
        Err(_) => br#"{"ok":false,"error":"invalid payload"}"#.as_slice(),
    }
}

#[cfg(windows)]
fn read_listen_port() -> Result<Option<u16>> {
    let path = socket_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).context("Failed to read UI sync port file")?;
    raw.trim()
        .parse::<u16>()
        .map(Some)
        .context("Invalid UI sync port")
}

#[cfg(windows)]
fn write_listen_port(port: u16) -> Result<()> {
    let path = socket_path()?;
    std::fs::write(&path, port.to_string())
        .with_context(|| format!("Failed to write UI sync port file {}", path.display()))
}

pub fn start_listener<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};

        use anyhow::Context;
        use tauri::Manager;

        use super::manager::UiSyncManager;

        let socket_path = socket_path()?;
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let listener = std::os::unix::net::UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind UI sync socket {}", socket_path.display()))?;
        listener
            .set_nonblocking(false)
            .context("Failed to configure UI sync socket")?;

        std::thread::Builder::new()
            .name("ui-sync-listener".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        continue;
                    };

                    let mut line = String::new();
                    let read_result = {
                        let mut reader = BufReader::new(&mut stream);
                        reader.read_line(&mut line)
                    };

                    let response = match read_result {
                        Ok(0) => br#"{"ok":false,"error":"empty request"}"#.as_slice(),
                        Ok(_) => sync_response_for_line(&line, |event| {
                            let manager = app.state::<UiSyncManager>();
                            manager.publish(event);
                        }),
                        Err(_) => br#"{"ok":false,"error":"read failed"}"#.as_slice(),
                    };

                    let _ = stream.write_all(response);
                    let _ = stream.write_all(b"\n");
                    let _ = stream.flush();
                }
            })
            .context("Failed to spawn UI sync socket listener")?;

        Ok(())
    }

    #[cfg(windows)]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        use anyhow::Context;
        use tauri::Manager;

        use super::manager::UiSyncManager;

        let port_path = socket_path()?;
        if port_path.exists() {
            let _ = std::fs::remove_file(&port_path);
        }

        let listener =
            TcpListener::bind(("127.0.0.1", 0)).context("Failed to bind UI sync TCP listener")?;
        let port = listener
            .local_addr()
            .context("Failed to read UI sync listener port")?
            .port();
        write_listen_port(port)?;

        std::thread::Builder::new()
            .name("ui-sync-listener".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

                    let mut line = String::new();
                    let read_result = {
                        let mut reader = BufReader::new(&mut stream);
                        reader.read_line(&mut line)
                    };

                    let response = match read_result {
                        Ok(0) => br#"{"ok":false,"error":"empty request"}"#.as_slice(),
                        Ok(_) => sync_response_for_line(&line, |event| {
                            let manager = app.state::<UiSyncManager>();
                            manager.publish(event);
                        }),
                        Err(_) => br#"{"ok":false,"error":"read failed"}"#.as_slice(),
                    };

                    let _ = stream.write_all(response);
                    let _ = stream.write_all(b"\n");
                    let _ = stream.flush();
                }
            })
            .context("Failed to spawn UI sync TCP listener")?;

        Ok(())
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = app;
        Ok(())
    }
}

fn exchange_sync_message(
    stream: &mut (impl std::io::Read + std::io::Write),
    payload: &str,
) -> Result<bool> {
    use std::io::{BufRead, BufReader};

    std::io::Write::write_all(stream, payload.as_bytes())
        .context("Failed to write UI sync payload")?;
    std::io::Write::write_all(stream, b"\n").context("Failed to terminate UI sync payload")?;
    std::io::Write::flush(stream).context("Failed to flush UI sync payload")?;

    let mut reader = BufReader::new(&mut *stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .context("Failed to read UI sync response")?;

    Ok(serde_json::from_str::<serde_json::Value>(&response)
        .ok()
        .and_then(|value| value.get("ok").and_then(|ok| ok.as_bool()))
        .unwrap_or(false))
}

pub fn notify_running_app(event: super::events::UiMutationEvent) -> Result<bool> {
    use super::events::UiMutationEnvelope;

    let payload = serde_json::to_string(&UiMutationEnvelope::new(event))
        .context("Failed to serialize UI mutation envelope")?;

    #[cfg(unix)]
    {
        let socket_path = socket_path()?;
        if !socket_path.exists() {
            return Ok(false);
        }

        let mut stream = match std::os::unix::net::UnixStream::connect(&socket_path) {
            Ok(stream) => stream,
            Err(_) => return Ok(false),
        };

        exchange_sync_message(&mut stream, &payload)
    }

    #[cfg(windows)]
    {
        use std::net::TcpStream;
        use std::time::Duration;

        let Some(port) = read_listen_port()? else {
            return Ok(false);
        };

        let mut stream = match TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(500),
        ) {
            Ok(stream) => stream,
            Err(_) => return Ok(false),
        };

        exchange_sync_message(&mut stream, &payload)
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = payload;
        Ok(false)
    }
}

pub fn is_listener_running() -> bool {
    #[cfg(unix)]
    {
        let Ok(socket_path) = socket_path() else {
            return false;
        };
        if !socket_path.exists() {
            return false;
        }

        std::os::unix::net::UnixStream::connect(socket_path).is_ok()
    }

    #[cfg(windows)]
    {
        use std::net::TcpStream;
        use std::time::Duration;

        let Ok(Some(port)) = read_listen_port() else {
            return false;
        };

        TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(200),
        )
        .is_ok()
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_dir::TEST_ENV_LOCK;
    use crate::ui_sync::events::{UiMutationEnvelope, UiMutationEvent};

    #[test]
    fn socket_path_uses_run_dir() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HELMOR_DATA_DIR", dir.path());

        let path = socket_path().unwrap();
        #[cfg(unix)]
        assert!(path.ends_with("run/ui-sync.sock"));
        #[cfg(windows)]
        assert!(path.ends_with("run/ui-sync.port"));
    }

    #[test]
    fn envelope_parser_accepts_current_version() {
        let line = serde_json::to_string(&UiMutationEnvelope::new(
            UiMutationEvent::WorkspaceListChanged,
        ))
        .unwrap();
        let envelope: UiMutationEnvelope = serde_json::from_str(&line).unwrap();
        assert_eq!(envelope.version, UiMutationEnvelope::VERSION);
    }

    #[test]
    fn envelope_parser_rejects_unsupported_version() {
        let line = r#"{"version":99,"event":{"type":"workspaceListChanged"}}"#;
        let envelope: UiMutationEnvelope = serde_json::from_str(line).unwrap();
        assert_ne!(envelope.version, UiMutationEnvelope::VERSION);
    }

    #[test]
    fn envelope_parser_rejects_garbage_json() {
        let result = serde_json::from_str::<UiMutationEnvelope>("not json");
        assert!(result.is_err());
    }

    #[test]
    fn envelope_parser_rejects_unknown_event_type() {
        let line = r#"{"version":1,"event":{"type":"madeUpEvent"}}"#;
        let result = serde_json::from_str::<UiMutationEnvelope>(line);
        assert!(result.is_err());
    }

    #[test]
    fn is_listener_running_returns_false_without_socket() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HELMOR_DATA_DIR", dir.path());
        assert!(!is_listener_running());
    }

    #[test]
    fn notify_running_app_returns_false_without_socket() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HELMOR_DATA_DIR", dir.path());
        let result = notify_running_app(UiMutationEvent::WorkspaceListChanged).unwrap();
        assert!(!result, "with no listener the call must succeed with false");
    }

    #[cfg(windows)]
    #[test]
    fn sync_response_for_line_accepts_workspace_list_changed() {
        let line = serde_json::to_string(&UiMutationEnvelope::new(
            UiMutationEvent::WorkspaceListChanged,
        ))
        .unwrap();
        let mut published = false;
        let response = sync_response_for_line(&line, |_| published = true);
        assert!(published);
        assert_eq!(response, br#"{"ok":true}"#.as_slice());
    }
}
