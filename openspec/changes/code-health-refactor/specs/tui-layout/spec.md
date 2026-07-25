## ADDED Requirements

### Requirement: Screen geometry has one source
Screen region arithmetic — the frame's main/info/bottom split, the list/detail pane split, and the detail pane's body/comments split — SHALL be computed by pure functions in one module, called by both the renderer and the key handler. Neither SHALL re-derive the other's arithmetic.

#### Scenario: The renderer lays out the frame
- **WHEN** `ui::draw` computes its regions
- **THEN** it obtains them from the shared layout functions rather than from inline `Layout` calls

#### Scenario: The key handler needs viewport dimensions
- **WHEN** a scroll or selection key handler needs the detail pane's inner width or a region's viewport height
- **THEN** it obtains them from the same shared layout functions, given the current terminal size
- **AND** it does not restate the pane percentages, the border insets, or the status-line row count

#### Scenario: A layout constant changes
- **WHEN** a pane percentage, border inset, or split ratio is changed in the layout module
- **THEN** the renderer and the key handler both observe the change, with no second edit required and no comment instructing a human to keep them aligned

### Requirement: Geometry is computed without mutating application state
The layout functions SHALL be pure functions of their inputs. Draw code SHALL NOT write geometry back onto application state for later reads.

#### Scenario: A key arrives before the first draw
- **WHEN** a key handler needs geometry
- **THEN** it computes it directly from the current terminal size
- **AND** its correctness does not depend on a draw having already run

### Requirement: The PR summary's rows and its open-able targets share one model
The PR summary popup's rendered rows and the navigable targets within it SHALL be produced by one function returning a single ordered model, where each entry carries its rendered line and an optional URL. Target row indices SHALL be positions in that model, never separately computed.

#### Scenario: The popup is drawn
- **WHEN** the PR summary popup renders
- **THEN** it draws the lines of the shared row model in order

#### Scenario: Targets are enumerated for navigation
- **WHEN** `Tab` or `Shift+Tab` cycles the PR summary selection
- **THEN** the targets are the entries of that same row model which carry a URL, and each target's row index is its position in the model

#### Scenario: A PR body contains a line longer than the popup's inner width
- **WHEN** the summary is shown for a PR whose body has a line exceeding the popup's inner width
- **THEN** the highlighted row is the one the user selected, and scrolling brings that same row into view
- **AND** the selection does not drift by the number of extra rows the long line occupies

#### Scenario: The popup's layout gains or loses a line
- **WHEN** a heading, blank line, or section is added to or removed from the popup
- **THEN** target row indices follow automatically, with no matching edit needed elsewhere

### Requirement: Wrapping is owned by the application, not the widget
Regions whose scroll offsets or row indices are computed by application code SHALL pre-wrap their content with the application's own wrapping and render with widget wrapping disabled, so that a rendered row and a computed row are the same unit.

#### Scenario: A region's content exceeds its width
- **WHEN** content in the detail pane or the PR summary popup is wider than its region
- **THEN** it is wrapped by `tui::linkmap` before rendering, and the rendering widget performs no further wrapping
- **AND** row counts computed by the application match the rows actually drawn
