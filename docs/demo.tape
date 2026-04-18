# VHS tape for generating the README screenshot/GIF.
# Install VHS: https://github.com/charmbracelet/vhs
# Run: vhs docs/demo.tape

Output docs/demo.gif
Set Width 800
Set Height 480
Set Shell zsh
Set FontSize 14
Set Padding 0
Set TypingSpeed 100ms

Hide
Type 'cargo build; export PATH="./target/debug:$PATH"; clear'
Enter
Wait
Show
# Misspelt
Type 'pipe-explorer git grep test \| gre '
Type "'#\['"
Sleep 1s
Enter
Sleep 2s

# Navigate to the broken stage to see its output
Type "l"
Sleep 0.5s

# Press 3 to see combined stdout+stderr — reveals the error message
Type "3"
Sleep 0.5s

# Edit the command to fix the typo: gre → grep
Type "e"
Sleep 1s
Ctrl+A
Right 3
Type "p"
Tab
Sleep 0.5s
Ctrl+E
Ctrl+Left
Type "--color=always "
Sleep 0.5s
Enter
Sleep 2s
Type "1"
Sleep 500ms

# Add a sort stage to rank files by match count
Type "o"
Sleep 500ms
Type "sort -t:"
Enter
Sleep 1.5s

Type "o"
Sleep 500ms
Type "cut -d: -f1
Enter
Sleep 1.5s

Type "o"
Sleep 500ms
Type "uniq -c"
Enter 
Sleep 2s

Type 'h'
Sleep 0.3s
Type 'h'
Sleep 0.3s
Type 'h'
Sleep 0.3s
Type 'e'
Sleep 0.3s

Ctrl+Left
Type '-v '
Tab
Sleep 1.5s
Enter

Type 'l'
Sleep 1.5s
Type 'l'
Sleep 1.5s
Type 'l'
Sleep 2s

# Quit
Type "q"
