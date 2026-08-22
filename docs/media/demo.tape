Output "/private/tmp/claude-501/-Users-matt/52bcdf98-12f0-46a8-ba6b-de0805ba7717/scratchpad/essh-demo.mp4"
Set Shell bash
Set FontFamily "Menlo"
Set FontSize 14
Set Width 1500
Set Height 820
Set Padding 12
Set Framerate 24
Set TypingSpeed 30ms
Set Theme { "background": "#0c1418", "foreground": "#c8d4d9", "cursor": "#5fdcff", "black": "#0c1418", "red": "#ff7878", "green": "#5cd989", "yellow": "#f0c060", "blue": "#5fdcff", "magenta": "#b8a8e8", "cyan": "#5fdcff", "white": "#c8d4d9", "brightBlack": "#42555d", "brightRed": "#ff7878", "brightGreen": "#5cd989", "brightYellow": "#f0c060", "brightBlue": "#5fdcff", "brightMagenta": "#b8a8e8", "brightCyan": "#5fdcff", "brightWhite": "#ffffff" }
Hide
Type "export HOME=/private/tmp/claude-501/-Users-matt/52bcdf98-12f0-46a8-ba6b-de0805ba7717/scratchpad/demohome"
Enter
Type "clear"
Enter
Show
Sleep 400ms

# search and connect
Type "essh"
Enter
Sleep 1100ms
Type "web-01"
Sleep 400ms
Enter
Sleep 4s
Hide
Type "clear"
Enter
Show
Sleep 250ms

# a real terminal
Type "uptime; free -h | head -2"
Enter
Sleep 1300ms
Type "df -h / && nproc"
Enter
Sleep 1400ms

# F2 — host monitor
Ctrl+A
Sleep 250ms
Type "m"
Sleep 3800ms
Escape
Sleep 500ms

# F5 — monitor beside the shell
Ctrl+A
Sleep 250ms
Type "M"
Sleep 700ms
Type "ps -eo pid,args,pcpu --sort=-pcpu | head -5"
Enter
Sleep 2200ms
Ctrl+A
Sleep 250ms
Type "M"
Sleep 500ms

# F10 — command menu
Ctrl+A
Sleep 250ms
Type "k"
Sleep 1800ms
Escape
Sleep 400ms

# F9 — second session, F7/F8 to switch
Ctrl+A
Sleep 250ms
Type "n"
Sleep 800ms
Type "web-02"
Sleep 400ms
Enter
Sleep 3200ms
Type "hostname; uptime"
Enter
Sleep 1300ms
Ctrl+A
Sleep 250ms
Left
Sleep 1400ms

# F6 — detach to the fleet
Ctrl+A
Sleep 250ms
Type "d"
Sleep 1200ms
Type "3"
Sleep 2400ms
Type "q"
Sleep 600ms
