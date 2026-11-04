/**
 * USDT0 bridge constants.
 *
 * USDT0 uses LayerZero's OFT (Omnichain Fungible Token) standard for
 * cross-chain transfers. On L2s the token contract IS the OFT — it has
 * send() and quoteSend() built in. Transfers burn on source, mint on
 * destination via LayerZero messaging.
 *
 * See: https://docs.usdt0.to/
 */

// ============================================================================
// LayerZero V2 Endpoint IDs (EIDs)
// See: https://docs.layerzero.network/v2/developers/evm/technical-reference/deployed-contracts
// ============================================================================

/**
 * LayerZero V2 Endpoint IDs for all chains with USDT0 deployments.
 * See: https://docs.usdt0.to/technical-documentation/deployments
 */
export const LZ_EIDS = {
  Ethereum: 30101,
  Polygon: 30109,
  Arbitrum: 30110,
  Optimism: 30111,
  Mantle: 30181,
  "Conflux eSpace": 30212,
  XLayer: 30274,
  Sei: 30280,
  Flare: 30295,
  Hedera: 30316,
  Unichain: 30320,
  Morph: 30322,
  Corn: 30331,
  Rootstock: 30333,
  Ink: 30339,
  Berachain: 30362,
  HyperEVM: 30367,
  Plasma: 30383,
  Monad: 30390,
  Stable: 30396,
  MegaETH: 30398,
  Tempo: 30410,
  Tron: 30420,
} as const;

export type Usdt0ChainName = keyof typeof LZ_EIDS;

// ============================================================================
// USDT0 token addresses per chain
// See: https://docs.usdt0.to/technical-documentation/deployments
// ============================================================================

export const USDT0_ADDRESSES: Record<string, string> = {
  Tron: "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
  Ethereum: "0xdAC17F958D2ee523a2206206994597C13D831ec7",
  Arbitrum: "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
  Polygon: "0xc2132D05D31c914a87C6611C10748AEb04B58e8F",
  Optimism: "0x01bFF41798a0BcF287b996046Ca68b395DbC1071",
  Berachain: "0x779Ded0c9e1022225f8E0630b35a9b54bE713736",
  "Conflux eSpace": "0xaf37E8B6C9ED7f6318979f56Fc287d76c30847ff",
  Corn: "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb",
  Flare: "0xe7cd86e13AC4309349F30B3435a9d337750fC82D",
  HyperEVM: "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb",
  Hedera: "0x00000000000000000000000000000000009Ce723",
  Ink: "0x0200C29006150606B650577BBE7B6248F58470c1",
  Mantle: "0x779Ded0c9e1022225f8E0630b35a9b54bE713736",
  MegaETH: "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb",
  Monad: "0xe7cd86e13AC4309349F30B3435a9d337750fC82D",
  Morph: "0xe7cd86e13AC4309349F30B3435a9d337750fC82D",
  Plasma: "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb",
  Rootstock: "0x779dED0C9e1022225F8e0630b35A9B54Be713736",
  Sei: "0x9151434b16b9763660705744891fA906F660EcC5",
  Stable: "0x779Ded0c9e1022225f8E0630b35a9b54bE713736",
  Tempo: "0x20C00000000000000000000014f22CA97301EB73",
  Unichain: "0x9151434b16b9763660705744891fA906F660EcC5",
  XLayer: "0x779Ded0c9e1022225f8E0630b35a9b54bE713736",
};
