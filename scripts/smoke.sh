#!/usr/bin/env bash
# Smoke test: record-then-replay traces against a real control plane.
#
# Runs the hello-trace worker twice with the same run identity:
#   attempt 1 (FLYTE_ATTEMPT_NUMBER=0) executes the task and records its traces
#     as child actions via the ActionsService;
#   attempt 2 (FLYTE_ATTEMPT_NUMBER=1) must REPLAY every trace from the backend
#     instead of re-running the bodies.
#
# Two modes:
#   demo/hosted (authenticated):  FLYTE_API_KEY=... ./scripts/smoke.sh
#       key from: flyte create api-key --name rust-sdk-smoke
#       defaults: org=demo project=flytesnacks domain=development
#   devbox (unauthenticated):     ./scripts/smoke.sh
#       requires the devbox control plane on localhost:8090
#       defaults: org=testorg project=testproject domain=development
#
# Overrides: SMOKE_ORG SMOKE_PROJECT SMOKE_DOMAIN SMOKE_RUN_NAME SMOKE_ENDPOINT
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -n "${FLYTE_API_KEY:-}" ]; then
  MODE=api-key
  ORG="${SMOKE_ORG:-demo}"
  PROJECT="${SMOKE_PROJECT:-flytesnacks}"
else
  MODE=devbox
  ORG="${SMOKE_ORG:-testorg}"
  PROJECT="${SMOKE_PROJECT:-testproject}"
fi
DOMAIN="${SMOKE_DOMAIN:-development}"
RUN_NAME="${SMOKE_RUN_NAME:-rust-smoke-$(date +%s)}"
BASE="/tmp/flyte-rust-smoke/$RUN_NAME"
mkdir -p "$BASE"

cargo build -p hello-trace
BIN=target/debug/hello-trace
FIXTURE=target/debug/smoke_fixture

# Echo the interface the worker declares: the contract the launcher reads.
echo "=== interface ==="
"$BIN" describe-interface

"$FIXTURE" write-inputs "$BASE/inputs.pb"

run_worker() {
  local attempt="$1"
  local -a envs=(
    RUN_NAME="$RUN_NAME"
    ACTION_NAME=a0
    FLYTE_ATTEMPT_NUMBER="$attempt"
    FLYTE_INTERNAL_EXECUTION_PROJECT="$PROJECT"
    FLYTE_INTERNAL_EXECUTION_DOMAIN="$DOMAIN"
    _U_ORG_NAME="$ORG"
    _U_RUN_BASE="$BASE"
    RUST_LOG="${RUST_LOG:-info}"
  )
  if [ "$MODE" = api-key ]; then
    envs+=(_UNION_EAGER_API_KEY="$FLYTE_API_KEY")
  else
    envs+=(_U_EP_OVERRIDE="${SMOKE_ENDPOINT:-localhost:8090}" _U_INSECURE=1)
  fi
  env "${envs[@]}" "$BIN" --inputs "$BASE/inputs.pb" --outputs-path "$BASE/a0" 2>&1
}

echo "=== [$MODE] run $RUN_NAME attempt 1 (execute + record traces) ==="
run_worker 0 | tee "$BASE/attempt1.log"
grep -q "task succeeded" "$BASE/attempt1.log" || { echo "FAIL: attempt 1 did not succeed"; exit 1; }
grep -q "replaying recorded trace" "$BASE/attempt1.log" && { echo "FAIL: attempt 1 unexpectedly replayed"; exit 1; }

echo "=== [$MODE] run $RUN_NAME attempt 2 (replay traces) ==="
run_worker 1 | tee "$BASE/attempt2.log"
grep -q "task succeeded" "$BASE/attempt2.log" || { echo "FAIL: attempt 2 did not succeed"; exit 1; }
REPLAYS=$(grep -c "replaying recorded trace" "$BASE/attempt2.log" || true)
echo "replayed traces: $REPLAYS (expected 3)"
[ "$REPLAYS" -eq 3 ] || { echo "FAIL: expected 3 replayed traces"; exit 1; }

echo "=== outputs ==="
"$FIXTURE" read-outputs "$BASE/a0/outputs.pb"
echo "SMOKE PASSED: mode=$MODE run=$RUN_NAME org=$ORG project=$PROJECT domain=$DOMAIN"
