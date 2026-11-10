# flyte_core 0.1.0, patched

Source: crates.io `flyte_core` 0.1.0, unchanged except for the following.

1. **A watch stream the server ends is reconnected** (`src/informer.rs`).
   0.1.0 treats `Ok(None)` from `stream.message()` as "keep looping". After the
   stream ends, that returns at once on every call, so the watch task spins
   without yielding and never reconnects. Every child completion after that is
   lost, and `wait_for_action` waits forever. The fix is backported from
   flyte-sdk main (`rs_controller/src/informer.rs`): end of stream is a
   retryable `StreamError`, and the retry budget resets after a healthy stream.
2. **HTTP/2 keepalive pings** (`src/core.rs`). `keep_alive_while_idle(true)`
   does nothing without `http2_keep_alive_interval`, so a connection dropped
   silently by the network left the stream pending forever. Pings every 30s,
   20s timeout, as Python's client does. Not yet in flyte-sdk main.

Seen as: demo run ugq9mw6f6zjr99pss8w6. All 19 children succeeded, and the Rust
parent hung in `call_task` for 2h with no further log lines.
