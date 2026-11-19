"""Run the same SDK WASM integration test against a loopback API in Node and Firefox."""

import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import sys
import threading

CRATE = Path(__file__).resolve().parents[1]
FIXTURES = json.loads((CRATE / "tests/fixtures/swap-responses.json").read_text())
HOLD = json.loads((CRATE.parent / "fixtures/lightning-invoices.json").read_text())[
    "invoices"
]["lnd_hold_index_0"]


def addr(byte):
    return "0x" + byte * 20


class Api(BaseHTTPRequestHandler):
    swaps = {}

    def reply(self, body=None, status=200):
        self.send_response(status)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        if body is not None:
            self.wfile.write(json.dumps(body).encode())

    def do_OPTIONS(self):
        self.reply(status=204)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/rpc":
            result = {
                "eth_chainId": hex(42161),
                "eth_call": "0x" + format(100_000, "064x"),
            }[req["method"]]
            self.reply({"jsonrpc": "2.0", "id": req["id"], "result": result})
            return

        if self.path == "/swap/lightning/evm":
            if req.get("hash_lock") != "0x" + HOLD["payment_hash"]:
                self.reply({"message": "unexpected hold invoice hash"}, status=400)
                return
            swap = FIXTURES["lightning_to_evm"] | {
                "hash_lock": req["hash_lock"],
                "bolt11_invoice": HOLD["bolt11"],
                "client_evm_address": req["claiming_address"],
            }
        elif self.path == "/swap/evm/lightning":
            swap = FIXTURES["evm_to_lightning"] | {
                "id": "00000000-0000-0000-0000-000000000002",
                "client_evm_address": req["user_address"],
                "evm_htlc_address": addr("66"),
                "evm_htlc_version": 7,
                "evm_coordinator_address": addr("44"),
                "server_evm_address": addr("77"),
                "wbtc_address": addr("55"),
            }
        else:
            self.reply(status=404)
            return

        self.swaps[swap["id"]] = swap
        self.reply(swap)

    def do_GET(self):
        parts = self.path.split("/")
        if len(parts) < 3 or parts[1] != "swap" or parts[2] not in self.swaps:
            self.reply(status=404)
            return
        swap = self.swaps[parts[2]]
        if parts[3:] == ["swap-and-lock-calldata-permit2"]:
            self.reply({
                "coordinator_address": swap["evm_coordinator_address"],
                "htlc_kind": swap["evm_htlc_kind"],
                "htlc_version": swap["evm_htlc_version"],
                "permit2_address": "0x000000000022D473030F116dDEE9F6B43aC78BA3",
                "source_token_address": swap["source_token"]["token_id"],
                "source_amount": swap["source_amount"],
                "lock_token_address": swap["wbtc_address"],
                "preimage_hash": swap["hash_lock"],
                "claim_address": swap["server_evm_address"],
                "timelock": swap["evm_refund_locktime"],
                "calls": [],
                # keccak256(abi.encode(empty Call[]))
                "calls_hash": "0x569e75fc77c1a856f6daaf9e69d8a9566ca34aa47f9133711ce065a571af0cfd",
            })
        elif len(parts) == 3:
            self.reply(swap)
        else:
            self.reply(status=404)


if __name__ == "__main__":
    runtimes = sys.argv[1:] or ["node", "browser"]
    if any(runtime not in ("node", "browser") for runtime in runtimes):
        raise SystemExit("usage: python3 tests/run_wasm.py [node] [browser]")
    with ThreadingHTTPServer(("127.0.0.1", 0), Api) as server:
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env = os.environ | {"SATORA_WASM_TEST_URL": f"http://127.0.0.1:{server.server_port}"}
        env.setdefault(
            "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", "wasm-bindgen-test-runner"
        )
        command = [
            "cargo", "test", "--target", "wasm32-unknown-unknown", "--test", "wasm_runtime",
            "--no-default-features", "--features", "gasless", "--locked",
        ]
        try:
            for runtime in runtimes:
                env.pop("WASM_BINDGEN_USE_BROWSER", None)
                if runtime == "browser":
                    env["WASM_BINDGEN_USE_BROWSER"] = "1"
                subprocess.run(command, cwd=CRATE, env=env, check=True)
        finally:
            server.shutdown()
