// Idiomatic C# facade over the UniFFI-generated bindings. The generated
// code lives in Generated/satora_sdk_ffi.cs and exposes everything
// under the `uniffi.satora_sdk_ffi` namespace; we re-expose a curated
// surface under `Satora.Sdk` so consumers don't import generated
// namespaces directly and we can evolve the Rust side without leaking
// the changes into the consumer's call sites.

namespace Satora.Sdk;

using Ffi = uniffi.satora_sdk_ffi;

// Re-export the FFI-generated tagged enums under the friendly
// Satora.Sdk namespace so callers write `ChainId.Arbitrum` instead
// of `uniffi.satora_sdk_ffi.ChainId.Arbitrum`. C# type aliases at
// the file level only apply to this file, so we expose these globally
// via `global using` in the csproj.
//
// (Aliases live in Satora.Sdk.csproj — see <ItemGroup><Using…/>.)

/// <summary>
/// Wraps every error surfaced by the Satora SDK across the FFI
/// boundary. uniffi-bindgen-cs marks its own <c>SdkException</c> base
/// class as <c>internal</c> (only the concrete variant subclasses are
/// public), so this public type lets callers catch SDK errors without
/// importing the generated namespace.
/// </summary>
public sealed class SdkException : Exception
{
    public SdkException(string message) : base(message) { }
    public SdkException(string message, Exception inner) : base(message, inner) { }
}

/// <summary>
/// Version reported by the Satora backend (<c>GET /version</c>).
/// </summary>
public sealed record Version(string Tag, string CommitHash)
{
    internal static Version FromFfi(Ffi.Version v) => new(v.@tag, v.@commitHash);
}

/// <summary>
/// Quote for a swap. Amount fields are decimal strings to preserve full
/// precision for large EVM token amounts; parse with
/// <see cref="System.Numerics.BigInteger"/> if you need to do math on
/// them.
/// </summary>
/// <param name="ExchangeRate">Rate as a decimal-as-string ("how much target per BTC").</param>
/// <param name="NetworkFee">Server-paid gas + on-chain mining fees (satoshis).</param>
/// <param name="GaslessNetworkFee">Extra gas the server pays for the gasless relay leg (satoshis).</param>
/// <param name="ProtocolFee">Protocol fee in satoshis.</param>
/// <param name="ProtocolFeeRate">Protocol fee rate (0.0025 = 0.25%).</param>
/// <param name="MinAmount">Minimum swap BTC value in satoshis.</param>
/// <param name="MaxAmount">Maximum swap BTC value in satoshis.</param>
/// <param name="SourceAmount">Pre-fee source amount in smallest source-token units.</param>
/// <param name="TargetAmount">Pre-fee target amount in smallest target-token units.</param>
/// <param name="NetSourceAmount">Final amount the user sends (incl. fees on source side).</param>
/// <param name="NetTargetAmount">Final amount the user receives (after fees on target side).</param>
/// <param name="BridgeFee">CCTP forwarding fee when a bridge was requested; null otherwise.</param>
public sealed record Quote(
    string ExchangeRate,
    ulong NetworkFee,
    ulong GaslessNetworkFee,
    ulong ProtocolFee,
    double ProtocolFeeRate,
    ulong MinAmount,
    ulong MaxAmount,
    string SourceAmount,
    string TargetAmount,
    string NetSourceAmount,
    string NetTargetAmount,
    ulong? BridgeFee)
{
    internal static Quote FromFfi(Ffi.QuoteResult q) => new(
        q.@exchangeRate,
        q.@networkFee,
        q.@gaslessNetworkFee,
        q.@protocolFee,
        q.@protocolFeeRate,
        q.@minAmount,
        q.@maxAmount,
        q.@sourceAmount,
        q.@targetAmount,
        q.@netSourceAmount,
        q.@netTargetAmount,
        q.@bridgeFee);
}


/// <summary>
/// Compact, user-facing view of a created swap. Mirrors
/// <c>lendaswap_sdk::Swap</c> — amount fields stay as strings to
/// preserve precision for large EVM token amounts.
/// </summary>
/// <param name="Id">Swap UUID. Persist this to drive funding / claim.</param>
/// <param name="Status">Current backend state; see <see cref="SwapStatus"/>.</param>
/// <param name="Funding">Funding instructions; depends on whether the swap was created with gasless on/off.</param>
/// <param name="DepositAmount">Amount the user must deposit, in the smallest unit of <paramref name="DepositToken"/>.</param>
/// <param name="DepositToken">Source token the user pays in.</param>
/// <param name="ReceiveAddress">Where the user receives the target asset.</param>
/// <param name="ReceiveAmount">Amount the user will receive, in the smallest unit of <paramref name="ReceiveToken"/>.</param>
/// <param name="ReceiveToken">Target token the user receives.</param>
public sealed record SwapDetails(
    string Id,
    SwapStatus Status,
    SwapFunding Funding,
    string DepositAmount,
    TokenId DepositToken,
    string ReceiveAddress,
    string ReceiveAmount,
    TokenId ReceiveToken)
{
    internal static SwapDetails FromFfi(Ffi.Swap s) => new(
        s.@id,
        s.@status,
        s.@funding,
        s.@depositAmount,
        s.@depositToken,
        s.@receiveAddress,
        s.@receiveAmount,
        s.@receiveToken);
}

/// <summary>
/// Result of submitting the gasless ERC-4337 funding userOp.
/// </summary>
/// <param name="UserOpHash">The bundler-computed userOpHash (32-byte hex, `0x…`).</param>
/// <param name="TransactionHash">
/// On-chain tx hash if the SDK's bounded receipt poll caught it; null
/// if the poll exhausted (caller can re-poll the bundler directly).
/// </param>
public sealed record FundReceipt(string UserOpHash, string? TransactionHash)
{
    internal static FundReceipt FromFfi(FundSwapReceiptRaw r) => new(r.@userOpHash, r.@transactionHash);
}

/// <summary>
/// Result of an Arkade VHTLC claim.
/// </summary>
/// <param name="ArkTxid">Ark TX ID of the offchain claim transaction (hex, `0x…`).</param>
/// <param name="ClaimAmountSats">Amount swept out of the VHTLC, in satoshis.</param>
public sealed record ClaimReceipt(string ArkTxid, ulong ClaimAmountSats)
{
    internal static ClaimReceipt FromFfi(ClaimReceiptRaw r) => new(r.@arkTxid, r.@claimAmountSats);
}

/// <summary>
/// Top-level client. Holds an FFI handle whose Rust side owns the
/// per-swap key_index storage that <see cref="CreateSwapAsync"/> writes
/// to and the funding / claim flows read from. Dispose this when done
/// — the underlying Rust resources are released by the handle's
/// <c>IDisposable</c>.
/// </summary>
public sealed class Client : IDisposable
{
    /// <summary>
    /// Underlying FFI handle. Internal so the generated namespace
    /// doesn't leak into the public surface, but exposed via interface
    /// for tests / extension code that wants to drop down.
    /// </summary>
    internal readonly Ffi.SatoraClient _ffi;

    /// <summary>
    /// Construct a client. Mnemonic is required (drives EVM signing
    /// and the Arkade identity — they're always derived from the same
    /// seed, consumers don't get to mismatch them). Every other knob
    /// has a sensible default keyed off <paramref name="network"/>;
    /// pass overrides only for dev/test setups.
    /// </summary>
    /// <param name="mnemonic">BIP-39 signing mnemonic. Also used as the Arkade identity.</param>
    /// <param name="network">Target Bitcoin network. Selects the default URL set.</param>
    /// <param name="baseUrl">Override the Satora backend base URL.</param>
    /// <param name="arkadeServerUrl">Override the Arkade arkd gRPC endpoint.</param>
    /// <param name="esploraUrl">Override the esplora HTTP endpoint.</param>
    /// <param name="referralCode">
    /// Optional referral code attached to every swap/quote originated
    /// through this client. Set it once here instead of repeating it
    /// per-call. Empty string is treated as null.
    /// </param>
    public Client(
        string mnemonic,
        BitcoinNetwork network = BitcoinNetwork.Mainnet,
        string? baseUrl = null,
        string? arkadeServerUrl = null,
        string? esploraUrl = null,
        string? referralCode = null)
    {
        var defaults = Defaults.For(network);
        var arkadeConfig = new Ffi.ArkadeConfig(
            arkadeServerUrl ?? defaults.ArkadeServerUrl,
            esploraUrl ?? defaults.EsploraUrl,
            mnemonic,
            network);
        _ffi = TryOrThrow(() => Ffi.SatoraClient.NewWithArkade(
            baseUrl ?? defaults.BaseUrl, mnemonic, arkadeConfig, referralCode));
    }

    /// <summary>Releases the FFI handle. Idempotent.</summary>
    public void Dispose() => _ffi.Dispose();

    /// <summary>
    /// Shared helper: invoke an FFI call and re-wrap its tagged-enum
    /// error as the public <see cref="SdkException"/>. Used by both the
    /// constructor (which can fail on invalid base URL) and the method
    /// bodies below.
    /// </summary>
    private static T TryOrThrow<T>(Func<T> call)
    {
        try { return call(); }
        catch (Ffi.SdkException.Internal ex) { throw new SdkException(ex.Message, ex); }
    }

    /// <summary>
    /// Fetch the deployed backend's version and commit hash.
    /// </summary>
    /// <remarks>
    /// The underlying FFI call is synchronous — it blocks on a tokio
    /// runtime inside the Rust library. We surface it as async by
    /// dispatching to the thread pool so callers in async contexts
    /// (e.g. ASP.NET request handlers) don't block their request
    /// threads.
    /// </remarks>
    public Task<Version> GetVersionAsync(CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(() => TryOrThrow(() => Version.FromFfi(ffi.Version())), cancellationToken);
    }

    /// <summary>
    /// Fetch a swap quote. The chain / token / amount enums encode all
    /// the wire-format constraints in their types — no string coercion
    /// or mutually-exclusive-fields gymnastics on the caller side.
    /// </summary>
    /// <param name="sourceChain">e.g. <c>new ChainId.Arbitrum()</c> or <c>new ChainId.Other("8453")</c>.</param>
    /// <param name="sourceToken">e.g. <c>new TokenId.Usdt0Arbitrum()</c> or <c>new TokenId.Other("0x…")</c>.</param>
    /// <param name="targetChain">Target chain enum.</param>
    /// <param name="targetToken">Target token enum.</param>
    /// <param name="amount">Source / target amount mutex (<c>new QuoteAmount.Source(100_000_000)</c>).</param>
    public Task<Quote> GetQuoteAsync(
        ChainId sourceChain,
        TokenId sourceToken,
        ChainId targetChain,
        TokenId targetToken,
        QuoteAmount amount,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => Quote.FromFfi(ffi.Quote(
                sourceChain,
                sourceToken,
                targetChain,
                targetToken,
                amount))),
            cancellationToken);
    }

    /// <summary>
    /// Create a swap. Today the SDK only routes EVM stablecoin → BTC on
    /// Arkade; other direction combos return an <see cref="SdkException"/>.
    /// Requires the client to be constructed with a mnemonic (see
    /// <see cref="Client(string, string)"/>) — the signer derives the
    /// per-swap preimage and EVM EOA from it.
    /// </summary>
    /// <param name="sourceChain">Source chain enum.</param>
    /// <param name="sourceToken">Source token enum.</param>
    /// <param name="targetChain">Target chain enum.</param>
    /// <param name="targetToken">Target token enum.</param>
    /// <param name="amount">Source / target amount mutex.</param>
    /// <param name="receiveTo">
    /// Destination address tagged with its rail. Pass <c>null</c> to
    /// route to the SDK's own internal Arkade wallet — only valid when
    /// the target token is BTC on Arkade.
    /// </param>
    /// <param name="gasless">
    /// When <c>true</c>, the backend returns a deposit address the user
    /// funds with a plain ERC-20 transfer; the SDK then relays into the
    /// HTLC via a Permit2-signed userOp. When <c>false</c>, funding the
    /// HTLC is the caller's responsibility (out-of-band calldata fetch).
    /// </param>
    /// <param name="extraFeesBps">
    /// Optional per-swap fee surcharge in basis points, bounded by the
    /// <c>max_extra_fee_bps</c> cap on the dev key matched by the
    /// client's referral code. Pass <c>null</c> to fall back to the
    /// key's configured default.
    /// </param>
    public Task<SwapDetails> CreateSwapAsync(
        ChainId sourceChain,
        TokenId sourceToken,
        ChainId targetChain,
        TokenId targetToken,
        QuoteAmount amount,
        Address? receiveTo,
        bool gasless,
        ushort? extraFeesBps = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(ffi.CreateSwap(
                sourceChain,
                sourceToken,
                targetChain,
                targetToken,
                amount,
                receiveTo,
                gasless,
                extraFeesBps))),
            cancellationToken);
    }

    /// <summary>
    /// Offchain VTXO balance of the SDK's internal Arkade wallet,
    /// broken into the three buckets ark-client distinguishes — see
    /// <see cref="ArkadeBalance"/>. Hits the Arkade server's gRPC
    /// indexer.
    /// </summary>
    /// <remarks>
    /// For "what can I send right now?" use
    /// <c>balance.confirmedSats</c>. <c>total_sats()</c>
    /// (confirmed + pre-confirmed + recoverable) over-reports
    /// spendable funds.
    /// </remarks>
    public Task<ArkadeBalance> GetArkadeBalanceAsync(CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(() => TryOrThrow(() => ffi.ArkadeBalance()), cancellationToken);
    }

    /// <summary>
    /// Roll over all spendable VTXOs + boarding outputs into the next
    /// Arkade batch — what users do before their VTXOs expire. Returns
    /// the hex commitment txid of the batch that absorbed them, or
    /// <c>null</c> if the wallet had nothing to settle.
    /// </summary>
    public Task<string?> SettleArkadeAsync(CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(() => TryOrThrow(() => ffi.ArkadeSettle()), cancellationToken);
    }

    /// <summary>
    /// Derive the SDK's internal Arkade wallet address. The same
    /// identity mnemonic always produces the same address (BIP-85
    /// derivation), so this is the destination for funds from an
    /// address-less <see cref="CreateSwapAsync"/>.
    /// </summary>
    public Task<string> GetArkadeAddressAsync(CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(() => TryOrThrow(() => ffi.ArkadeOffchainAddress()), cancellationToken);
    }

    /// <summary>
    /// Send <paramref name="amountSats"/> from the SDK's internal
    /// Arkade wallet to <paramref name="destination"/> (any
    /// <c>tark1q…</c> Arkade address) via an offchain Ark transaction.
    /// Returns the Ark txid as <c>0x…</c> hex.
    ///
    /// Primary use case: funding the Arkade VHTLC returned by
    /// <see cref="CreateArkadeToLightningSwapAsync"/>.
    /// </summary>
    public Task<string> SendArkadeAsync(
        string destination,
        ulong amountSats,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ffi.ArkadeSend(destination, amountSats)),
            cancellationToken);
    }

    /// <summary>
    /// On-chain Bitcoin boarding address for the SDK's internal
    /// Arkade wallet. Funding flow: send L1 BTC to the returned
    /// address, mine a confirmation, then call
    /// <see cref="SettleArkadeAsync"/> to promote the boarding output
    /// into a confirmed VTXO. The address is deterministic per wallet
    /// identity, so safe to display once and reuse.
    /// </summary>
    public Task<string> GetArkadeBoardingAddressAsync(
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ffi.ArkadeBoardingAddress()),
            cancellationToken);
    }

    /// <summary>
    /// Create an Arkade → Lightning swap. The user funds the returned
    /// Arkade VHTLC address (in <c>swap.Funding</c> as
    /// <c>SwapFunding.ArkadeAddress</c>); the server pays the
    /// Lightning destination via Boltz and claims the VHTLC with the
    /// resulting preimage. <b>No client-side claim path</b> —
    /// <see cref="ClaimAsync"/> doesn't apply for this direction.
    /// </summary>
    /// <param name="destination">
    /// Lightning destination. Use <c>LightningDestination.Invoice</c>
    /// for a BOLT11 (amount embedded), or
    /// <c>LightningDestination.Address</c> /
    /// <c>LightningDestination.Lnurl</c> with an explicit <c>sats</c>
    /// — the server resolves LNURL-pay itself, no client-side LNURL
    /// resolver needed.
    /// </param>
    public Task<SwapDetails> CreateArkadeToLightningSwapAsync(
        LightningDestination destination,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(ffi.CreateArkadeToLightningSwap(destination))),
            cancellationToken);
    }

    /// <summary>
    /// Poll until the gasless deposit address has received enough
    /// source token AND enough native gas. Idiomatic when integrating
    /// against a real user wallet: print the address + required
    /// amounts to the user, then await this to know when funding has
    /// arrived and you can submit the gasless userOp.
    /// </summary>
    /// <param name="swapId">UUID returned by <see cref="CreateSwapAsync"/>.</param>
    /// <param name="nodeRpcUrl">EVM node RPC URL (e.g. Alchemy / Infura).</param>
    /// <param name="minEthWei">Native gas headroom in wei. ~1e15 (0.001 ETH) on Arbitrum without a paymaster; 0 with.</param>
    /// <param name="timeout">Total wait budget. Throws <see cref="SdkException"/> on timeout.</param>
    public Task WaitForDepositFundingAsync(
        string swapId,
        string nodeRpcUrl,
        ulong minEthWei,
        TimeSpan timeout,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        var seconds = (ulong)Math.Ceiling(timeout.TotalSeconds);
        return Task.Run(
            () => TryOrThrow(() =>
            {
                ffi.WaitForDepositFunding(swapId, nodeRpcUrl, minEthWei, seconds);
                return 0; // TryOrThrow wants a T; the void method returns nothing.
            }),
            cancellationToken);
    }

    /// <summary>
    /// Submit the gasless ERC-4337 + EIP-7702 funding userOp for a
    /// previously-created swap. Requires the client to have been built
    /// with a mnemonic (the SDK re-derives the per-swap signing key
    /// from it) AND the depositor EOA must already hold the source
    /// token at the time of submission. Bundler + paymaster URLs come
    /// from the backend's `/aa/config` endpoint — the caller only
    /// supplies the bits the server can't ship.
    /// </summary>
    /// <param name="swapId">UUID returned by <see cref="CreateSwapAsync"/>.</param>
    /// <param name="opts">Node-RPC URL + optional paymaster context / gas overrides.</param>
    public Task<FundReceipt> FundSwapAsync(
        string swapId,
        GaslessOpts opts,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => FundReceipt.FromFfi(ffi.FundSwapGasless(swapId, opts))),
            cancellationToken);
    }

    /// <summary>
    /// Convenience overload: submits the gasless funding userOp with
    /// all-default options. The Rust core picks the node RPC URL from
    /// the swap's deposit chain (public RPC per chain — fine for
    /// low-volume use). Use the
    /// <see cref="FundSwapAsync(string, GaslessOpts, CancellationToken)"/>
    /// overload when you need a custom RPC provider, paymaster context,
    /// or gas overrides.
    /// </summary>
    public Task<FundReceipt> FundSwapAsync(
        string swapId,
        CancellationToken cancellationToken = default)
        => FundSwapAsync(
            swapId,
            new GaslessOpts(nodeRpcUrl: null, paymasterContextJson: null, gasOverrides: null),
            cancellationToken);

    /// <summary>
    /// Fetch a swap's current state. Works on any client (signing or
    /// read-only). Returns the same shape <see cref="CreateSwapAsync"/>
    /// does, so callers can re-read after the backend transitions
    /// states (e.g. ServerFunded).
    /// </summary>
    public Task<SwapDetails> GetSwapAsync(string swapId, CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(ffi.GetSwap(swapId))),
            cancellationToken);
    }

    /// <summary>
    /// Poll <c>GET /swap/{id}</c> until the backend reaches one of
    /// <paramref name="targets"/> or <paramref name="timeout"/> elapses
    /// (in which case the SDK throws <see cref="SdkException"/> with
    /// its internal `Error::Timeout` message). 3s poll interval is
    /// hard-coded on the Rust side.
    /// </summary>
    /// <param name="swapId">UUID of a previously-created swap.</param>
    /// <param name="targets">Accept-states. A typical post-funding wait is
    /// <c>[SwapStatus.ServerFunded, SwapStatus.ClientRedeemed, SwapStatus.ServerRedeemed]</c>
    /// — three accept-states cover the case where the swap raced past
    /// ServerFunded between polls.</param>
    /// <param name="timeout">Total wait budget. The SDK accepts seconds; we round up.</param>
    public Task<SwapStatus> WaitForSwapStatusAsync(
        string swapId,
        IEnumerable<SwapStatus> targets,
        TimeSpan timeout,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        // The generated Vec<SwapStatus> binding takes an array, not a List.
        var targetArray = targets.ToArray();
        // Round up so callers asking for 30.5s don't get cut to 30s.
        var seconds = (ulong)Math.Ceiling(timeout.TotalSeconds);
        return Task.Run(
            () => TryOrThrow(() => ffi.WaitForSwapStatus(swapId, targetArray, seconds)),
            cancellationToken);
    }

    /// <summary>
    /// Redeem the Arkade VHTLC for an EVM→Arkade swap that has reached
    /// (or passed) ServerFunded. Sweeps the BTC to <paramref name="destination"/>.
    /// </summary>
    /// <param name="swapId">UUID of a swap whose backend state is at least <see cref="SwapStatus.ServerFunded"/>.</param>
    /// <param name="destination">Arkade address (`tark1…`) to receive the BTC.</param>
    public Task<ClaimReceipt> ClaimAsync(
        string swapId,
        string destination,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ClaimReceipt.FromFfi(ffi.Claim(swapId, destination))),
            cancellationToken);
    }
}

/// <summary>
/// Network-keyed default endpoints for the <see cref="Client"/>
/// constructor. Internal so the surface stays one-knob; consumers
/// that need a non-default URL pass it explicitly.
/// </summary>
internal static class Defaults
{
    internal readonly record struct Endpoints(string BaseUrl, string ArkadeServerUrl, string EsploraUrl);

    internal static Endpoints For(BitcoinNetwork network) => network switch
    {
        BitcoinNetwork.Mainnet => new(
            "https://api.satora.io",
            "https://arkade.computer",
            "https://mempool.space/api"),
        // Testnet and Signet both route to mutinynet — vanilla Bitcoin
        // testnet isn't supported by our Arkade infrastructure, and
        // "testnet" in user-facing language really means "a working
        // test environment" (= mutinynet, which is technically a Signet).
        BitcoinNetwork.Testnet or BitcoinNetwork.Signet => new(
            "https://mutinynetswap.lendasat.com",
            "https://mutinynet.arkade.sh",
            "https://mutinynet.com/api"),
        BitcoinNetwork.Regtest => new(
            "http://localhost:3333",
            "http://localhost:7070",
            "http://localhost:3000"),
        _ => throw new ArgumentOutOfRangeException(nameof(network), network, "Unsupported network."),
    };
}
