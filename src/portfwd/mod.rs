use std::fmt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Port forwarding types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardDirection {
    Local,
    Remote,
}

impl fmt::Display for ForwardDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForwardDirection::Local => write!(f, "L"),
            ForwardDirection::Remote => write!(f, "R"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PortForward {
    pub id: String,
    pub direction: ForwardDirection,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// PortForwardManager
// ---------------------------------------------------------------------------

pub struct PortForwardManager {
    pub forwards: Vec<PortForward>,
    pub selected: usize,
}

impl PortForwardManager {
    pub fn new() -> Self {
        Self {
            forwards: Vec::new(),
            selected: 0,
        }
    }

    pub fn add_local(
        &mut self,
        bind_host: &str,
        bind_port: u16,
        target_host: &str,
        target_port: u16,
    ) -> &PortForward {
        let fwd = PortForward {
            id: uuid::Uuid::new_v4().to_string(),
            direction: ForwardDirection::Local,
            bind_host: bind_host.to_string(),
            bind_port,
            target_host: target_host.to_string(),
            target_port,
            active: true,
        };
        self.forwards.push(fwd);
        self.forwards.last().unwrap()
    }

    pub fn add_remote(
        &mut self,
        bind_host: &str,
        bind_port: u16,
        target_host: &str,
        target_port: u16,
    ) -> &PortForward {
        let fwd = PortForward {
            id: uuid::Uuid::new_v4().to_string(),
            direction: ForwardDirection::Remote,
            bind_host: bind_host.to_string(),
            bind_port,
            target_host: target_host.to_string(),
            target_port,
            active: true,
        };
        self.forwards.push(fwd);
        self.forwards.last().unwrap()
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let len_before = self.forwards.len();
        self.forwards.retain(|f| f.id != id);
        let removed = self.forwards.len() < len_before;
        if removed && self.selected >= self.forwards.len() && !self.forwards.is_empty() {
            self.selected = self.forwards.len() - 1;
        }
        removed
    }

    pub fn summary(&self) -> String {
        if self.forwards.is_empty() {
            return String::new();
        }
        self.forwards
            .iter()
            .filter(|f| f.active)
            .map(|f| format!("{}:{}→{}", f.direction, f.bind_port, f.target_port))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self.forwards.is_empty()
    }

    pub fn select_next(&mut self) {
        if !self.forwards.is_empty() {
            self.selected = (self.selected + 1) % self.forwards.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.forwards.is_empty() {
            if self.selected == 0 {
                self.selected = self.forwards.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.forwards.get(self.selected).map(|f| f.id.as_str())
    }
}

/// Parse a forward spec like "L:8080:localhost:80" or "R:3306:localhost:3306".
/// Returns (direction, bind_port, target_host, target_port) on success.
pub fn parse_forward_spec(spec: &str) -> Option<(ForwardDirection, u16, String, u16)> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() != 4 {
        return None;
    }
    let direction = match parts[0] {
        "L" | "l" => ForwardDirection::Local,
        "R" | "r" => ForwardDirection::Remote,
        _ => return None,
    };
    let bind_port: u16 = parts[1].parse().ok()?;
    let target_host = parts[2].to_string();
    let target_port: u16 = parts[3].parse().ok()?;
    Some((direction, bind_port, target_host, target_port))
}

// ---------------------------------------------------------------------------
// Actual forwarding logic
// ---------------------------------------------------------------------------

/// Start a local TCP port forward: binds a local listener and proxies each
/// accepted connection through the SSH session via `channel_open_direct_tcpip`.
///
/// Returns a `JoinHandle` that can be aborted to cancel the forward.
#[allow(dead_code)]
pub fn start_local_forward(
    ssh_handle: russh::client::Handle<crate::ssh::ClientHandler>,
    forward: &PortForward,
) -> tokio::task::JoinHandle<()> {
    let bind_addr = format!("{}:{}", forward.bind_host, forward.bind_port);
    let target_host = forward.target_host.clone();
    let target_port = forward.target_port as u32;
    let ssh_handle = Arc::new(ssh_handle);

    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
            Ok(l) => l,
            Err(_) => return,
        };

        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };

            let handle = Arc::clone(&ssh_handle);
            let host = target_host.clone();

            tokio::spawn(async move {
                let channel: russh::Channel<russh::client::Msg> = match handle
                    .channel_open_direct_tcpip(&host, target_port, "127.0.0.1", 0)
                    .await
                {
                    Ok(ch) => ch,
                    Err(_) => return,
                };

                let (mut reader, mut writer) = tokio::io::split(stream);
                let mut channel = channel;

                // Proxy data bidirectionally
                let mut buf = vec![0u8; 8192];
                loop {
                    tokio::select! {
                        result = tokio::io::AsyncReadExt::read(&mut reader, &mut buf) => {
                            match result {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    if channel.data(&buf[..n]).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        msg = channel.wait() => {
                            match msg {
                                Some(russh::ChannelMsg::Data { data }) => {
                                    if tokio::io::AsyncWriteExt::write_all(&mut writer, &data).await.is_err() {
                                        break;
                                    }
                                }
                                Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => break,
                                _ => {}
                            }
                        }
                    }
                }
            });
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager_is_empty() {
        let mgr = PortForwardManager::new();
        assert!(mgr.is_empty());
        assert!(mgr.forwards.is_empty());
        assert_eq!(mgr.summary(), "");
    }

    #[test]
    fn test_add_local_forward() {
        let mut mgr = PortForwardManager::new();
        let fwd = mgr.add_local("127.0.0.1", 8080, "localhost", 80);
        assert_eq!(fwd.direction, ForwardDirection::Local);
        assert_eq!(fwd.bind_port, 8080);
        assert_eq!(fwd.target_port, 80);
        assert!(fwd.active);
        assert!(!mgr.is_empty());
        assert_eq!(mgr.forwards.len(), 1);
    }

    #[test]
    fn test_add_remote_forward() {
        let mut mgr = PortForwardManager::new();
        let fwd = mgr.add_remote("0.0.0.0", 3306, "localhost", 3306);
        assert_eq!(fwd.direction, ForwardDirection::Remote);
        assert_eq!(fwd.bind_port, 3306);
        assert_eq!(fwd.target_port, 3306);
        assert!(fwd.active);
    }

    #[test]
    fn test_summary_formatting() {
        let mut mgr = PortForwardManager::new();
        mgr.add_local("127.0.0.1", 8080, "localhost", 80);
        mgr.add_remote("0.0.0.0", 3306, "localhost", 3306);
        assert_eq!(mgr.summary(), "L:8080→80 R:3306→3306");
    }

    #[test]
    fn test_summary_empty() {
        let mgr = PortForwardManager::new();
        assert_eq!(mgr.summary(), "");
    }

    #[test]
    fn test_remove_forward() {
        let mut mgr = PortForwardManager::new();
        mgr.add_local("127.0.0.1", 8080, "localhost", 80);
        let id = mgr.forwards[0].id.clone();
        assert!(mgr.remove(&id));
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut mgr = PortForwardManager::new();
        assert!(!mgr.remove("nonexistent-id"));
    }

    #[test]
    fn test_remove_preserves_other_forwards() {
        let mut mgr = PortForwardManager::new();
        mgr.add_local("127.0.0.1", 8080, "localhost", 80);
        mgr.add_remote("0.0.0.0", 3306, "localhost", 3306);
        let id = mgr.forwards[0].id.clone();
        mgr.remove(&id);
        assert_eq!(mgr.forwards.len(), 1);
        assert_eq!(mgr.forwards[0].direction, ForwardDirection::Remote);
    }

    #[test]
    fn test_parse_forward_spec_local() {
        let result = parse_forward_spec("L:8080:localhost:80");
        assert!(result.is_some());
        let (dir, bind, host, target) = result.unwrap();
        assert_eq!(dir, ForwardDirection::Local);
        assert_eq!(bind, 8080);
        assert_eq!(host, "localhost");
        assert_eq!(target, 80);
    }

    #[test]
    fn test_parse_forward_spec_remote() {
        let result = parse_forward_spec("R:3306:localhost:3306");
        assert!(result.is_some());
        let (dir, ..) = result.unwrap();
        assert_eq!(dir, ForwardDirection::Remote);
    }

    #[test]
    fn test_parse_forward_spec_invalid() {
        assert!(parse_forward_spec("X:8080:localhost:80").is_none());
        assert!(parse_forward_spec("L:abc:localhost:80").is_none());
        assert!(parse_forward_spec("L:8080:localhost").is_none());
        assert!(parse_forward_spec("").is_none());
    }

    #[test]
    fn test_direction_display() {
        assert_eq!(format!("{}", ForwardDirection::Local), "L");
        assert_eq!(format!("{}", ForwardDirection::Remote), "R");
    }

    #[test]
    fn test_select_navigation() {
        let mut mgr = PortForwardManager::new();
        mgr.add_local("127.0.0.1", 8080, "localhost", 80);
        mgr.add_remote("0.0.0.0", 3306, "localhost", 3306);
        assert_eq!(mgr.selected, 0);
        mgr.select_next();
        assert_eq!(mgr.selected, 1);
        mgr.select_next();
        assert_eq!(mgr.selected, 0);
        mgr.select_prev();
        assert_eq!(mgr.selected, 1);
    }
}
