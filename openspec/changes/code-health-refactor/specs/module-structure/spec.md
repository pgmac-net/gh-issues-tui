## ADDED Requirements

### Requirement: TUI modules are bounded in size and scope
No TUI source file SHALL carry more than roughly 600 lines of production code. Each file SHALL cover one identifiable area of the interface, and its name SHALL say which.

#### Scenario: A file grows past the bound
- **WHEN** a file's production line count approaches 600
- **THEN** it is split along an existing seam rather than allowed to keep growing

#### Scenario: A contributor looks for a feature
- **WHEN** someone needs the code for the PR summary popup, the issue form, the detail pane, or a picker
- **THEN** the file names in `src/tui/app/`, `src/tui/event/`, and `src/tui/ui/` identify where each layer of it lives

### Requirement: The three-layer separation is preserved
State and pure logic, key handling and asynchronous work, and rendering SHALL remain in separate modules. Splitting a layer into submodules SHALL NOT move responsibility between layers.

#### Scenario: A layer is split into submodules
- **WHEN** `app.rs`, `event.rs`, or `ui.rs` is split
- **THEN** state and pure logic stay under `app/`, key handling and spawned work stay under `event/`, rendering stays under `ui/`
- **AND** `app/` performs no I/O, and `ui/` mutates no state

#### Scenario: Call sites reference a moved item
- **WHEN** code outside a split module refers to an item that moved into a submodule
- **THEN** the parent module re-exports it so the existing path continues to resolve

### Requirement: Structural refactors do not change behaviour
A change whose purpose is moving code SHALL NOT alter behaviour in the same step. Movement and logic edits SHALL be separated into different phases.

#### Scenario: A file split is reviewed
- **WHEN** the diff for a module split is examined
- **THEN** additions and deletions are near-symmetric, and changed lines are confined to imports, module declarations, and visibility
- **AND** any semantic difference is treated as an error to be removed, not as incidental cleanup

#### Scenario: Tests move with their code
- **WHEN** a function moves to a new submodule
- **THEN** its unit tests move with it, unedited

### Requirement: Architecture documentation tracks the module layout
`CLAUDE.md` SHALL describe the actual module layout, and SHALL NOT instruct contributors to manually keep two pieces of code synchronised where the code itself can enforce it.

#### Scenario: The module layout changes
- **WHEN** files are split or added under `src/tui/`
- **THEN** `CLAUDE.md`'s architecture section is updated in the same change

#### Scenario: A duplicated invariant is single-sourced
- **WHEN** a refactor removes the need for two places to be kept in agreement by hand
- **THEN** the corresponding "keep both in sync" instruction is deleted from `CLAUDE.md`, because the hazard it warns about no longer exists
