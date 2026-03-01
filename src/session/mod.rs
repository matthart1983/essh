pub mod manager;

use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    Connecting,
    Active,
    Suspended,
    Reconnecting { attempt: u32, max: u32 },
    Disconnected { reason: String },
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionState::Connecting => write!(f, "Connecting"),
            SessionState::Active => write!(f, "Active"),
            SessionState::Suspended => write!(f, "Suspended"),
            SessionState::Reconnecting { attempt, max } => {
                write!(f, "Reconnecting ({}/{})", attempt, max)
            }
            SessionState::Disconnected { reason } => write!(f, "Disconnected: {}", reason),
        }
    }
}

/// Virtual terminal backed by vt100 parser — properly interprets ANSI escapes
pub struct VirtualTerminal {
    parser: vt100::Parser,
    last_rows: u16,
    last_cols: u16,
}

impl VirtualTerminal {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            last_rows: rows,
            last_cols: cols,
        }
    }

    pub fn process(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    /// Resize the virtual terminal. Returns true if the size actually changed.
    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        if rows == self.last_rows && cols == self.last_cols {
            return false;
        }
        self.parser.screen_mut().set_size(rows, cols);
        self.last_rows = rows;
        self.last_cols = cols;
        true
    }

    /// Get the visible screen contents as lines of (text, style) pairs
    pub fn screen_lines(&self) -> Vec<Vec<StyledCell>> {
        let screen = self.parser.screen();
        let (rows, cols) = (screen.size().0, screen.size().1);
        let mut lines = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let mut line = Vec::new();
            let mut col = 0;
            while col < cols {
                let cell = screen.cell(row, col).unwrap();
                let ch = cell.contents();
                let fg = convert_color(cell.fgcolor());
                let bg = convert_color(cell.bgcolor());
                let bold = cell.bold();
                let underline = cell.underline();
                let inverse = cell.inverse();

                line.push(StyledCell {
                    text: if ch.is_empty() {
                        " ".to_string()
                    } else {
                        ch.to_string()
                    },
                    fg,
                    bg,
                    bold,
                    underline,
                    inverse,
                });
                col += 1;
            }
            lines.push(line);
        }
        lines
    }

    /// Get cursor position (row, col)
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }
}

#[derive(Clone, Debug)]
pub struct StyledCell {
    pub text: String,
    pub fg: Option<ratatui::style::Color>,
    pub bg: Option<ratatui::style::Color>,
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
}

fn convert_color(color: vt100::Color) -> Option<ratatui::style::Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ratatui::style::Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(ratatui::style::Color::Rgb(r, g, b)),
    }
}

/// Represents a single SSH session
pub struct Session {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub state: SessionState,
    pub terminal: VirtualTerminal,
    pub created_at: Instant,
    pub has_new_output: bool,
    pub jump_host: Option<String>,
}

impl Session {
    pub fn new(
        id: String,
        label: String,
        hostname: String,
        port: u16,
        username: String,
        _scrollback: usize,
    ) -> Self {
        // Default terminal size, will be resized on first render
        Self {
            id,
            label,
            hostname,
            port,
            username,
            state: SessionState::Connecting,
            terminal: VirtualTerminal::new(24, 80),
            created_at: Instant::now(),
            has_new_output: false,
            jump_host: None,
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
}
