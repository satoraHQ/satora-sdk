//! Account abstraction: gasless EVM funding for swaps.
//!
//! The depositor's per-swap EVM key (derived by [`crate::Signer`]) is an
//! ephemeral EOA. Under EIP-7702 that EOA delegates its code to a ZeroDev
//! Kernel V3.3 implementation, becoming an ERC-4337 (EntryPoint v0.7)
//! smart account *at its own address* — no CREATE2, no counterfactual
//! address. A bundler relays the UserOperation and a paymaster sponsors
//! gas, so the user never needs a native gas token.
//!
//! Layout:
//! - [`abi`]    — `sol!` ABI blocks for the contracts we encode calls to.
//! - [`userop`] — `PackedUserOperation` assembly + EntryPoint v0.7 `getUserOpHash`.
//!
//! Later phases add: `permit2` (Permit2 witness EIP-712 signing),
//! `kernel` (Kernel V3.3 `execute` batch encoding + signature envelopes),
//! `bundler` (bundler + paymaster RPC, 7702-auth attachment).
//!
//! The verified encoding details this module relies on are documented
//! inline in each submodule; the source spike checked them against the
//! ZeroDev Kernel `v3.3` git tag.

pub mod abi;
pub mod bundler;
pub mod client_ext;
pub mod kernel;
pub mod orchestrate;
pub mod paymaster;
pub mod permit2;
pub mod signing;
pub mod userop;

pub use client_ext::AaConfig;
pub use client_ext::PaymasterConfig;
pub use orchestrate::FundSwapReceipt;
