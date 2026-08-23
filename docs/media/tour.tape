# tour.tape — the README demo.
#
# Rendered by scripts/demo.sh, which substitutes @HOME@ and @OUT@ and builds
# the three hosts. Do not run this file directly; the placeholders are there so
# the tape stays reproducible on any machine.
#
# Everything shown is collected over live SSH against three real Debian
# containers — two bookworm, one bullseye, with an nginx.conf edited by hand on
# the odd one out. The divergence is genuine, not scripted.
#
# Eight beats, about 35 seconds. Each one holds long enough to read.

Output "@OUT@"

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 14
Set Width 1500
Set Height 820
Set Padding 12
Set Framerate 24
Set TypingSpeed 35ms
Set WindowBar Colorful
Set BorderRadius 9
Set Margin 14
Set MarginFill "#05080a"
Set CursorBlink false
Set Theme { "background": "#0c1418", "foreground": "#c8d4d9", "cursor": "#5fdcff", "black": "#0c1418", "red": "#ff7878", "green": "#5cd989", "yellow": "#f0c060", "blue": "#5fdcff", "magenta": "#b8a8e8", "cyan": "#5fdcff", "white": "#c8d4d9", "brightBlack": "#42555d", "brightRed": "#ff7878", "brightGreen": "#5cd989", "brightYellow": "#f0c060", "brightBlue": "#5fdcff", "brightMagenta": "#b8a8e8", "brightCyan": "#5fdcff", "brightWhite": "#ffffff" }

Hide
Type "export HOME=@HOME@"
Enter
Type "alias essh='./target/release/essh'"
Enter
Type "clear"
Enter
Show

# ── 1. Search and connect ──────────────────────────────────────────────
Type "essh"
Enter
Sleep 1500ms
Type "web-01"
Sleep 2s
Enter
Sleep 4s

# ── 2. It is a real shell, not a viewer ────────────────────────────────
Type "uptime && df -h / | tail -1"
Enter
Sleep 3s

# ── 3. F2 — the host monitor, with peer medians ────────────────────────
# Escape back to the shell before the next chord. Leaving the monitor up means
# every prefix after this one is typed into the remote shell instead.
Ctrl+A
Sleep 400ms
Type "m"
Sleep 4500ms
Escape
Sleep 600ms

# ── 4. F5 — the monitor beside the shell, one hairline between them ────
Ctrl+A
Sleep 400ms
Type "M"
Sleep 1s
Type "ps -eo pid,args,pcpu --sort=-pcpu | head -4"
Enter
Sleep 3s
Ctrl+A
Sleep 400ms
Type "M"
Sleep 800ms

# ── 5. F9 — a second host. This is the one that turns out to differ, and
#       the shell shows why a beat before the fleet screen says so. ────────
Ctrl+A
Sleep 400ms
Type "n"
Sleep 1200ms
Type "web-03"
Sleep 1s
Enter
Sleep 3s
Type "cat /etc/debian_version"
Enter
Sleep 2s

# ── 6. The fleet, and the one host that disagrees ──────────────────────
Ctrl+A
Sleep 400ms
Type "d"
Sleep 1200ms
Type "3"
Sleep 3s
# Land on web-03 — the host connected a beat ago, whose shell just printed
# debian 11. The card then says what that costs.
#
# Three presses, not one: the table sorts by divergence but the selection
# walks the configured order, so the count here is not the row position. That
# mismatch is a real defect, noted rather than worked around silently.
Down
Down
Down
Sleep 500ms
Type "D"
Sleep 6s
Escape
Sleep 800ms

# ── 7. Eight themes, cycled with one key ───────────────────────────────
Type "t"
Sleep 1s
Type "t"
Sleep 1s
Type "t"
Sleep 1s
Type "t"
Sleep 1200ms

# ── 8. The command menu, and the themes in it ──────────────────────────
Ctrl+P
Sleep 1s
Type "theme"
Sleep 2500ms
Escape
Sleep 600ms

Hide
Type "q"
Sleep 500ms
