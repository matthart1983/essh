use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use russh::client::{self, Handle, Msg};
use russh::keys::agent::client::AgentClient;
use russh::keys::ssh_key;
use russh::keys::PrivateKeyWithHashAlg;
use russh::{Channel, Disconnect};
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("Connection error: {0}")]
    #[allow(dead_code)]
    Connection(String),
    #[error("Authentication error: {0}")]
    Auth(String),
    #[error("Host key error: {0}")]
    HostKey(String),
    #[error("Channel error: {0}")]
    #[allow(dead_code)]
    Channel(String),
    #[error(transparent)]
    Russh(#[from] russh::Error),
    #[error("Key error: {0}")]
    Key(russh::keys::Error),
}

impl From<russh::keys::Error> for SshError {
    fn from(e: russh::keys::Error) -> Self {
        SshError::Key(e)
    }
}

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
}

impl ConnectConfig {
    #[allow(dead_code)]
    pub fn new(hostname: String, username: String, auth: AuthMethod) -> Self {
        Self {
            hostname,
            port: 22,
            username,
            auth,
        }
    }
}

#[derive(Clone, Debug)]
pub enum AuthMethod {
    Password(String),
    KeyFile(PathBuf),
    Agent,
}

pub struct ClientHandler {
    host_key_fingerprint: Arc<Mutex<Option<String>>>,
    server_banner: Arc<Mutex<Option<String>>>,
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let fingerprint = server_public_key
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string();
        let fp_store = self.host_key_fingerprint.clone();
        async move {
            *fp_store.lock().await = Some(fingerprint);
            Ok(true)
        }
    }

    fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let banner = banner.to_string();
        let banner_store = self.server_banner.clone();
        async move {
            *banner_store.lock().await = Some(banner);
            Ok(())
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub struct SshSession {
    pub handle: Handle<ClientHandler>,
    #[allow(dead_code)]
    session_id: String,
    pub jump_host: Option<String>,
}

pub struct SshClient;

impl SshClient {
    pub async fn connect(
        config: &ConnectConfig,
    ) -> Result<(SshSession, String, Option<String>), SshError> {
        let ssh_config = Arc::new(client::Config::default());

        let host_key_fingerprint = Arc::new(Mutex::new(None));
        let server_banner = Arc::new(Mutex::new(None));

        let handler = ClientHandler {
            host_key_fingerprint: host_key_fingerprint.clone(),
            server_banner: server_banner.clone(),
        };

        let mut handle =
            client::connect(ssh_config, (config.hostname.as_str(), config.port), handler).await?;

        let authenticated = match &config.auth {
            AuthMethod::Password(pw) => {
                let result = handle
                    .authenticate_password(&config.username, pw)
                    .await
                    .map_err(SshError::Russh)?;
                result.success()
            }
            AuthMethod::KeyFile(path) => {
                let key = russh::keys::load_secret_key(path, None)?;
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), None);
                let result = handle
                    .authenticate_publickey(&config.username, key)
                    .await
                    .map_err(SshError::Russh)?;
                result.success()
            }
            AuthMethod::Agent => {
                Self::authenticate_with_agent(&mut handle, &config.username).await?
            }
        };

        if !authenticated {
            return Err(SshError::Auth("Authentication failed".into()));
        }

        let fingerprint = host_key_fingerprint
            .lock()
            .await
            .take()
            .ok_or_else(|| SshError::HostKey("No host key fingerprint captured".into()))?;

        let banner = server_banner.lock().await.take();

        let session_id = uuid::Uuid::new_v4().to_string();

        Ok((
            SshSession {
                handle,
                session_id,
                jump_host: None,
            },
            fingerprint,
            banner,
        ))
    }
}

impl SshClient {
    /// Try each key from the local ssh-agent until one succeeds.
    async fn authenticate_with_agent(
        handle: &mut Handle<ClientHandler>,
        username: &str,
    ) -> Result<bool, SshError> {
        let mut agent = AgentClient::connect_env()
            .await
            .map_err(|e| SshError::Auth(format!("Could not connect to ssh-agent: {}", e)))?;

        let identities = agent
            .request_identities()
            .await
            .map_err(|e| SshError::Auth(format!("Failed to list agent keys: {}", e)))?;

        if identities.is_empty() {
            return Err(SshError::Auth("No keys found in ssh-agent".into()));
        }

        for key in &identities {
            let result = handle
                .authenticate_publickey_with(username, key.clone(), None, &mut agent)
                .await;
            match result {
                Ok(r) if r.success() => return Ok(true),
                _ => continue,
            }
        }

        Ok(false)
    }
}

impl SshClient {
    /// Connect to a target host through a jump host (ProxyJump).
    /// Opens a direct-tcpip channel on the jump host to forward to the target.
    pub async fn connect_via_jump(
        jump_config: &ConnectConfig,
        target_config: &ConnectConfig,
    ) -> Result<(SshSession, String, Option<String>), SshError> {
        // Connect to the jump host first
        let (jump_session, _jump_fp, _jump_banner) = Self::connect(jump_config).await?;

        // Open a direct-tcpip channel through the jump host to the target
        let forwarded_channel = jump_session
            .handle
            .channel_open_direct_tcpip(
                &target_config.hostname,
                target_config.port as u32,
                "127.0.0.1",
                0,
            )
            .await
            .map_err(SshError::Russh)?;

        // Build a tokio stream from the forwarded channel for the target SSH handshake
        let (reader_tx, reader_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
        let (writer_tx, mut writer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

        // Spawn a task to bridge the forwarded channel ↔ mpsc channels
        let mut fwd_channel = forwarded_channel;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(msg) = fwd_channel.wait() => {
                        match msg {
                            russh::ChannelMsg::Data { data } => {
                                if reader_tx.send(data.to_vec()).await.is_err() {
                                    break;
                                }
                            }
                            russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                            _ => {}
                        }
                    }
                    Some(data) = writer_rx.recv() => {
                        if fwd_channel.data(&data[..]).await.is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
        });

        // Use the channel as a transport for a new SSH connection to the target
        let stream = ChannelStream::new(reader_rx, writer_tx);

        let host_key_fingerprint = Arc::new(Mutex::new(None));
        let server_banner = Arc::new(Mutex::new(None));

        let handler = ClientHandler {
            host_key_fingerprint: host_key_fingerprint.clone(),
            server_banner: server_banner.clone(),
        };

        let ssh_config = Arc::new(client::Config::default());
        let mut handle = client::connect_stream(ssh_config, stream, handler).await?;

        let authenticated = match &target_config.auth {
            AuthMethod::Password(pw) => handle
                .authenticate_password(&target_config.username, pw)
                .await
                .map_err(SshError::Russh)?
                .success(),
            AuthMethod::KeyFile(path) => {
                let key = russh::keys::load_secret_key(path, None)?;
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), None);
                handle
                    .authenticate_publickey(&target_config.username, key)
                    .await
                    .map_err(SshError::Russh)?
                    .success()
            }
            AuthMethod::Agent => {
                Self::authenticate_with_agent(&mut handle, &target_config.username).await?
            }
        };

        if !authenticated {
            return Err(SshError::Auth(
                "Authentication failed on target host".into(),
            ));
        }

        let fingerprint = host_key_fingerprint
            .lock()
            .await
            .take()
            .ok_or_else(|| SshError::HostKey("No host key fingerprint captured".into()))?;

        let banner = server_banner.lock().await.take();
        let session_id = uuid::Uuid::new_v4().to_string();

        Ok((
            SshSession {
                handle,
                session_id,
                jump_host: Some(jump_config.hostname.clone()),
            },
            fingerprint,
            banner,
        ))
    }
}

/// A tokio AsyncRead + AsyncWrite wrapper around mpsc channels,
/// used to bridge a forwarded SSH channel as a stream for russh's connect_stream.
struct ChannelStream {
    reader: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    writer: tokio::sync::Mutex<tokio::sync::mpsc::Sender<Vec<u8>>>,
    read_buf: std::sync::Mutex<Vec<u8>>,
}

impl ChannelStream {
    fn new(
        reader: tokio::sync::mpsc::Receiver<Vec<u8>>,
        writer: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> Self {
        Self {
            reader: tokio::sync::Mutex::new(reader),
            writer: tokio::sync::Mutex::new(writer),
            read_buf: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl tokio::io::AsyncRead for ChannelStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;

        let this = self.get_mut();

        // Try to drain from internal buffer first
        if let Ok(mut read_buf) = this.read_buf.lock() {
            if !read_buf.is_empty() {
                let n = read_buf.len().min(buf.remaining());
                buf.put_slice(&read_buf[..n]);
                read_buf.drain(..n);
                return Poll::Ready(Ok(()));
            }
        }

        // Try to receive from channel
        let mut reader = match this.reader.try_lock() {
            Ok(r) => r,
            Err(_) => return Poll::Pending,
        };

        match reader.poll_recv(cx) {
            Poll::Ready(Some(data)) => {
                let n = data.len().min(buf.remaining());
                buf.put_slice(&data[..n]);
                if n < data.len() {
                    if let Ok(mut read_buf) = this.read_buf.lock() {
                        read_buf.extend_from_slice(&data[n..]);
                    }
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl tokio::io::AsyncWrite for ChannelStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let writer = match this.writer.try_lock() {
            Ok(w) => w,
            Err(_) => return std::task::Poll::Pending,
        };
        let data = buf.to_vec();
        let len = data.len();
        match writer.try_send(data) {
            Ok(()) => std::task::Poll::Ready(Ok(len)),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => std::task::Poll::Pending,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => std::task::Poll::Ready(Err(
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "channel closed"),
            )),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl SshSession {
    pub async fn open_shell(
        &mut self,
        term: &str,
        cols: u32,
        rows: u32,
    ) -> Result<Channel<Msg>, SshError> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;

        channel
            .request_pty(false, term, cols, rows, 0, 0, &[])
            .await
            .map_err(SshError::Russh)?;

        channel
            .request_shell(false)
            .await
            .map_err(SshError::Russh)?;

        Ok(channel)
    }

    pub async fn close(&self) -> Result<(), SshError> {
        self.handle
            .disconnect(Disconnect::ByApplication, "goodbye", "en")
            .await
            .map_err(SshError::Russh)?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_config_new() {
        let config = ConnectConfig::new(
            "example.com".to_string(),
            "user".to_string(),
            AuthMethod::Agent,
        );
        assert_eq!(config.hostname, "example.com");
        assert_eq!(config.port, 22);
        assert_eq!(config.username, "user");
    }

    #[test]
    fn test_connect_config_clone() {
        let config = ConnectConfig {
            hostname: "host.example.com".to_string(),
            port: 2222,
            username: "deploy".to_string(),
            auth: AuthMethod::Agent,
        };
        let cloned = config.clone();
        assert_eq!(cloned.hostname, config.hostname);
        assert_eq!(cloned.port, config.port);
        assert_eq!(cloned.username, config.username);
    }

    #[test]
    fn test_auth_method_variants() {
        let pw = AuthMethod::Password("secret".to_string());
        assert!(matches!(pw, AuthMethod::Password(_)));

        let key = AuthMethod::KeyFile("/home/user/.ssh/id_ed25519".into());
        assert!(matches!(key, AuthMethod::KeyFile(_)));

        let agent = AuthMethod::Agent;
        assert!(matches!(agent, AuthMethod::Agent));
    }

    #[test]
    fn test_channel_stream_creation() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (writer_tx, _writer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let stream = ChannelStream::new(rx, writer_tx);
        // Verify the stream was created successfully
        assert!(stream.read_buf.lock().unwrap().is_empty());
        drop(tx);
    }

    #[test]
    fn test_ssh_error_display() {
        let err = SshError::Connection("timeout".to_string());
        assert_eq!(err.to_string(), "Connection error: timeout");

        let err = SshError::Auth("bad key".to_string());
        assert_eq!(err.to_string(), "Authentication error: bad key");

        let err = SshError::HostKey("changed".to_string());
        assert_eq!(err.to_string(), "Host key error: changed");

        let err = SshError::Channel("closed".to_string());
        assert_eq!(err.to_string(), "Channel error: closed");
    }
}
