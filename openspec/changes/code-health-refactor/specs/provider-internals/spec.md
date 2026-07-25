## ADDED Requirements

### Requirement: Shared HTTP plumbing lives in the provider layer
Logic that is not specific to a single backend — the HTTP client construction, the rate-limit state store and its user-facing message, GraphQL error joining, and JSON path extraction — SHALL live in `src/provider/` and be used by every backend that needs it. A backend SHALL NOT carry its own copy.

#### Scenario: A backend needs an HTTP client
- **WHEN** `github::Client`, `linear::Client`, or `jira::Client` is constructed
- **THEN** it obtains its `reqwest::Client` from the shared provider-layer builder, supplying only its own authorization header value (`Bearer {token}`, a raw key, or `Basic {base64}`)
- **AND** the user agent, sensitive-header marking, and default-header wiring come from the shared builder, not from the backend

#### Scenario: A GraphQL response carries errors
- **WHEN** a GraphQL response body contains a non-empty `errors` array
- **THEN** the messages are joined by the single shared `join_error_messages` implementation
- **AND** neither `github/client.rs` nor `linear/client.rs` defines its own copy of that function

#### Scenario: A backend extracts a typed value from a JSON path
- **WHEN** a backend needs a typed value at a nested JSON path
- **THEN** it calls the single shared `parse_at` implementation, which reports a `ProviderError::Shape` naming the missing path
- **AND** neither `github/client.rs` nor `linear/client.rs` defines its own copy of that function

#### Scenario: Rate limit state is observed
- **WHEN** a backend reads rate-limit headers from a response
- **THEN** it supplies its own header names and unit conversion (GitHub's `x-ratelimit-*` in seconds, Linear's `x-ratelimit-requests-*` in milliseconds)
- **AND** the resulting state is stored in, and the user-facing message formatted by, the shared rate-limit store — the message format string SHALL exist in exactly one place

### Requirement: Backend-specific behaviour stays with its backend
The shared provider layer SHALL NOT absorb logic that only one backend has. Provider-specific error detection, pagination strategy, and priority value mappings remain in that backend's module.

#### Scenario: GitHub handles complexity-budget backoff
- **WHEN** GitHub returns a resource-limit error
- **THEN** `graphql_with_backoff` and its page-size halving remain in `github/client.rs`, not in the shared layer

#### Scenario: GitHub reports a rate limit as HTTP 200
- **WHEN** GitHub returns HTTP 200 with a `RATE_LIMITED` error entry
- **THEN** that detection remains in `github/client.rs`, since no other backend behaves this way

#### Scenario: Linear reports a rate limit in the errors array
- **WHEN** Linear returns an errors array indicating a rate limit
- **THEN** `errors_contain_ratelimit` remains in `linear/client.rs`

### Requirement: Synthetic priority label helpers are prefix-parameterised
Backends whose priority is a native field rather than a label (Linear, Jira) SHALL share one implementation of the synthetic `priority:*` label machinery, parameterised by the backend's prefix, rather than each carrying an identical copy.

#### Scenario: A backend offers priority options to the picker
- **WHEN** Linear or Jira builds its label list for the priority picker or new-issue form
- **THEN** the four synthetic `(id, name)` pairs are produced by the single shared helper, given that backend's prefix
- **AND** the pairs are ordered urgent, high, medium, low, with names of the form `priority:<value>`

#### Scenario: A synthetic id is peeled on mutation
- **WHEN** a label id carrying the backend's synthetic prefix reaches a create or update path
- **THEN** the shared helper strips the prefix to recover the `priority:<value>` value
- **AND** the backend maps that value to its own native representation — a Linear integer, or a Jira priority name — using its own mapping table
- **AND** the synthetic id is never transmitted to the backend

#### Scenario: A real label id is passed through
- **WHEN** a label id does not carry the backend's synthetic prefix
- **THEN** the shared helper reports no match and the id is treated as a real label
