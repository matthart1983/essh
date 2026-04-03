#![cfg(unix)]

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::rand_core::OsRng;
use russh::keys::Algorithm;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{server, Channel, ChannelId, CryptoVec};
use ssh_key::{LineEnding, PrivateKey};
use tempfile::TempDir;
use tokio::net::TcpListener;

const PTY_CONNECT_TIMEOUT_SECS: u64 = 15;
const SNAPSHOT_PREVIEW_LIMIT: usize = 8 * 1024;

struct Harness {
    _temp: TempDir,
    home: PathBuf,
    bin: PathBuf,
}

struct CommandResult {
    stdout: String,
    stderr: String,
    diagnostics: String,
}

struct MockServerRuntime {
    port: u16,
    handle: server::RunningServerHandle,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

#[derive(Clone, Default)]
struct MockSshServer;

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new().expect("create temp dir");
        let home = temp.path().join("home");
        fs::create_dir_all(&home).expect("create temp home");

        Self {
            _temp: temp,
            home,
            bin: PathBuf::from(env!("CARGO_BIN_EXE_essh")),
        }
    }

    fn essh_dir(&self) -> PathBuf {
        self.home.join(".essh")
    }

    fn write_file(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, contents).expect("write file");
        path.to_path_buf()
    }

    fn run<I, S>(&self, args: I) -> CommandResult
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(&self.bin)
            .args(args)
            .env("HOME", &self.home)
            .env("TERM", "dumb")
            .env_remove("SSH_AUTH_SOCK")
            .output()
            .expect("run essh binary");

        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        let diagnostics = self.diagnostics_snapshot();

        assert!(
            output.status.success(),
            "command failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}\ndiagnostics:\n{}",
            format_exit_status(&output.status),
            stdout,
            stderr,
            diagnostics
        );

        CommandResult {
            stdout,
            stderr,
            diagnostics,
        }
    }

    fn run_connect_under_pty(&self, script_body: &str) -> CommandResult {
        let script_path = self.write_file(self.home.join("run-connect.sh"), script_body);
        let pty_runner = r#"
import os
import pty
import select
import subprocess
import sys
import time

script_path = sys.argv[1]
timeout_secs = float(sys.argv[2])
deadline = time.monotonic() + timeout_secs
master, slave = pty.openpty()
proc = subprocess.Popen(
    ["sh", script_path],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    close_fds=True,
)
os.close(slave)

chunks = []
timed_out = False

def drain_master():
    while True:
        try:
            data = os.read(master, 4096)
        except OSError:
            data = b""
        if not data:
            break
        chunks.append(data)

while True:
    ready, _, _ = select.select([master], [], [], 0.1)
    if master in ready:
        try:
            data = os.read(master, 4096)
        except OSError:
            data = b""
        if not data:
            break
        chunks.append(data)
        continue

    if time.monotonic() >= deadline:
        timed_out = True
        proc.kill()
        break

    if proc.poll() is not None:
        drain_master()
        break

drain_master()
os.close(master)
sys.stdout.buffer.write(b"".join(chunks))
sys.stdout.flush()
status = proc.wait()

if timed_out:
    sys.stderr.write(
        f"pty wrapper timed out after {timeout_secs:.1f}s while running {script_path}\n"
    )
    if status == 0:
        status = 124

if status != 0:
    sys.stderr.write(f"pty wrapper child exit status: {status}\n")

sys.exit(status)
"#;

        let output = Command::new("python3")
            .arg("-c")
            .arg(pty_runner)
            .arg(&script_path)
            .arg(PTY_CONNECT_TIMEOUT_SECS.to_string())
            .output()
            .expect("run python pty wrapper");

        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        let diagnostics = self.diagnostics_snapshot();

        assert!(
            output.status.success(),
            "pty command failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}\ndiagnostics:\n{}",
            format_exit_status(&output.status),
            stdout,
            stderr,
            diagnostics
        );

        CommandResult {
            stdout,
            stderr,
            diagnostics,
        }
    }

    fn diagnostics_snapshot(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "home: {}", self.home.display());
        let _ = writeln!(out, "bin: {}", self.bin.display());
        self.append_tree_snapshot(&mut out, &self.home, &self.home);
        out
    }

    fn append_tree_snapshot(&self, out: &mut String, root: &Path, dir: &Path) {
        let mut entries = match fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
            Err(err) => {
                let rel = dir.strip_prefix(root).unwrap_or(dir);
                let label = if rel.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    rel.display().to_string()
                };
                let _ = writeln!(out, "[error] {}: {}", label, err);
                return;
            }
        };

        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path);

            if path.is_dir() {
                let _ = writeln!(out, "[dir] {}", rel.display());
                self.append_tree_snapshot(out, root, &path);
                continue;
            }

            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            let _ = writeln!(out, "[file] {} ({} bytes)", rel.display(), size);

            if should_preview_file(&path) {
                append_file_preview(out, rel, &path);
            }
        }
    }
}

impl CommandResult {
    fn stdout_contains(&self, needle: &str) {
        assert!(
            self.stdout.contains(needle),
            "expected stdout to contain {:?}\nstdout:\n{}\nstderr:\n{}\ndiagnostics:\n{}",
            needle,
            self.stdout,
            self.stderr,
            self.diagnostics
        );
    }
}

fn format_exit_status(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return code.to_string();
    }

    match status.signal() {
        Some(signal) => format!("signal {}", signal),
        None => "unknown".to_owned(),
    }
}

fn should_preview_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("config.toml") | Some("audit.log") | Some("run-connect.sh")
    ) || matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("jsonl") | Some("cast")
    )
}

fn append_file_preview(out: &mut String, display_path: &Path, path: &Path) {
    match fs::read(path) {
        Ok(contents) => {
            let preview_len = contents.len().min(SNAPSHOT_PREVIEW_LIMIT);
            let preview = String::from_utf8_lossy(&contents[..preview_len]);
            let _ = writeln!(out, "----- {} -----", display_path.display());
            out.push_str(&preview);
            if !preview.ends_with('\n') {
                out.push('\n');
            }
            if contents.len() > preview_len {
                let _ = writeln!(
                    out,
                    "[truncated] showed {} of {} bytes",
                    preview_len,
                    contents.len()
                );
            }
        }
        Err(err) => {
            let _ = writeln!(out, "[error reading {}] {}", display_path.display(), err);
        }
    }
}

impl MockServerRuntime {
    async fn shutdown(self) {
        self.handle.shutdown("test complete".to_string());
        self.task
            .await
            .expect("await server task")
            .expect("server exit cleanly");
    }
}

impl server::Server for MockSshServer {
    type Handler = Self;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self
    }
}

impl server::Handler for MockSshServer {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<Msg>,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        _: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.request_success();
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.request_success();
        session.data(
            channel,
            CryptoVec::from_slice(b"mock-shell ready\r\nType exit to close\r\n"),
        )?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let text = String::from_utf8_lossy(data);
        session.data(
            channel,
            CryptoVec::from(format!("mock-echo: {}", text).into_bytes()),
        )?;

        if text.contains("exit") {
            session.data(channel, CryptoVec::from_slice(b"bye\r\n"))?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        }

        Ok(())
    }
}

async fn spawn_mock_ssh_server() -> MockServerRuntime {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind mock ssh server");
    let port = listener.local_addr().expect("mock ssh local addr").port();

    let config = Arc::new(russh::server::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        auth_rejection_time: Duration::from_millis(50),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        keys: vec![
            russh::keys::PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
                .expect("generate host key"),
        ],
        ..Default::default()
    });

    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut server = MockSshServer;
        let running = server.run_on_socket(config, &listener);
        let handle = running.handle();
        let _ = handle_tx.send(handle);
        running.await
    });

    let handle = handle_rx.await.expect("receive running server handle");
    tokio::time::sleep(Duration::from_millis(100)).await;

    MockServerRuntime { port, handle, task }
}

fn generate_client_key(path: &Path) {
    let key = PrivateKey::random(&mut ssh_key::rand_core::OsRng, ssh_key::Algorithm::Ed25519)
        .expect("generate client key");
    key.write_openssh_file(path, LineEnding::LF)
        .expect("write client key");
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn extract_session_id(output: &str) -> String {
    output
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Connected. Session ID: ")
                .map(ToOwned::to_owned)
        })
        .expect("extract session id from connect output")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_ssh_server_drives_real_connect_flow_end_to_end() {
    let harness = Harness::new();
    let server = spawn_mock_ssh_server().await;

    harness.write_file(
        harness.essh_dir().join("config.toml"),
        b"[general]\ntofu_policy = \"auto\"\n",
    );

    let key_path = harness.home.join("id_ed25519_mock");
    generate_client_key(&key_path);

    let connect_script = format!(
        "#!/bin/sh\nset -eu\nprintf 'status\\nexit\\n' | HOME={} TERM=xterm-256color {} connect tester@127.0.0.1 --port {} -i {}\n",
        shell_escape(harness.home.to_str().expect("home path utf8")),
        shell_escape(harness.bin.to_str().expect("bin path utf8")),
        server.port,
        shell_escape(key_path.to_str().expect("key path utf8")),
    );

    let connect = harness.run_connect_under_pty(&connect_script);
    connect.stdout_contains("Connecting to tester@127.0.0.1");
    connect.stdout_contains("mock-shell ready");
    connect.stdout_contains("mock-echo: status");
    connect.stdout_contains("bye");
    connect.stdout_contains("Session ended.");

    let session_id = extract_session_id(&connect.stdout);

    let hosts = harness.run(["hosts", "list"]);
    hosts.stdout_contains("127.0.0.1");
    hosts.stdout_contains(&server.port.to_string());

    let diag = harness.run(["diag", &session_id]);
    diag.stdout_contains(&format!("Session: {}", session_id));
    diag.stdout_contains("Bytes sent:");
    diag.stdout_contains("Bytes received:");

    let audit = harness.run(["audit", "tail", "--lines", "10"]);
    audit.stdout_contains("ConnectionAttempt");
    audit.stdout_contains("AuthSuccess");
    audit.stdout_contains("SessionStart");
    audit.stdout_contains("SessionEnd");
    audit.stdout_contains("127.0.0.1");

    server.shutdown().await;
}
