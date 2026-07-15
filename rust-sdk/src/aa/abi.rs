//! Solidity ABI definitions for the contracts the gasless funding flow
//! encodes calls to.
//!
//! Plain `sol!` (no `#[sol(rpc)]`) — this phase only needs the generated
//! `*Call` types for ABI encoding. The provider-bound contract instances
//! (e.g. for the `getNonce` view call) come in the bundler phase.

use alloy::sol;

sol! {
    // ─────────────────────────────────────────────────────────────────
    // Shared structs
    // ─────────────────────────────────────────────────────────────────

    /// A single call in a batch. Doubles as the Kernel `Execution` tuple
    /// `(address target, uint256 value, bytes callData)` — the layout
    /// Kernel V3.3's `execute` expects inside `abi.encode(Execution[])`.
    #[derive(Debug, PartialEq, Eq)]
    struct Call {
        address target;
        uint256 value;
        bytes callData;
    }

    /// Permit2 `TokenPermissions` — the (token, amount) the depositor
    /// authorises Permit2 to pull.
    #[derive(Debug, PartialEq, Eq)]
    struct TokenPermissions {
        address token;
        uint256 amount;
    }

    /// Permit2 `PermitTransferFrom` — the permit payload passed to the
    /// HTLC coordinator alongside the depositor's witness signature.
    #[derive(Debug, PartialEq, Eq)]
    struct PermitTransferFrom {
        TokenPermissions permitted;
        uint256 nonce;
        uint256 deadline;
    }

    // ─────────────────────────────────────────────────────────────────
    // EntryPoint v0.7 — UserOp nonce lookup
    // ─────────────────────────────────────────────────────────────────

    /// Canonical EntryPoint v0.7. `getNonce` reads the next nonce for a
    /// `(sender, key)` pair; for the Kernel 7702 root validator the
    /// 192-bit key is all-zero, so callers pass `key = 0`.
    interface IEntryPoint {
        function getNonce(address sender, uint192 key) external view returns (uint256 nonce);
    }

    // ─────────────────────────────────────────────────────────────────
    // Kernel V3.3 — the delegated account's batch executor
    // ─────────────────────────────────────────────────────────────────

    /// Kernel V3.3 (the EIP-7702 delegation target). `execute` runs a
    /// batch when `execMode` selects CALLTYPE_BATCH; `executionCalldata`
    /// is then `abi.encode(Call[])`.
    interface IKernel {
        function execute(bytes32 execMode, bytes executionCalldata) external;
    }

    // ─────────────────────────────────────────────────────────────────
    // ERC-20 — token approval (USDC → Permit2)
    // ─────────────────────────────────────────────────────────────────

    interface IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
    }

    // ─────────────────────────────────────────────────────────────────
    // HTLC coordinator — the swap-and-lock entrypoint
    // ─────────────────────────────────────────────────────────────────

    /// The coordinator's atomic "run the DEX swap, then lock into the
    /// HTLC" call. `permit` + `signature` are the Permit2 authorisation
    /// the depositor (smart account) signs; `calls` is the inner DEX
    /// forward-call batch the backend supplies.
    interface IHTLCCoordinator {
        function executeAndCreateWithPermit2(
            Call[] calls,
            bytes32 preimageHash,
            address token,
            address claimAddress,
            uint256 timelock,
            address depositor,
            PermitTransferFrom permit,
            bytes signature
        ) external;
    }

    // ─────────────────────────────────────────────────────────────────
    // CCTP MessageTransmitter — CCTP-inbound path only
    // ─────────────────────────────────────────────────────────────────

    /// Mints USDC on the settlement chain from a source-chain burn.
    /// Only the first call of a CCTP-inbound batch; unused for in-chain
    /// USDC funding.
    interface IMessageTransmitter {
        function receiveMessage(bytes message, bytes attestation) external returns (bool);
    }
}
