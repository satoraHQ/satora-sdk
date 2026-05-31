# Lendaswap Pure TypeScript SDK

A pure TypeScript SDK for interacting with the Lendaswap API. This SDK is designed to work in all JavaScript
environments, including React Native.

## Installation

```bash
npm install @lendasat/lendaswap-sdk-pure
```

## Supported Swaps

This SDK supports the following swap directions:

### BTC to EVM

| Source    | Target                      | Status    |
| --------- | --------------------------- | --------- |
| Lightning | Ethereum, Polygon, Arbitrum | Supported |
| Arkade    | Ethereum, Polygon, Arbitrum | Supported |
| On-chain  | Ethereum, Polygon, Arbitrum | Supported |

### EVM to BTC

| Source                      | Target    | Status    |
| --------------------------- | --------- | --------- |
| Polygon, Arbitrus, Ethereum | Arkade    | Supported |
| Polygon, Arbitrus, Ethereum | Lightning | Supported |
| Polygon, Arbitrus, Ethereum | On-chain  | Supported |

**Refund support:**

- Lightning swaps: Auto-expire, no refund needed
- Arkade swaps: Off-chain refund via Arkade protocol
- On-chain BTC swaps: On-chain refund transaction after timelock
- EVM swaps: On-chain refund after timelock expires

## Usage

### Setup

```typescript
import { Client, IdbWalletStorage, IdbSwapStorage } from "@lendasat/lendaswap-sdk-pure";

// Create a client with persistent storage (browser)
const client = await Client.builder()
  .withSignerStorage(new IdbWalletStorage())
  .withSwapStorage(new IdbSwapStorage())
  .build();

// Or import an existing wallet
const client = await Client.builder()
  .withSignerStorage(new IdbWalletStorage())
  .withSwapStorage(new IdbSwapStorage())
  .withMnemonic("abandon abandon abandon ...")
  .build();

// Get the mnemonic (for backup)
const mnemonic = client.getMnemonic();
```

### API Status

```typescript
const status = await client.getStatus();
console.log(status.healthy, status.services.bitcoin.healthy);
```

### Quotes

```typescript
import { Asset } from "@lendasat/lendaswap-sdk-pure";

const quote = await client.getQuote({
  sourceChain: Asset.BTC_ARKADE.chain,      // "Arkade"
  sourceToken: Asset.BTC_ARKADE.tokenId,    // "btc"
  targetChain: Asset.USDC_POLYGON.chain,    // "137"
  targetToken: Asset.USDC_POLYGON.tokenId,  // "0x3c499c..."
  sourceAmount: 100_000,                    // sats
});
console.log(`You'll receive: ${quote.target_amount} USDC`);
```

### Create a Swap

Use `createSwap` for all swap directions. The SDK routes automatically based on the source/target asset:

```typescript
// BTC → EVM: Arkade to USDC on Polygon
const result = await client.createSwap({
  source: Asset.BTC_ARKADE,
  target: Asset.USDC_POLYGON,
  sourceAmount: 100_000,         // sats
  targetAddress: "0x...",        // your EVM address
});

// EVM → BTC: USDC on Polygon to Arkade
const result = await client.createSwap({
  source: Asset.USDC_POLYGON,
  target: Asset.BTC_ARKADE,
  sourceAmount: 10_000_000,      // 10 USDC (6 decimals)
  targetAddress: "ark1...",      // your Arkade address
  userAddress: "0x...",          // your EVM wallet address (required for EVM-sourced swaps)
});

// Or use any token by chain + contract address
const result = await client.createSwap({
  source: {chain: "42161", tokenId: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"}, // USDC on Arbitrum
  target: Asset.BTC_LIGHTNING,
  targetAddress: "lnbc...",      // BOLT11 invoice
});

const swapId = result.response.id;
```

### Fund EVM-Sourced Swaps

EVM-sourced swaps need to be funded after creation. The SDK handles the full Permit2 flow (allowance check, ERC-20
approval, EIP-712 signing, transaction submission) in a single call:

```typescript
// Fund with an external wallet
const {txHash} = await client.fundSwap(swapId, signer);

// Or fund gaslessly (SDK signs internally, server submits via relay)
const {txHash} = await client.fundSwapGasless(swapId);
```

### The EvmSigner Interface

`EvmSigner` is a minimal wallet abstraction. Implement it once for your stack — the SDK stays free of EVM library
dependencies:

```typescript
import type { EvmSigner } from "@lendasat/lendaswap-sdk-pure";
```

**wagmi / viem:**

```typescript
import { createPublicClient, http } from "viem";

const publicClient = createPublicClient({chain, transport: http()});

const signer: EvmSigner = {
  address: walletClient.account.address,
  chainId: chain.id,
  signTypedData: (td) =>
    walletClient.signTypedData({...td, account: walletClient.account}),
  sendTransaction: (tx) =>
    walletClient.sendTransaction({to: tx.to, data: tx.data, chain, gas: tx.gas}),
  waitForReceipt: (hash) =>
    publicClient.waitForTransactionReceipt({hash}).then((r) => ({
      status: r.status,
      blockNumber: r.blockNumber,
      transactionHash: r.transactionHash,
    })),
  getTransaction: (hash) =>
    publicClient.getTransaction({hash}).then((tx) => ({
      to: tx.to ?? null, input: tx.input, from: tx.from,
    })),
  call: (tx) =>
    publicClient.call({to: tx.to, data: tx.data, account: tx.from, blockNumber: tx.blockNumber})
      .then((r) => r.data ?? "0x"),
};
```

**ethers.js v6:**

```typescript
const signer: EvmSigner = {
  address: await wallet.getAddress(),
  chainId: Number((await wallet.provider.getNetwork()).chainId),
  signTypedData: (td) =>
    wallet.signTypedData(td.domain, td.types, td.message),
  sendTransaction: (tx) =>
    wallet.sendTransaction({to: tx.to, data: tx.data, gasLimit: tx.gas})
      .then((r) => r.hash),
  waitForReceipt: (hash) =>
    wallet.provider.waitForTransaction(hash).then((r) => ({
        status: r.status === 1 ? "success" : "reverted",
        blockNumber: BigInt(r.blockNumber),
        transactionHash: r.hash,
      })
    ),
  getTransaction: (hash) =>
    wallet.provider.getTransaction(hash).then((tx) => ({
      to: tx.to, input: tx.data, from: tx.from,
    })),
  call: (tx) =>
    wallet.provider.call({to: tx.to, data: tx.data, from: tx.from, blockTag: tx.blockNumber}),
};
```

### Monitor Swap Status

```typescript
const swap = await client.getSwap(swapId);
console.log(`Status: ${swap.status}`);

// Status flow: pending → clientfunded → serverfunded → clientredeemed → serverredeemed
```

### Claim

For BTC-to-EVM swaps, once the server has funded the EVM HTLC (`serverfunded`), claim your tokens:

```typescript
const result = await client.claim(swapId);
```

### Refund

If a swap times out, refund your funds:

```typescript
// BTC swaps (on-chain or Arkade)
const result = await client.refundSwap(swapId, {
  destinationAddress: "bc1q...",   // or "ark1..."
});

// EVM swaps — manual refund (after timelock expires, user pays gas)
const {txHash} = await client.refundEvmWithSigner(swapId, signer, "direct");

// EVM swaps — collaborative refund (instant, gasless, server cosigns)
const {txHash} = await client.collabRefundEvmWithSigner(swapId, signer, "swap-back");

// EVM swaps — collaborative refund for gasless swaps (SDK signs internally)
const {txHash} = await client.collabRefundEvmSwap(swapId, "swap-back");
```

Refund mode controls what token you receive:

- `"direct"` — refund as WBTC/TBTC (the HTLC lock token)
- `"swap-back"` — refund as the original source token (e.g., USDC) via DEX swap

### Storage

The SDK provides pluggable storage interfaces for persisting wallet and swap data.
Browser storage uses [Dexie](https://dexie.org/) for IndexedDB access with the database name `lendaswap-v3`.

### Auto-generated API Types

The TypeScript types in `src/generated/api.ts` are **auto-generated** from the OpenAPI specification. Do not edit this
file manually.

#### How it works

1. The Lendaswap backend generates an OpenAPI 3.0 specification (`openapi.json`)
   using [utoipa](https://github.com/juhaku/utoipa)
2. We use [openapi-typescript](https://github.com/openapi-ts/openapi-typescript) to generate TypeScript types from the
   spec
3. The [openapi-fetch](https://github.com/openapi-ts/openapi-typescript/tree/main/packages/openapi-fetch) library
   provides a type-safe HTTP client

#### Regenerating the API client

When the backend API changes:

1. Download the latest OpenAPI spec from the backend:
   ```bash
   curl -o openapi.json https://api.satora.io/api-docs/openapi.json
   ```

2. Run the generate command:
   ```bash
   npm run generate:api
   ```

This will regenerate `src/generated/api.ts` with the updated types.

### Scripts

| Command                | Description                            |
| ---------------------- | -------------------------------------- |
| `npm run build`        | Compile TypeScript to JavaScript       |
| `npm run test`         | Run tests with Vitest                  |
| `npm run lint`         | Run Biome linter                       |
| `npm run lint:fix`     | Fix linting issues                     |
| `npm run generate:api` | Regenerate API types from OpenAPI spec |

### Adding New Features

When adding new functionality:

1. **API methods**: Add convenience methods to `src/client.ts` that wrap the low-level API calls
2. **New types**: If the backend adds new endpoints, regenerate the API types first
3. **Exports**: Update `src/index.ts` to export any new public types or classes
