# pw Demo
# Run: vhs demo.tape

Output demo.gif

Set Shell zsh
Set FontSize 14
Set Width 1200
Set Height 600
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

# --- Session Picker ---
Hide
Type "cargo run -p pw -- --fresh"
Enter
Sleep 500ms
Show
Sleep 4s

# Browse down through sessions
Down
Sleep 400ms
Down

Sleep 1s

# Switch to previous year to show year navigation
Left
Sleep 2s

# Come back to current year
Right
Sleep 1s

# Scroll back up to the China race (2nd from top)
Down 3

Sleep 500ms

# Select China race session
Enter
Sleep 8s

Right 30

# --- Timing Board ---

# Scroll down through drivers
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 600ms

Sleep 1s

# Scroll back up
Up
Sleep 400ms
Up
Sleep 400ms
Up
Sleep 400ms
Up
Sleep 400ms

Sleep 1s

# Toggle race control panel off
Type "r"
Sleep 3s

# Toggle race control panel on
Type "r"
Sleep 1s

# --- Telemetry ---

# Move to a driver near the top for telemetry
Up
Sleep 300ms
Up
Sleep 300ms
Up
Sleep 300ms

# Open telemetry
Type "t"
Sleep 4s

# Scroll back through telemetry history
Type "h"
Sleep 2s
Type "h"
Sleep 2s
Type "h"
Sleep 2s
Type "h"
Sleep 2s

# Jump back to live edge
Type "0"
Sleep 6s

# Switch to next driver
Down
Sleep 8s

# Close telemetry
Type "t"
Sleep 3s

# show map
Space
Type "m"
Sleep 5s

# Quit
Type "q"
Sleep 1s
