# @satora/swap

The Satora swap client — a Lightning / on-chain BTC / Arkade ↔ EVM on/off-ramp.
Pure TypeScript, works in every JS environment (browser, Node, React Native).

The package includes the underlying swap implementation and its types.
`Client` and `ClientBuilder` retain the legacy API, alongside Satora-native
features. No separate `@lendasat/lendaswap-sdk-pure` installation or release is
required. The `/node`, `/delegate`, and `/cctp-bridge` entry points are included;
SQLite remains an optional native dependency and CCTP uses optional Circle peers.

## Install

```sh
npm install @satora/swap
```

## Compatibility headers

API requests include `X-Lendaswap-Client: <sdk-name>/<semver>` and one `x-satora-<component>-version: <epoch>` header per protocol component (`bitcoin-htlc`, `arkade-vhtlc`, `lightning`, `evm-erc20-htlc`, `evm-native-htlc`). The server rejects a client whose epoch for a component the swap uses is too old.

## Supported swaps

| Direction | Source                      | Target                      |
| --------- | --------------------------- | --------------------------- |
| BTC → EVM | Lightning, Arkade, on-chain | Ethereum, Polygon, Arbitrum |
| EVM → BTC | Ethereum, Polygon, Arbitrum | Lightning, Arkade, on-chain |

Refunds: Lightning auto-expires; Arkade refunds off-chain; on-chain BTC and EVM
refund after their timelock.

## Quick start

### Build a client

```ts
import { Client, IdbWalletStorage, IdbSwapStorage } from "@satora/swap";

// Browser, persistent storage (IndexedDB). Omit the storages for a fresh
// ephemeral wallet, or pass .withMnemonic("abandon abandon …") to import one.
const client = await Client.builder()
  .withSignerStorage(new IdbWalletStorage())
  .withSwapStorage(new IdbSwapStorage())
  .build();

const mnemonic = client.getMnemonic(); // back this up
```

### Quote and create a swap

`createSwap` handles every direction; the SDK routes by source/target asset.

```ts
import { Asset } from "@satora/swap";

const quote = await client.getQuote({
  sourceChain: Asset.BTC_ARKADE.chain,
  sourceToken: Asset.BTC_ARKADE.tokenId,
  targetChain: Asset.USDC_POLYGON.chain,
  targetToken: Asset.USDC_POLYGON.tokenId,
  sourceAmount: 100_000, // sats
});

// BTC → EVM: Arkade BTC to USDC on Polygon
const {response} = await client.createSwap({
  source: Asset.BTC_ARKADE,
  target: Asset.USDC_POLYGON,
  sourceAmount: 100_000,       // sats
  targetAddress: "0x…",        // your EVM address
});
const swapId = response.id;
```

### Fund EVM-sourced swaps

EVM-sourced swaps are funded after creation. The SDK runs the full Permit2 flow
(allowance, ERC-20 approval, EIP-712 signing, submission) in one call — you
provide an `EvmSigner` (a small wallet abstraction; adapters for viem/wagmi and
ethers v6 are a few lines — see the underlying SDK's README).

```ts
const {txHash} = await client.fundSwap(swapId, signer);
```

### Monitor, claim, refund

```ts
const swap = await client.getSwap(swapId);
// pending → clientfunded → serverfunded → clientredeemed → serverredeemed

// BTC → EVM: once serverfunded, claim your tokens
await client.claim(swapId);

// Refund after timeout
await client.refundSwap(swapId, {destinationAddress: "bc1q…"}); // or "ark1…"
await client.collabRefundEvmWithSigner(swapId, signer); // EVM, instant/gasless
```

## License

MIT
