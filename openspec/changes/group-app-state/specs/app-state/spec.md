## ADDED Requirements

### Requirement: State that resets together is grouped together
Fields that are cleared or initialised as a unit SHALL live in one struct with a `Default`, rather than as sibling fields on `App`. Fields with no such shared lifecycle SHALL remain flat.

#### Scenario: A group is reset to its initial state
- **WHEN** a concern's state is discarded wholesale
- **THEN** it is reset by assigning the group's `Default`, not by clearing each field at the call site

#### Scenario: A new field is added to an existing concern
- **WHEN** a field is added to a grouped concern
- **THEN** every reset of that concern accounts for it without any call site being edited

#### Scenario: A field has no shared lifecycle
- **WHEN** a field is not cleared or initialised alongside others — the status message, the loading flag, the observed rate limit
- **THEN** it stays a direct field of `App`, because grouping it would add indirection without recording anything

### Requirement: Partial resets are named, not inlined
Where a concern is reset only in part, that partial reset SHALL be a named method on the group describing the intent. The differing subsets SHALL be preserved exactly.

#### Scenario: The PR summary popup is closed
- **WHEN** the PR summary is closed
- **THEN** the target, summary, scroll and selection are cleared
- **AND** the discovered PR links are **retained**, so reopening the picker does not require refetching them

#### Scenario: The PR summary is refreshed in place
- **WHEN** a refresh is requested for the PR already being shown
- **THEN** the summary, scroll and selection are cleared and the target is retained, so the response can be matched against it

#### Scenario: PR state is discarded entirely
- **WHEN** the detail pane opens or closes, or the org is switched
- **THEN** all PR state including the links is discarded

### Requirement: Logic belongs to the state it operates on
A function that reads and writes only one group's fields SHALL be a method on that group, not on `App`. A function that spans groups SHALL stay on `App` and delegate.

#### Scenario: Picker index arithmetic is exercised
- **WHEN** the type-ahead filter narrows the option list and the highlighted index must be clamped
- **THEN** that logic is reachable and testable without constructing an `App`

#### Scenario: An operation spans concerns
- **WHEN** opening the detail pane, which moves keyboard focus and discards PR state as well as setting up the pane
- **THEN** the entry point stays on `App` and delegates to each group, since no single group owns the operation

### Requirement: Grouping does not change behaviour
Restructuring state SHALL NOT alter what the application does.

#### Scenario: The change is reviewed
- **WHEN** the regrouping is complete
- **THEN** the existing tests pass unedited, and the characterisation goldens are untouched
- **AND** the function inventory and the test roster are unchanged
