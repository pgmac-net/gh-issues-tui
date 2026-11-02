## ADDED Requirements

### Requirement: A session belongs to exactly one issue
Launching a harness for an issue that already has a live session SHALL attach to it rather than start a second agent, whichever entry point is used.

#### Scenario: The launch key is pressed twice on one issue
- **WHEN** `A` is pressed on an issue whose session is running
- **THEN** that session is shown, and no second child is spawned

#### Scenario: A harness is chosen from the picker for an issue that already has one
- **WHEN** a harness is selected from the picker, or `F12 n` is used, for an issue with a running session
- **THEN** the existing session is attached instead, because the rule lives in the launch path rather than in any one caller

#### Scenario: The issue's previous session has exited
- **WHEN** a launch is requested for an issue whose session has exited
- **THEN** the user is asked first, because relaunching discards the exited screen
- **AND** on confirmation the exited session is replaced, not accumulated alongside the new one

### Requirement: Detaching is not killing
Leaving a session SHALL return to the issue list with the child still running.

#### Scenario: The detach chord is used
- **WHEN** `F12 d` is pressed in a session
- **THEN** the issue list is shown and the child keeps running, its output still being parsed

#### Scenario: A detached session is re-entered
- **WHEN** a detached session is attached again
- **THEN** its screen is as the child has since drawn it, opened at the newest output

### Requirement: The child owns the keyboard except for one reserved key
All keys SHALL be forwarded to the child except a single reserved prefix, which SHALL NOT be a key any coding agent binds.

#### Scenario: A key an agent binds is pressed
- **WHEN** `Esc`, `Ctrl+C`, `Shift+Tab` or an arrow key is pressed in a running session
- **THEN** it is encoded and written to the child, not interpreted by the TUI

#### Scenario: The reserved key itself must reach the child
- **WHEN** the reserved prefix is pressed twice
- **THEN** one literal press of that key is sent to the child, so no key is permanently unavailable

#### Scenario: An unrecognised chord key is pressed
- **WHEN** the prefix is followed by a key with no chord meaning
- **THEN** the chord is disarmed and the available chords are shown, rather than the key reaching the child

### Requirement: An exited session keeps its output
A session whose child has exited SHALL remain until dismissed, with its final screen readable.

#### Scenario: The child exits on its own
- **WHEN** a harness child exits
- **THEN** the session is marked with its exit code and its last screen is retained and scrollable

#### Scenario: An exited session is dismissed
- **WHEN** the kill chord is used on an exited session
- **THEN** it is removed with no confirmation, because there is nothing left to lose

### Requirement: Quitting names what it will destroy
Quitting with sessions still running SHALL require confirmation listing them.

#### Scenario: Quit is pressed with running sessions
- **WHEN** quit is pressed and any session is running
- **THEN** a confirmation lists each running session by issue reference and harness before anything is terminated

#### Scenario: Quit is pressed with only exited sessions
- **WHEN** quit is pressed and no session is running
- **THEN** the application exits without asking

### Requirement: Harness commands are argv arrays
A harness command SHALL be an array of arguments executed without a shell, and each placeholder SHALL expand into exactly one argument.

#### Scenario: Issue text contains shell metacharacters
- **WHEN** an expanded placeholder contains quotes, backticks or `$(…)`
- **THEN** the text is passed to the child inert, as a single argument, because no shell parses it

#### Scenario: A configured harness is not installed
- **WHEN** the command's program is not on `PATH`
- **THEN** the failure is reported in the status line and no session is registered

### Requirement: A harness runs in the issue's clone
A harness SHALL start in the working directory of the clone belonging to the issue's repository.

#### Scenario: The TUI was started inside the issue's repo
- **WHEN** the current directory's repository is the issue's repository
- **THEN** the harness runs there, whatever the configured roots say

#### Scenario: No clone can be found
- **WHEN** no configured root contains a directory named after the repository
- **THEN** nothing is launched and the message names every path that was tried

### Requirement: Session output cannot stall the interface
Output from a session SHALL NOT be routed through the application event channel, and a session that is not on screen SHALL NOT force a redraw.

#### Scenario: A detached agent produces heavy output
- **WHEN** a session that is not being displayed writes output
- **THEN** the bytes are parsed on that session's own thread and no frame is drawn

### Requirement: A session's terminal matches the pane it is drawn in
A session's PTY SHALL be resized whenever the terminal is, including sessions not currently displayed.

#### Scenario: The terminal is resized while a session is detached
- **WHEN** the terminal size changes
- **THEN** every session's PTY is resized, so a session attached later is already correct
