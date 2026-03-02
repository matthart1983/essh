# ─────────────────────────────────────────────────────────────────────
#  ESSH Demo — Enterprise SSH Client
#  Real TUI, real SSH, organic typing
# ─────────────────────────────────────────────────────────────────────

Output demo.gif

Require ./target/release/essh

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 15
Set Width 1800
Set Height 1000
Set Padding 15
Set Framerate 30
Set TypingSpeed 70ms
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set BorderRadius 8
Set Margin 12
Set MarginFill "#1a1a2e"
Set CursorBlink true

# ── Setup (hidden) ────────────────────────────────────────────────────
Hide
Type "clear"
Enter
Sleep 500ms
Show

# ══════════════════════════════════════════════════════════════════════
#  ACT 1: Launch & Explore Dashboard
# ══════════════════════════════════════════════════════════════════════

Type@120ms "./target/release/essh"
Sleep 500ms
Enter
Sleep 2.5s

# Sessions tab (default) — let viewer take it in
Sleep 1.5s

# ── Hosts tab — filter straight to localhost ──────────────────────────
Type "2"
Sleep 1.5s

Type "/"
Sleep 400ms
Type@90ms "local"
Sleep 1.5s

# Connect
Enter
Sleep 3.5s

# ── Sysadmin commands ─────────────────────────────────────────────────
Type@75ms "top -l 1 -n 5 -s 0 | head -12"
Sleep 200ms
Enter
Sleep 1.5s

Type@75ms "diskutil list | head -15"
Sleep 200ms
Enter
Sleep 1.5s

Type@75ms "netstat -an | grep LISTEN | head -10"
Sleep 200ms
Enter
Sleep 2s

# ══════════════════════════════════════════════════════════════════════
#  ACT 5: Host Monitor (remote htop via Alt+m)
# ══════════════════════════════════════════════════════════════════════

# µ = macOS Option+m → triggers monitor toggle
Type "µ"
Sleep 5s

# Return to terminal
Escape
Sleep 1s

# ══════════════════════════════════════════════════════════════════════
#  ACT 6: Split-Pane View (terminal + monitor)
# ══════════════════════════════════════════════════════════════════════

# ß = macOS Option+s → triggers split-pane toggle
Type "ß"
Sleep 3s

# Run commands while split — both panes live
Type@75ms "tail -5 /var/log/system.log"
Sleep 200ms
Enter
Sleep 2s

Type@75ms "last -5"
Sleep 200ms
Enter
Sleep 2s

# Close split
Type "ß"
Sleep 1s

# ══════════════════════════════════════════════════════════════════════
#  ACT 7: File Browser (Alt+f)
# ══════════════════════════════════════════════════════════════════════

Alt+f
Sleep 2.5s

# Browse local files
Down
Sleep 400ms
Down
Sleep 400ms
Down
Sleep 400ms

# Switch to remote pane
Tab
Sleep 1.2s

Down
Sleep 400ms
Down
Sleep 400ms

# Back to local
Tab
Sleep 1s

# Close browser
Escape
Sleep 1.5s

# ══════════════════════════════════════════════════════════════════════
#  ACT 8: Command Palette (Ctrl+P)
# ══════════════════════════════════════════════════════════════════════

Ctrl+p
Sleep 1.5s

# Search: "host"
Type@90ms "host"
Sleep 1.5s

# Clear, try "monitor"
Backspace 4
Sleep 300ms
Type@90ms "monitor"
Sleep 1.5s

# Clear, search "fleet"
Backspace 7
Sleep 300ms
Type@90ms "fleet"
Sleep 1.5s

# Select — navigate to Fleet tab
Enter
Sleep 2.5s

# ══════════════════════════════════════════════════════════════════════
#  ACT 9: Multi-Session (connect second host)
# ══════════════════════════════════════════════════════════════════════

# Use palette to go to hosts
Ctrl+p
Sleep 800ms
Type@90ms "hosts"
Sleep 800ms
Enter
Sleep 1.5s

# Connect second session to localhost
Type "/"
Sleep 300ms
Type@90ms "local"
Sleep 600ms
Enter
Sleep 3s

# Run a command in second session
Type@75ms "hostname && date"
Sleep 200ms
Enter
Sleep 1.5s

# Switch between sessions: Alt+Tab = last-used
Alt+Tab
Sleep 1.5s

# Switch back
Alt+Tab
Sleep 1.5s

# ══════════════════════════════════════════════════════════════════════
#  ACT 10: Help Overlay
# ══════════════════════════════════════════════════════════════════════

Alt+h
Sleep 3.5s
Alt+h
Sleep 1s

# ══════════════════════════════════════════════════════════════════════
#  ACT 11: Detach & wrap up
# ══════════════════════════════════════════════════════════════════════

# Detach to dashboard
Alt+d
Sleep 1.5s

# Sessions tab — show 2 active sessions
Type "1"
Sleep 2s

# Final fleet view
Type "3"
Sleep 2.5s

# ── Quit ──────────────────────────────────────────────────────────────
Type "q"
Sleep 1.5s
