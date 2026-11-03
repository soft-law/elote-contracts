#!/usr/bin/env bash
# The scripts end to end on a local validator: the demo (pool, deposits, a line drawn and repaid,
# a redemption), then request-line.ts, six draws with draw.ts, one payoff with repay.ts (split into
# parts of at most five pagaré records) and every cancel-pagare.ts branch that does not need the
# clock moved forward. A third line covers endorse-pagare.ts (an endoso en procuración, a revocation)
# and the procurador's cancellation. The backstop's success needs seven days to pass; the Rust suite
# covers it.
#
#   scripts/local-e2e.sh            (after `NO_DNA=1 anchor build --ignore-keys --arch v0 -- --features localnet`)
#
# Local only: the validator preloads target/deploy/elote_pool.so and the pinned Metaplex Core
# binary at their program ids (`--bpf-program`), which is not a deploy. The RPC must be this
# machine; the ledger and every key live in a temporary directory removed on exit, so the
# repository's .demo-keys/ is never read or written.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${ELOTE_E2E_PORT:-18899}"
RPC="http://127.0.0.1:${PORT}"
PROGRAM_ID="BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM"
CORE_ID="CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d"
PROGRAM_SO="$REPO/target/deploy/elote_pool.so"
CORE_SO="$REPO/programs/elote_pool/tests/fixtures/mpl_core_program-0.15.3.so"
TSX="$REPO/node_modules/.bin/tsx"

case "${ELOTE_RPC:-$RPC}" in
  http://127.0.0.1:*|http://localhost:*) ;;
  *) echo "error: local-e2e runs against this machine only (ELOTE_RPC=${ELOTE_RPC})" >&2; exit 1 ;;
esac
[ -f "$PROGRAM_SO" ] || { echo "error: build first: NO_DNA=1 anchor build --ignore-keys --arch v0 -- --features localnet" >&2; exit 1; }
[ -f "$CORE_SO" ] || { echo "error: missing $CORE_SO" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/elote-e2e.XXXXXX")"
VALIDATOR_PID=""
cleanup() {
  if [ -n "$VALIDATOR_PID" ]; then kill "$VALIDATOR_PID" 2>/dev/null || true; wait "$VALIDATOR_PID" 2>/dev/null || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "== local validator on $RPC (ledger in a temporary directory)"
solana-test-validator --ledger "$WORK/ledger" --rpc-port "$PORT" --faucet-port "$((PORT + 1))" --quiet --reset \
  --bpf-program "$PROGRAM_ID" "$PROGRAM_SO" --bpf-program "$CORE_ID" "$CORE_SO" >"$WORK/validator.log" 2>&1 &
VALIDATOR_PID=$!
for _ in $(seq 1 60); do
  if curl -s "$RPC" -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q '"ok"'; then break; fi
  sleep 1
done
curl -s "$RPC" -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q '"ok"' \
  || { echo "error: the validator did not start; see its log:" >&2; tail -20 "$WORK/validator.log" >&2; exit 1; }

export ELOTE_RPC="$RPC"
cd "$WORK"
run() { "$TSX" "$REPO/scripts/$1" --cluster local "${@:2}"; }
KEYS="$WORK/.demo-keys"
HASH="$(printf 'e2e' | shasum -a 256 | cut -d' ' -f1)"
RECEIPT="$(printf 'e2e receipt' | shasum -a 256 | cut -d' ' -f1)"

echo "== demo-pool.ts"
run demo-pool.ts | tee "$WORK/demo.log"
POOL="$(sed -n 's/^pool \([1-9A-HJ-NP-Za-km-z]\{32,44\}\)$/\1/p' "$WORK/demo.log" | tail -1)"
[ -n "$POOL" ] || { echo "error: no pool address in the demo's output" >&2; exit 1; }

echo "== cancel-pagare.ts: the demo line's pagaré 0, with the attestor"
run cancel-pagare.ts --pool "$POOL" --line-id 0 --draw-index 0 --receipt-hash "$RECEIPT"

echo "== request-line.ts: a guarantor-sponsored bullet line of 600"
run request-line.ts --pool "$POOL" --borrower-keypair "$KEYS/borrower.json" --principal 600 \
  --sponsor-keypair "$KEYS/sponsor.json" --sponsor-role guarantor --expediente-root "$HASH" --agreement-hash "$HASH"

echo "== draw.ts: six tranches of 100"
for i in 0 1 2 3 4 5; do
  run draw.ts --pool "$POOL" --line-id 1 --borrower-keypair "$KEYS/borrower.json" --amount 100 --document-hash "$HASH"
done

echo "== repay.ts --all: six pagaré records, so two repay parts"
# The interest is 12% a year on 600 for about 90 days; the borrower holds the 600 it drew, the
# sponsor tops it up.
run repay.ts --pool "$POOL" --line-id 1 --amount 20 --payer-keypair "$KEYS/sponsor.json"
run repay.ts --pool "$POOL" --line-id 1 --all | tee "$WORK/repay.log"
grep -q 'repay part 2' "$WORK/repay.log" || { echo "error: the payoff was not split" >&2; exit 1; }
grep -q 'status       Repaid' "$WORK/repay.log" || { echo "error: the line is not repaid" >&2; exit 1; }

echo "== cancel-pagare.ts: pagaré 0 with the attestor; pagaré 1 requested, then refused alone before the deadline"
run cancel-pagare.ts --pool "$POOL" --line-id 1 --draw-index 0 --receipt-hash "$RECEIPT"
run cancel-pagare.ts --pool "$POOL" --line-id 1 --draw-index 1 --receipt-hash "$RECEIPT" --request
if run cancel-pagare.ts --pool "$POOL" --line-id 1 --draw-index 1 --receipt-hash "$RECEIPT" --backstop 2>"$WORK/backstop.err"; then
  echo "error: the backstop cancelled before the deadline" >&2; exit 1
fi
grep -q "attestor must sign" "$WORK/backstop.err" || { echo "error: unexpected refusal:" >&2; cat "$WORK/backstop.err" >&2; exit 1; }
echo "  refused as expected: $(cat "$WORK/backstop.err")"
echo "== cancel-pagare.ts: pagaré 1, with the attestor despite the pending request"
run cancel-pagare.ts --pool "$POOL" --line-id 1 --draw-index 1 --receipt-hash "$RECEIPT"

echo "== endorse-pagare.ts: a third line, pagaré 0 en procuración, pagaré 1 endorsed and revoked"
solana-keygen new --no-bip39-passphrase --silent -o "$KEYS/procurador.json" >/dev/null
PROCURADOR="$(solana-keygen pubkey "$KEYS/procurador.json")"
run request-line.ts --pool "$POOL" --borrower-keypair "$KEYS/borrower.json" --principal 300 --agreement-hash "$HASH"
for i in 0 1 2; do
  run draw.ts --pool "$POOL" --line-id 2 --borrower-keypair "$KEYS/borrower.json" --amount 100 --document-hash "$HASH"
done
run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 0 --procurador "$PROCURADOR" | tee "$WORK/endorse.log"
grep -q "procuración in force, procurador $PROCURADOR" "$WORK/endorse.log" || { echo "error: no procuración recorded" >&2; exit 1; }
run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 1 --procurador "$PROCURADOR"
run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 1 --revoke | tee "$WORK/revoke.log"
grep -q "procuración none" "$WORK/revoke.log" || { echo "error: the procuración was not revoked" >&2; exit 1; }
if run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 2 --procurador "$POOL" 2>"$WORK/endorsee.err"; then
  echo "error: the pool was accepted as procurador" >&2; exit 1
fi
echo "  refused as expected: $(cat "$WORK/endorsee.err")"

echo "== repay.ts: the sponsor pays the interest, the borrower the principal"
run repay.ts --pool "$POOL" --line-id 2 --amount 15 --payer-keypair "$KEYS/sponsor.json"
run repay.ts --pool "$POOL" --line-id 2 --all | tee "$WORK/repay2.log"
grep -q 'status       Repaid' "$WORK/repay2.log" || { echo "error: the third line is not repaid" >&2; exit 1; }

echo "== cancel-pagare.ts: pagaré 0 by the procurador, pagarés 1 and 2 by the holder with the attestor"
if run cancel-pagare.ts --pool "$POOL" --line-id 2 --draw-index 1 --receipt-hash "$RECEIPT" --procurador-keypair "$KEYS/procurador.json" 2>"$WORK/revoked.err"; then
  echo "error: the procurador cancelled a pagaré whose procuración was revoked" >&2; exit 1
fi
echo "  refused as expected: $(cat "$WORK/revoked.err")"
run cancel-pagare.ts --pool "$POOL" --line-id 2 --draw-index 0 --receipt-hash "$RECEIPT" --procurador-keypair "$KEYS/procurador.json" | tee "$WORK/procurador.log"
grep -q 'PAID (extinguished).*burnt' "$WORK/procurador.log" || { echo "error: the procurador's cancellation did not burn the asset" >&2; exit 1; }
run cancel-pagare.ts --pool "$POOL" --line-id 2 --draw-index 1 --receipt-hash "$RECEIPT"
run cancel-pagare.ts --pool "$POOL" --line-id 2 --draw-index 2 --receipt-hash "$RECEIPT"

echo "== local e2e passed"
