use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use tempfile::TempDir;

struct Harness {
    _temp: TempDir,
    home: PathBuf,
    bin: PathBuf,
}

struct CommandResult {
    stdout: String,
    stderr: String,
}

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

        assert!(
            output.status.success(),
            "command failed\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );

        CommandResult { stdout, stderr }
    }
}

impl CommandResult {
    fn stdout_contains(&self, needle: &str) {
        assert!(
            self.stdout.contains(needle),
            "expected stdout to contain {:?}\nstdout:\n{}\nstderr:\n{}",
            needle,
            self.stdout,
            self.stderr
        );
    }
}

fn generate_test_key(path: &Path) {
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).expect("generate test key");
    key.write_openssh_file(path, LineEnding::LF)
        .expect("write test key");
}

#[test]
fn config_commands_round_trip_in_temp_home() {
    let harness = Harness::new();

    let init = harness.run(["config", "init"]);
    init.stdout_contains("Default config written to");
    assert!(harness.essh_dir().join("config.toml").exists());

    let show = harness.run(["config", "show"]);
    show.stdout_contains("[general]");
    show.stdout_contains("tofu_policy = \"prompt\"");
    show.stdout_contains("[session]");
}

#[test]
fn hosts_cache_and_import_workflows_are_end_to_end() {
    let harness = Harness::new();

    let add = harness.run([
        "hosts",
        "add",
        "db.internal",
        "--port",
        "2222",
        "--tag",
        "env=prod",
        "--tag",
        "role=db",
    ]);
    add.stdout_contains("Host db.internal added to cache.");

    let list = harness.run(["hosts", "list"]);
    list.stdout_contains("db.internal");
    list.stdout_contains("2222");
    list.stdout_contains("env=prod");
    list.stdout_contains("role=db");

    let filtered = harness.run(["hosts", "list", "--tag", "env=prod"]);
    filtered.stdout_contains("db.internal");

    let removed = harness.run(["hosts", "remove", "db.internal", "--port", "2222"]);
    removed.stdout_contains("Host db.internal removed.");

    let empty = harness.run(["hosts", "list"]);
    empty.stdout_contains("No cached hosts.");

    let ssh_config = harness.write_file(
        harness.home.join("import-test.conf"),
        r#"
Host web-prod
  HostName 10.0.0.10
  User deploy

Host db-prod
  HostName 10.0.0.20
  Port 2202
  User postgres
"#,
    );

    let import = harness.run(["hosts", "import", ssh_config.to_str().expect("path utf8")]);
    import.stdout_contains("Imported 2 hosts");
    import.stdout_contains("web-prod -> 10.0.0.10:22");
    import.stdout_contains("db-prod -> 10.0.0.20:2202");

    let imported_hosts = harness.run(["hosts", "list"]);
    imported_hosts.stdout_contains("10.0.0.10");
    imported_hosts.stdout_contains("10.0.0.20");
    imported_hosts.stdout_contains("2202");
}

#[test]
fn key_cache_workflow_uses_a_real_generated_private_key() {
    let harness = Harness::new();
    let key_path = harness.home.join("id_ed25519_test");
    generate_test_key(&key_path);

    let add = harness.run([
        "keys",
        "add",
        key_path.to_str().expect("path utf8"),
        "--name",
        "ci-ed25519",
    ]);
    add.stdout_contains("Key 'ci-ed25519' added.");

    let list = harness.run(["keys", "list"]);
    list.stdout_contains("ci-ed25519");
    list.stdout_contains(key_path.to_str().expect("path utf8"));
    list.stdout_contains("Ed25519");

    let remove = harness.run(["keys", "remove", "ci-ed25519"]);
    remove.stdout_contains("Key 'ci-ed25519' removed.");

    let empty = harness.run(["keys", "list"]);
    empty.stdout_contains("No cached keys.");
}

#[test]
fn diagnostics_audit_and_session_artifacts_are_reported_end_to_end() {
    let harness = Harness::new();
    let essh_dir = harness.essh_dir();

    harness.write_file(
        essh_dir.join("sessions").join("demo-session.jsonl"),
        format!(
            "{}\n{}\n",
            json!({
                "timestamp": "2026-04-03T10:00:00Z",
                "session_id": "demo-session",
                "rtt_ms": 12.5,
                "bytes_sent": 120,
                "bytes_received": 240,
                "throughput_up_bps": 12.0,
                "throughput_down_bps": 24.0,
                "packet_loss_pct": 0.0,
                "quality": "Excellent",
                "uptime_secs": 10,
                "channels_active": 1,
            }),
            json!({
                "timestamp": "2026-04-03T10:01:00Z",
                "session_id": "demo-session",
                "rtt_ms": 18.0,
                "bytes_sent": 512,
                "bytes_received": 2048,
                "throughput_up_bps": 32.0,
                "throughput_down_bps": 128.0,
                "packet_loss_pct": 1.2,
                "quality": "Good",
                "uptime_secs": 70,
                "channels_active": 1,
            })
        ),
    );

    harness.write_file(
        essh_dir.join("recordings").join("demo-recording.cast"),
        format!(
            "{}\n{}\n",
            json!({
                "version": 2,
                "width": 80,
                "height": 24,
                "timestamp": 1_775_214_400_i64,
                "title": "demo recording",
            }),
            json!([0.0, "o", "hello from essh\r\n"])
        ),
    );

    harness.write_file(
        essh_dir.join("audit.log"),
        format!(
            "{}\n{}\n",
            json!({
                "timestamp": "2026-04-03T10:00:00Z",
                "event_type": "connection_attempt",
                "session_id": "demo-session",
                "hostname": "demo.internal",
                "port": 22,
                "username": "deploy",
                "details": {},
            }),
            json!({
                "timestamp": "2026-04-03T10:01:00Z",
                "event_type": "session_start",
                "session_id": "demo-session",
                "hostname": "demo.internal",
                "port": 22,
                "username": "deploy",
                "details": {},
            })
        ),
    );

    let diag = harness.run(["diag", "demo-session"]);
    diag.stdout_contains("Session: demo-session");
    diag.stdout_contains("RTT: Some(18.0) ms");
    diag.stdout_contains("Quality: Good");

    let session_list = harness.run(["session", "list"]);
    session_list.stdout_contains("demo-recording");
    session_list.stdout_contains("1 recording(s) found.");

    let audit = harness.run(["audit", "tail", "--lines", "1"]);
    audit.stdout_contains("SessionStart");
    audit.stdout_contains("demo.internal");
    audit.stdout_contains("demo-session");

    let missing_replay = harness.run(["session", "replay", "missing-session"]);
    missing_replay.stdout_contains("Session missing-session not found.");
    missing_replay.stdout_contains("missing-session.cast");
    missing_replay.stdout_contains("missing-session.jsonl");
}
