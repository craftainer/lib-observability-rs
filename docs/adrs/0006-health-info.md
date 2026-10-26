# 0006. An opt-in `/health/info` router

## Status

Accepted

## Context

AX9a adds the `/info` route that `lib-observability-py` has behind
`include_info` (default off, because it discloses the version). ADR 0002 lists
it as a later item. Python returns `{"version", "commit"}` as JSON with status
200 and no caching headers, reading `APP_VERSION` and `APP_COMMIT` on every
request; the version falls back to the installed distribution's metadata
(`info_package`).

## Decision

`health::info::info_router<S>() -> Router<S>` is a separate router that the
host merges next to `router()`, for example
`router().merge(info_router())` nested under `/health`. There is no flag on
`router()`; not merging it is the off switch, so it is off by default.
It takes no state and is generic over any `S: Clone + Send + Sync + 'static`,
like `router()`. It needs no Cargo feature.

Matched with Python: `GET /info`, status 200, `application/json`, no extra
headers, body `{"version", "commit"}`, an unset or empty variable is `null`,
variables read on every request.

Deviations from Python:

- **No package-version fallback.** A library cannot know its host's package
  version, so an unset `APP_VERSION` is `null` (user decision, 2026-10-27).
  Python's `info_package` has no counterpart.
- **A non-UTF-8 value is `null`.** Python decodes it; Rust's `env::var`
  reports an error, which is treated as unset.

## Consequences

A host sets `APP_VERSION` and `APP_COMMIT` in its image, as template-axum's
build args already provide (AX13 mounts the route). The route is
unauthenticated like `/live`; a host that does not want the version public
simply does not merge it. Tests inject the environment through a private
helper rather than mutating the process's.
