# hero.tape — the README hero.
#
# One idea, told in eleven seconds: your fleet disagrees with itself, and ESSH
# says how. The tour lives in demo-divergence.tape; this is not a tour.
#
# Everything shown is collected from three real Debian containers — two
# bookworm, one bullseye, with a hand-edited nginx.conf on the odd one out.
# See the reproduction notes at the foot of demo-divergence.tape.

Output hero.gif

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 15
Set Width 1760
Set Height 680
Set Padding 16
Set Framerate 20
Set WindowBar Colorful
Set BorderRadius 9
Set Margin 16
Set MarginFill "#05080a"
Set CursorBlink false
Set Theme { "background": "#0c1418", "foreground": "#c8d4d9", "cursor": "#5fdcff", "black": "#0c1418", "red": "#ff7878", "green": "#5cd989", "yellow": "#f0c060", "blue": "#5fdcff", "magenta": "#b8a8e8", "cyan": "#5fdcff", "white": "#c8d4d9", "brightBlack": "#42555d", "brightRed": "#ff7878", "brightGreen": "#5cd989", "brightYellow": "#f0c060", "brightBlue": "#5fdcff", "brightMagenta": "#b8a8e8", "brightCyan": "#5fdcff", "brightWhite": "#ffffff" }

# Everything up to the payoff happens off-camera. A hero that opens on a shell
# prompt has spent its first two seconds before it says anything.
Hide
Type "export HOME=/private/tmp/claude-501/-Users-matt/0c72e3f9-c761-4ed4-8c59-344e9459894b/scratchpad/demohome"
Enter
Type "clear"
Enter
# Divergence is computed from facts collected over live sessions, so the
# hidden half opens all three and waits for them to report. Nothing is staged;
# the differences are whatever the containers actually have.
Type "./target/release/essh workspace open production"
Enter
Sleep 25s
# Detach to the dashboard, then the Fleet tab.
Ctrl+A
Sleep 500ms
Type "d"
Sleep 2s
Type "3"
Sleep 6s
Show

# ── The hook: three hosts, and one of them disagrees. ──────────────────
Sleep 3s

# ── The payoff: what it disagrees about. ───────────────────────────────
Down
Down
Sleep 800ms
Type "D"
Sleep 7s

Hide
Type "q"
Sleep 500ms
