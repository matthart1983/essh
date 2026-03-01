use std::path::PathBuf;

/// Represents a remote file entry
#[derive(Clone, Debug)]
pub struct RemoteFileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    #[allow(dead_code)]
    pub permissions: String,
    #[allow(dead_code)]
    pub modified: String,
}

/// Represents a local file entry
#[derive(Clone, Debug)]
pub struct LocalFileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

/// Which pane is focused in the file browser
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilePaneFocus {
    Local,
    Remote,
}

/// Transfer operation in progress or completed
#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub filename: String,
    pub direction: TransferDirection,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub enum TransferDirection {
    Upload,
    Download,
}

impl TransferProgress {
    pub fn percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.bytes_transferred as f64 / self.total_bytes as f64) * 100.0
        }
    }
}

/// File browser state
pub struct FileBrowser {
    pub local_path: PathBuf,
    pub remote_path: String,
    pub local_files: Vec<LocalFileEntry>,
    pub remote_files: Vec<RemoteFileEntry>,
    pub local_selected: usize,
    pub remote_selected: usize,
    pub focus: FilePaneFocus,
    pub transfer: Option<TransferProgress>,
    pub status_message: Option<String>,
}

impl FileBrowser {
    pub fn new() -> Self {
        let local_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            local_path,
            remote_path: "/home".to_string(),
            local_files: Vec::new(),
            remote_files: Vec::new(),
            local_selected: 0,
            remote_selected: 0,
            focus: FilePaneFocus::Local,
            transfer: None,
            status_message: None,
        }
    }

    pub fn list_local_files(&mut self) {
        self.local_files.clear();
        let read_dir = match std::fs::read_dir(&self.local_path) {
            Ok(rd) => rd,
            Err(e) => {
                self.status_message = Some(format!("Error reading dir: {}", e));
                return;
            }
        };

        let mut entries: Vec<LocalFileEntry> = Vec::new();
        for entry in read_dir.flatten() {
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            entries.push(LocalFileEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.local_files = entries;
        if self.local_selected >= self.local_files.len() {
            self.local_selected = 0;
        }
    }

    pub fn next_file(&mut self) {
        match self.focus {
            FilePaneFocus::Local => {
                if !self.local_files.is_empty() {
                    self.local_selected = (self.local_selected + 1) % self.local_files.len();
                }
            }
            FilePaneFocus::Remote => {
                if !self.remote_files.is_empty() {
                    self.remote_selected = (self.remote_selected + 1) % self.remote_files.len();
                }
            }
        }
    }

    pub fn prev_file(&mut self) {
        match self.focus {
            FilePaneFocus::Local => {
                if !self.local_files.is_empty() {
                    if self.local_selected == 0 {
                        self.local_selected = self.local_files.len() - 1;
                    } else {
                        self.local_selected -= 1;
                    }
                }
            }
            FilePaneFocus::Remote => {
                if !self.remote_files.is_empty() {
                    if self.remote_selected == 0 {
                        self.remote_selected = self.remote_files.len() - 1;
                    } else {
                        self.remote_selected -= 1;
                    }
                }
            }
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FilePaneFocus::Local => FilePaneFocus::Remote,
            FilePaneFocus::Remote => FilePaneFocus::Local,
        };
    }

    pub fn enter_dir_local(&mut self) {
        if let Some(entry) = self.selected_local() {
            if entry.is_dir {
                let new_path = entry.path.clone();
                self.local_path = new_path;
                self.local_selected = 0;
                self.list_local_files();
            }
        }
    }

    pub fn parent_local(&mut self) {
        if let Some(parent) = self.local_path.parent() {
            self.local_path = parent.to_path_buf();
            self.local_selected = 0;
            self.list_local_files();
        }
    }

    pub fn enter_dir_remote(&mut self) {
        if let Some(entry) = self.remote_files.get(self.remote_selected) {
            if entry.is_dir {
                let name = entry.name.clone();
                if self.remote_path.ends_with('/') {
                    self.remote_path = format!("{}{}", self.remote_path, name);
                } else {
                    self.remote_path = format!("{}/{}", self.remote_path, name);
                }
                self.remote_selected = 0;
            }
        }
    }

    pub fn parent_remote(&mut self) {
        if let Some(pos) = self.remote_path.rfind('/') {
            if pos == 0 {
                self.remote_path = "/".to_string();
            } else {
                self.remote_path = self.remote_path[..pos].to_string();
            }
            self.remote_selected = 0;
        }
    }

    pub fn selected_local(&self) -> Option<&LocalFileEntry> {
        self.local_files.get(self.local_selected)
    }

    pub fn selected_remote(&self) -> Option<&RemoteFileEntry> {
        self.remote_files.get(self.remote_selected)
    }
}

/// Parse `ls -la` output into remote file entries.
pub fn parse_ls_output(output: &str) -> Vec<RemoteFileEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        // Skip header line ("total NNN") and empty lines
        if line.is_empty() || line.starts_with("total ") {
            continue;
        }
        // Split on whitespace, filtering empty parts from consecutive spaces
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let permissions = parts[0].to_string();
        let size: u64 = parts[4].parse().unwrap_or(0);
        let modified = format!("{} {} {}", parts[5], parts[6], parts[7]);
        // Name may contain spaces — rejoin everything from index 8 onward
        let name = parts[8..].join(" ");

        // Skip . and ..
        if name == "." || name == ".." {
            continue;
        }

        let is_dir = permissions.starts_with('d');
        entries.push(RemoteFileEntry {
            name,
            is_dir,
            size,
            permissions,
            modified,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_filebrowser_creation() {
        let browser = FileBrowser::new();
        assert_eq!(browser.focus, FilePaneFocus::Local);
        assert_eq!(browser.remote_path, "/home");
        assert!(browser.local_files.is_empty());
        assert!(browser.remote_files.is_empty());
        assert_eq!(browser.local_selected, 0);
        assert_eq!(browser.remote_selected, 0);
        assert!(browser.transfer.is_none());
        assert!(browser.status_message.is_none());
    }

    #[test]
    fn test_local_file_listing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("alpha.txt"), "hello").unwrap();
        fs::write(dir.path().join("beta.txt"), "world").unwrap();
        fs::create_dir(dir.path().join("gamma_dir")).unwrap();

        let mut browser = FileBrowser::new();
        browser.local_path = dir.path().to_path_buf();
        browser.list_local_files();

        assert_eq!(browser.local_files.len(), 3);
        // Directories should come first
        assert!(browser.local_files[0].is_dir);
        assert_eq!(browser.local_files[0].name, "gamma_dir");
    }

    #[test]
    fn test_navigation_next_prev_wrap() {
        let mut browser = FileBrowser::new();
        browser.local_files = vec![
            LocalFileEntry { name: "a".into(), path: PathBuf::from("/a"), is_dir: false, size: 0 },
            LocalFileEntry { name: "b".into(), path: PathBuf::from("/b"), is_dir: false, size: 0 },
            LocalFileEntry { name: "c".into(), path: PathBuf::from("/c"), is_dir: false, size: 0 },
        ];
        browser.focus = FilePaneFocus::Local;
        browser.local_selected = 0;

        browser.next_file();
        assert_eq!(browser.local_selected, 1);
        browser.next_file();
        assert_eq!(browser.local_selected, 2);
        browser.next_file();
        assert_eq!(browser.local_selected, 0); // wrap

        browser.prev_file();
        assert_eq!(browser.local_selected, 2); // wrap back
        browser.prev_file();
        assert_eq!(browser.local_selected, 1);
    }

    #[test]
    fn test_navigation_remote_pane() {
        let mut browser = FileBrowser::new();
        browser.remote_files = vec![
            RemoteFileEntry { name: "x".into(), is_dir: false, size: 10, permissions: "-rw-r--r--".into(), modified: "Jan 1 00:00".into() },
            RemoteFileEntry { name: "y".into(), is_dir: false, size: 20, permissions: "-rw-r--r--".into(), modified: "Jan 1 00:00".into() },
        ];
        browser.focus = FilePaneFocus::Remote;
        browser.remote_selected = 0;

        browser.next_file();
        assert_eq!(browser.remote_selected, 1);
        browser.next_file();
        assert_eq!(browser.remote_selected, 0); // wrap
    }

    #[test]
    fn test_focus_toggling() {
        let mut browser = FileBrowser::new();
        assert_eq!(browser.focus, FilePaneFocus::Local);
        browser.toggle_focus();
        assert_eq!(browser.focus, FilePaneFocus::Remote);
        browser.toggle_focus();
        assert_eq!(browser.focus, FilePaneFocus::Local);
    }

    #[test]
    fn test_transfer_progress_percentage() {
        let progress = TransferProgress {
            filename: "test.txt".into(),
            direction: TransferDirection::Upload,
            bytes_transferred: 75,
            total_bytes: 100,
            complete: false,
        };
        assert!((progress.percent() - 75.0).abs() < f64::EPSILON);

        let zero = TransferProgress {
            filename: "empty.txt".into(),
            direction: TransferDirection::Download,
            bytes_transferred: 0,
            total_bytes: 0,
            complete: false,
        };
        assert!((zero.percent() - 0.0).abs() < f64::EPSILON);

        let done = TransferProgress {
            filename: "done.txt".into(),
            direction: TransferDirection::Upload,
            bytes_transferred: 500,
            total_bytes: 500,
            complete: true,
        };
        assert!((done.percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_enter_directory_local() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inner.txt"), "data").unwrap();

        let mut browser = FileBrowser::new();
        browser.local_path = dir.path().to_path_buf();
        browser.list_local_files();

        // First entry should be the directory
        assert!(browser.local_files[0].is_dir);
        browser.local_selected = 0;
        browser.enter_dir_local();

        assert_eq!(browser.local_path, sub);
        assert_eq!(browser.local_files.len(), 1);
        assert_eq!(browser.local_files[0].name, "inner.txt");
    }

    #[test]
    fn test_parent_local() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("child");
        fs::create_dir(&sub).unwrap();

        let mut browser = FileBrowser::new();
        browser.local_path = sub.clone();
        browser.list_local_files();
        browser.parent_local();

        assert_eq!(browser.local_path, dir.path());
    }

    #[test]
    fn test_selected_local_empty() {
        let browser = FileBrowser::new();
        assert!(browser.selected_local().is_none());
    }

    #[test]
    fn test_selected_remote_empty() {
        let browser = FileBrowser::new();
        assert!(browser.selected_remote().is_none());
    }

    #[test]
    fn test_parse_ls_output() {
        let output = "\
total 16
drwxr-xr-x 2 user user 4096 Jan  1 10:00 .ssh
-rw-r--r-- 1 user user  220 Jan  1 10:00 .bashrc
-rw-r--r-- 1 user user 3771 Jan  1 10:00 .profile
drwxr-xr-x 3 user user 4096 Jan  1 10:00 app
";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 4);
        // Dirs first
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert!(!entries[2].is_dir);
        assert!(!entries[3].is_dir);
    }

    #[test]
    fn test_parse_ls_output_skips_dots() {
        let output = "\
total 8
drwxr-xr-x 3 user user 4096 Jan  1 10:00 .
drwxr-xr-x 3 root root 4096 Jan  1 10:00 ..
-rw-r--r-- 1 user user  100 Jan  1 10:00 file.txt
";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.txt");
    }

    #[test]
    fn test_enter_dir_remote() {
        let mut browser = FileBrowser::new();
        browser.remote_path = "/home".to_string();
        browser.remote_files = vec![
            RemoteFileEntry { name: "user".into(), is_dir: true, size: 4096, permissions: "drwxr-xr-x".into(), modified: "Jan 1 10:00".into() },
        ];
        browser.remote_selected = 0;
        browser.enter_dir_remote();
        assert_eq!(browser.remote_path, "/home/user");
    }

    #[test]
    fn test_parent_remote() {
        let mut browser = FileBrowser::new();
        browser.remote_path = "/home/user".to_string();
        browser.parent_remote();
        assert_eq!(browser.remote_path, "/home");

        browser.parent_remote();
        assert_eq!(browser.remote_path, "/");
    }
}
