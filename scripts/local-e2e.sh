#!/usr/bin/env bash
# The scripts end to end on a local validator, with the council a real Squads v4 multisig:
# the demo (pool, the council's FIX, deposits, a MXN line approved by the council, drawn as USDC and
# repaid, a redemption; the demo's draw needs the cotejo key the council names, decision 56), then
# request-line.ts and approve-line.ts, six draws with draw.ts co-signed by a cotejo key set with
# council-propose.ts (one refused without it), one payoff
# with repay.ts (split into parts of at most three pagaré records), the council's settings and FIX,
# the delivery of every paid pagaré and the owner's burn. A third line covers
# endorse-pagare.ts (an endoso en procuración to a key made for the case, a revocation) and the
# procuración whose collection is the repayment itself (decision 38); the Rust suite covers the
# rest.
#
#   scripts/local-e2e.sh            (after `NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet`)
#
# Local only: the validator preloads target/deploy/elote_pool.so, the pinned Metaplex Core and
# Squads v4 binaries at their program ids (`--bpf-program`) and a 2-of-3 Squads multisig account of
# three keys made here (`--account`), none of which is a deploy. The RPC must be this machine; the
# ledger and every key live in a temporary directory removed on exit, so the repository's
# .demo-keys/ is never read or written.
#
# post_fix's market reference (RedStone's MXNe account) exists on mainnet only and is not loaded
# here: the localnet build skips the reference check when the account is missing. The LiteSVM
# suite (tests/fx.rs) injects the account and covers the check.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${ELOTE_E2E_PORT:-18899}"
RPC="http://127.0.0.1:${PORT}"
PROGRAM_ID="BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM"
CORE_ID="CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d"
SQUADS_ID="SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf"
PROGRAM_SO="$REPO/target/deploy/elote_pool.so"
CORE_SO="$REPO/programs/elote_pool/tests/fixtures/mpl_core_program-0.15.3.so"
CORE_SHA256="5b4dacdae4af476d3c9d8aa6712883f559ba0e13705f9b244d96d1fd0dd56ce5"
SQUADS_SO="$REPO/programs/elote_pool/tests/fixtures/squads_v4-mainnet.so"
SQUADS_SHA256="dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c"
TSX="$REPO/node_modules/.bin/tsx"

case "${ELOTE_RPC:-$RPC}" in
  http://127.0.0.1:*|http://localhost:*) ;;
  *) echo "error: local-e2e runs against this machine only (ELOTE_RPC=${ELOTE_RPC})" >&2; exit 1 ;;
esac
[ -f "$PROGRAM_SO" ] || { echo "error: build first: NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet" >&2; exit 1; }
for pair in "$CORE_SO:$CORE_SHA256" "$SQUADS_SO:$SQUADS_SHA256"; do
  file="${pair%%:*}"; want="${pair##*:}"
  [ -f "$file" ] || { echo "error: missing $file" >&2; exit 1; }
  [ "$(shasum -a 256 "$file" | cut -d' ' -f1)" = "$want" ] || { echo "error: $file is not the pinned binary" >&2; exit 1; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/elote-e2e.XXXXXX")"
VALIDATOR_PID=""
cleanup() {
  if [ -n "$VALIDATOR_PID" ]; then kill "$VALIDATOR_PID" 2>/dev/null || true; wait "$VALIDATOR_PID" 2>/dev/null || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

KEYS="$WORK/.demo-keys"
mkdir -p "$KEYS"
chmod 700 "$KEYS"
for i in 1 2 3; do solana-keygen new --no-bip39-passphrase --silent -o "$KEYS/curator-$i.json" >/dev/null; done
echo "== the council: a 2-of-3 Squads v4 multisig of three keys made for this run"
"$TSX" "$REPO/scripts/squads-local-multisig.ts" --members "$KEYS/curator-1.json,$KEYS/curator-2.json,$KEYS/curator-3.json" \
  --threshold 2 --out "$WORK/multisig.json" >"$WORK/multisig.txt"
MULTISIG="$(sed -n 1p "$WORK/multisig.txt")"
echo "  multisig $MULTISIG, council (vault 0) $(sed -n 2p "$WORK/multisig.txt")"

if curl -s --max-time 2 "$RPC" -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' >/dev/null 2>&1; then
  echo "error: something already answers on $RPC; set ELOTE_E2E_PORT to a free port (this run must talk to its own validator)" >&2; exit 1
fi
echo "== local validator on $RPC (ledger in a temporary directory)"
solana-test-validator --ledger "$WORK/ledger" --rpc-port "$PORT" --faucet-port "$((PORT + 1))" --gossip-port "$((PORT + 2))" \
  --dynamic-port-range "$((PORT + 3))-$((PORT + 40))" --quiet --reset \
  --bpf-program "$PROGRAM_ID" "$PROGRAM_SO" --bpf-program "$CORE_ID" "$CORE_SO" --bpf-program "$SQUADS_ID" "$SQUADS_SO" \
  --account "$MULTISIG" "$WORK/multisig.json" >"$WORK/validator.log" 2>&1 &
VALIDATOR_PID=$!
for _ in $(seq 1 60); do
  if curl -s "$RPC" -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q '"ok"'; then break; fi
  sleep 1
done
curl -s "$RPC" -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q '"ok"' \
  || { echo "error: the validator did not start; see its log:" >&2; tail -20 "$WORK/validator.log" >&2; exit 1; }
kill -0 "$VALIDATOR_PID" 2>/dev/null || { echo "error: the validator exited; see its log:" >&2; tail -20 "$WORK/validator.log" >&2; exit 1; }

export ELOTE_RPC="$RPC"
cd "$WORK"
run() { "$TSX" "$REPO/scripts/$1" --cluster local "${@:2}"; }
HASH="$(printf 'e2e' | shasum -a 256 | cut -d' ' -f1)"
RECEIPT="$(printf 'e2e receipt' | shasum -a 256 | cut -d' ' -f1)"
# The curators pay the council's fees and rent.
for i in 1 2 3; do solana airdrop --url "$RPC" 10 "$(solana-keygen pubkey "$KEYS/curator-$i.json")" >/dev/null; done

echo "== demo-pool.ts"
run demo-pool.ts --multisig "$MULTISIG" | tee "$WORK/demo.log"
POOL="$(sed -n 's/^pool \([1-9A-HJ-NP-Za-km-z]\{32,44\}\)$/\1/p' "$WORK/demo.log" | tail -1)"
[ -n "$POOL" ] || { echo "error: no pool address in the demo's output" >&2; exit 1; }
grep -q 'execute approve_line 0' "$WORK/demo.log" || { echo "error: the council did not approve through Squads" >&2; exit 1; }
grep -q 'refused as expected: draw signed by the borrower alone was rejected: .*cotejo' "$WORK/demo.log" \
  || { echo "error: the demo's draw without the cotejo was not refused by the program" >&2; exit 1; }
grep -q 'co-signed by the cotejo' "$WORK/demo.log" || { echo "error: the demo's co-signed draw did not land" >&2; exit 1; }

echo "== deliver-pagare.ts: the demo repay already delivered pagaré 0; a second delivery is refused"
if run deliver-pagare.ts --pool "$POOL" --line-id 0 --draw-index 0 2>"$WORK/redeliver.err"; then
  echo "error: a second delivery went through" >&2; exit 1
fi
grep -q "already delivered" "$WORK/redeliver.err" || { echo "error: unexpected refusal:" >&2; cat "$WORK/redeliver.err" >&2; exit 1; }
echo "  refused as expected: $(cat "$WORK/redeliver.err")"

echo "== council-propose.ts and post-fix.ts: a feeder named, the line cap restated, today's FIX corrected"
solana-keygen new --no-bip39-passphrase --silent -o "$KEYS/feeder.json" >/dev/null
FEEDER="$(solana-keygen pubkey "$KEYS/feeder.json")"
run council-propose.ts --pool "$POOL" --op set-fx-feeder --address "$FEEDER"
run council-propose.ts --pool "$POOL" --op set-max-line-bps --bps 5000
run post-fix.ts --pool "$POOL" --fix 18.3750 | tee "$WORK/fix.log"
grep -q 'FIX in force: 183750' "$WORK/fix.log" || { echo "error: the council's FIX correction did not land" >&2; exit 1; }

echo "== request-line.ts and approve-line.ts: a guarantor-sponsored bullet line of MXN 6,000"
run request-line.ts --pool "$POOL" --borrower-keypair "$KEYS/borrower.json" --principal 6000 \
  --sponsor-keypair "$KEYS/sponsor.json" --sponsor-role guarantor --expediente-root "$HASH" --agreement-hash "$HASH"
run approve-line.ts --pool "$POOL" --line-id 1 | tee "$WORK/approve.log"
grep -q 'status       Approved' "$WORK/approve.log" || { echo "error: the line was not approved" >&2; exit 1; }

echo "== council-propose.ts set-cotejo-keys: a cotejo key co-signs every draw (decision 56)"
solana-keygen new --no-bip39-passphrase --silent -o "$KEYS/cotejo-e2e.json" >/dev/null
COTEJO="$(solana-keygen pubkey "$KEYS/cotejo-e2e.json")"
run council-propose.ts --pool "$POOL" --op set-cotejo-keys --keys "$COTEJO"
if run draw.ts --pool "$POOL" --line-id 1 --borrower-keypair "$KEYS/borrower.json" --amount 1000 2>"$WORK/nocotejo.err"; then
  echo "error: a draw without the cotejo went through" >&2; exit 1
fi
grep -q "cotejo" "$WORK/nocotejo.err" || { echo "error: unexpected refusal:" >&2; cat "$WORK/nocotejo.err" >&2; exit 1; }
echo "  refused as expected: $(cat "$WORK/nocotejo.err")"

echo "== draw.ts: six tranches of MXN 1,000, co-signed by the cotejo, the first with a sealed copy's hash"
SEALED="$(printf 'e2e sealed copy' | shasum -a 256 | cut -d' ' -f1)"
run draw.ts --pool "$POOL" --line-id 1 --borrower-keypair "$KEYS/borrower.json" --amount 1000 \
  --cotejo-keypair "$KEYS/cotejo-e2e.json" --sealed-hash "$SEALED" | tee "$WORK/draw0.log"
grep -q "cotejo       $COTEJO; sealed hash $SEALED" "$WORK/draw0.log" || { echo "error: the pagaré does not record the cotejo and the sealed hash" >&2; exit 1; }
for i in 1 2 3 4 5; do
  run draw.ts --pool "$POOL" --line-id 1 --borrower-keypair "$KEYS/borrower.json" --amount 1000 --cotejo-keypair "$KEYS/cotejo-e2e.json"
done

echo "== repay.ts --all: six pagaré records, so two repay parts"
# The interest is 12% a year on the principal outstanding, for the seconds the line has run
# (decision 48); the borrower holds the USDC it drew, the sponsor pays a first part.
run repay.ts --pool "$POOL" --line-id 1 --amount 20 --payer-keypair "$KEYS/sponsor.json"
run repay.ts --pool "$POOL" --line-id 1 --all | tee "$WORK/repay.log"
grep -q 'repay part 2' "$WORK/repay.log" || { echo "error: the payoff was not split" >&2; exit 1; }
grep -q 'status       Repaid' "$WORK/repay.log" || { echo "error: the line is not repaid" >&2; exit 1; }

echo "== the repay delivered its pagarés in the same transactions; the borrower burns pagaré 0 as its owner"
grep -q ", 1 delivered" "$WORK/repay.log" || grep -q "delivered" "$WORK/repay.log" || { echo "error: the repay delivered nothing" >&2; exit 1; }
if run deliver-pagare.ts --pool "$POOL" --line-id 1 --draw-index 0 --burn --owner-keypair "$KEYS/sponsor.json" 2>"$WORK/burn.err"; then
  echo "error: a non-owner burnt the asset" >&2; exit 1
fi
echo "  refused as expected: a non-owner cannot burn"
run deliver-pagare.ts --pool "$POOL" --line-id 1 --draw-index 0 --burn --owner-keypair "$KEYS/borrower.json"

echo "== endorse-pagare.ts: a third line, pagaré 0 en procuración to a key made for the case, pagaré 1 endorsed and revoked"
run request-line.ts --pool "$POOL" --borrower-keypair "$KEYS/borrower.json" --principal 3000 --agreement-hash "$HASH"
run approve-line.ts --pool "$POOL" --line-id 2
run council-propose.ts --pool "$POOL" --op set-cotejo-keys --keys none
for i in 0 1 2; do
  run draw.ts --pool "$POOL" --line-id 2 --borrower-keypair "$KEYS/borrower.json" --amount 1000
done
run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 0 | tee "$WORK/endorse.log"
PROCURADOR_KEY="$(sed -n 's/^.* is in \(.*\.json\): hand it to the collection agency$/\1/p' "$WORK/endorse.log")"
[ -f "$PROCURADOR_KEY" ] || { echo "error: no procurador key was written for the case" >&2; exit 1; }
PROCURADOR="$(solana-keygen pubkey "$PROCURADOR_KEY")"
grep -q "procuración in force, procurador $PROCURADOR" "$WORK/endorse.log" || { echo "error: no procuración recorded" >&2; exit 1; }
run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 1 --procurador "$PROCURADOR" | tee "$WORK/reuse.log"
grep -q "already holds 1 pagaré(s) under procuración" "$WORK/reuse.log" || { echo "error: no warning for a reused procurador key" >&2; exit 1; }
run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 1 --revoke | tee "$WORK/revoke.log"
grep -q "procuración none" "$WORK/revoke.log" || { echo "error: the procuración was not revoked" >&2; exit 1; }
if run endorse-pagare.ts --pool "$POOL" --line-id 2 --draw-index 2 --procurador "$POOL" 2>"$WORK/endorsee.err"; then
  echo "error: the pool was accepted as procurador" >&2; exit 1
fi
echo "  refused as expected: $(cat "$WORK/endorsee.err")"

echo "== repay.ts: the sponsor pays a part (the interest accrued, then principal), the borrower the rest"
run repay.ts --pool "$POOL" --line-id 2 --amount 15 --payer-keypair "$KEYS/sponsor.json"
run repay.ts --pool "$POOL" --line-id 2 --all | tee "$WORK/repay2.log"
grep -q 'status       Repaid' "$WORK/repay2.log" || { echo "error: the third line is not repaid" >&2; exit 1; }

echo "== the repay settled the procurador's collection and delivered the títulos (decision 38)"
grep -q "delivered" "$WORK/repay2.log" || { echo "error: the third line's repay delivered nothing" >&2; exit 1; }
if run deliver-pagare.ts --pool "$POOL" --line-id 2 --draw-index 0 2>"$WORK/redeliver2.err"; then
  echo "error: a second delivery went through on the procurador's pagaré" >&2; exit 1
fi
grep -q "already delivered" "$WORK/redeliver2.err" || { echo "error: unexpected refusal:" >&2; cat "$WORK/redeliver2.err" >&2; exit 1; }
echo "  refused as expected: $(cat "$WORK/redeliver2.err")"

echo "== local e2e passed"
