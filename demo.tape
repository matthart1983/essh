# ─────────────────────────────────────────────────────────────────────
#  ESSH Demo — Enterprise SSH Client
#  Real TUI, real SSH session, real interactions
# ─────────────────────────────────────────────────────────────────────

Output demo.gif

Require ./target/release/essh

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 15
Set Width 1400
Set Height 750
Set Padding 15
Set Framerate 30
Set TypingSpeed 70ms
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set BorderRadius 8
Set Margin 12
Set MarginFill "#1a1a2e"
Set CursorBlink true

# ── Build fresh (hidden) ──────────────────────────────────────────────
Hide
Type "clear"
Enter
Sleep 500ms
Show

# ── Launch ESSH ──────────────────────────────────────────────────────
Type@120ms "./target/release/essh"
Sleep 700ms
Enter
Sleep 2s

# ── Dashboard: Sessions tab (default) ────────────────────────────────
# Pause to let viewer see the dashboard
Sleep 2s

# ── Switch to Hosts tab ──────────────────────────────────────────────
Type "2"
Sleep 2s

# ── Browse host list — navigate down through hosts ───────────────────
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms

# Pause at bottom
Sleep 1s

# Navigate back up
Up
Sleep 400ms
Up
Sleep 400ms
Up
Sleep 400ms
Up
Sleep 400ms
Up
Sleep 400ms
Up
Sleep 1.5s

# ── Search/filter hosts with "/" ──────────────────────────────────────
Type "/"
Sleep 600ms
Type@100ms "prod"
Sleep 2s

# Clear search
Escape
Sleep 1s

# ── Fleet tab — live health overview ──────────────────────────────────
Type "3"
Sleep 3s

# ── Config tab ────────────────────────────────────────────────────────
Type "4"
Sleep 2s

# ── Back to Hosts, connect to localhost ───────────────────────────────
Type "2"
Sleep 800ms

# Search for local
Type "/"
Sleep 400ms
Type@90ms "local"
Sleep 1s

# Select and connect
Enter
Sleep 3s

# ── Live SSH session — type real commands ─────────────────────────────
Type@80ms "echo '=== ESSH Live Session ==='"
Sleep 300ms
Enter
Sleep 1s

Type@80ms "uname -a"
Sleep 300ms
Enter
Sleep 1.5s

Type@80ms "uptime"
Sleep 300ms
Enter
Sleep 1s

Type@80ms "df -h / | head -3"
Sleep 300ms
Enter
Sleep 1.5s

Type@80ms "ps aux --sort=-%cpu | head -6"
Sleep 300ms
Enter
Sleep 2s

# ── Help overlay (?) ─────────────────────────────────────────────────
Alt+h
Sleep 3s
Alt+h
Sleep 1s

# ── Command Palette (Ctrl+P) from session ────────────────────────────
Ctrl+p
Sleep 1.5s

# Type a search to filter palette entries
Type@90ms "host"
Sleep 1.5s

# Clear and try another search
Backspace 4
Sleep 400ms
Type@90ms "fleet"
Sleep 1.5s

# Select entry to go to Fleet dashboard
Enter
Sleep 2s

# ── Command Palette again — search for host to connect ───────────────
Ctrl+p
Sleep 1s
Type@90ms "web"
Sleep 1.5s

# Dismiss palette
Escape
Sleep 1s

# ── Back to hosts tab, connect to localhost again for monitor demo ────
Type "2"
Sleep 500ms
Type "/"
Sleep 300ms
Type@90ms "local"
Sleep 500ms
Enter
Sleep 3s

# ── File browser (Alt+f) ─────────────────────────────────────────────
Alt+f
Sleep 3s

# Navigate a bit
Down
Sleep 400ms
Down
Sleep 400ms
Down
Sleep 400ms
Up
Sleep 400ms

# Close file browser
Escape
Sleep 1.5s

# ── More terminal commands to show it's a real session ────────────────
Type@80ms "ls -la"
Sleep 300ms
Enter
Sleep 1.5s

# ── Detach back to dashboard ─────────────────────────────────────────
Alt+d
Sleep 2s

# ── Final look: Hosts tab with all hosts ─────────────────────────────
Type "2"
Sleep 2s

# ── Quit ──────────────────────────────────────────────────────────────
Type "q"
Sleep 1.5s
