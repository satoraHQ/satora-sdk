// Idiomatic C# facade over the UniFFI-generated bindings. The generated
// code lives in Generated/lendaswap_sdk_ffi.cs and exposes everything
// under the `uniffi.lendaswap_sdk_ffi` namespace; we re-expose a curated
// surface under `Lendaswap.Sdk` so consumers don't import generated
// namespaces directly and we can evolve the Rust side without leaking
// the changes into the consumer's call sites.

namespace Lendaswap.Sdk;

using Ffi = uniffi.lendaswap_sdk_ffi;

// Re-export the FFI-generated tagged enums under the friendly
// Lendaswap.Sdk namespace so callers write `ChainId.Arbitrum` instead
// of `uniffi.lendaswap_sdk_ffi.ChainId.Arbitrum`. C# type aliases at
// the file level only apply to this file, so we expose these globally
// via `global using` in the csproj.
//
// (Aliases live in Lendaswap.Sdk.csproj — see <ItemGroup><Using…/>.)

/// <summary>
/// Wraps every error surfaced by the Lendaswap SDK across the FFI
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
/// Version reported by the Lendaswap backend (<c>GET /version</c>).
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
    internal readonly Ffi.LendaswapClient _ffi;
    private readonly bool _hasMnemonic;

    /// <summary>
    /// Construct a read-only client — supports <see cref="GetVersionAsync"/>
    /// and <see cref="GetQuoteAsync"/>. Calls that require a signer (e.g.
    /// <see cref="CreateSwapAsync"/>) throw if invoked from here.
    /// </summary>
    public Client(string baseUrl)
    {
        // uniffi-bindgen-cs maps the first `#[uniffi::constructor]` to
        // a normal C# `new T(...)`; secondary constructors become static
        // factories (`NewSigning` below).
        _ffi = TryOrThrow(() => new Ffi.LendaswapClient(baseUrl));
        _hasMnemonic = false;
    }

    /// <summary>
    /// Construct a signing client. The mnemonic is needed to derive the
    /// per-swap preimage and EVM key for create / fund / claim flows.
    /// Held in memory for the lifetime of this instance — callers
    /// concerned about exposure should keep the client short-lived.
    /// </summary>
    public Client(string baseUrl, string mnemonic)
    {
        _ffi = TryOrThrow(() => Ffi.LendaswapClient.NewSigning(baseUrl, mnemonic));
        _hasMnemonic = true;
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
    /// <param name="receiveTo">Destination address tagged with its rail.</param>
    /// <param name="gasless">
    /// When <c>true</c>, the backend returns a deposit address the user
    /// funds with a plain ERC-20 transfer; the SDK then relays into the
    /// HTLC via a Permit2-signed userOp. When <c>false</c>, funding the
    /// HTLC is the caller's responsibility (out-of-band calldata fetch).
    /// </param>
    public Task<SwapDetails> CreateSwapAsync(
        ChainId sourceChain,
        TokenId sourceToken,
        ChainId targetChain,
        TokenId targetToken,
        QuoteAmount amount,
        Address receiveTo,
        bool gasless,
        CancellationToken cancellationToken = default)
    {
        if (!_hasMnemonic)
        {
            throw new InvalidOperationException(
                "CreateSwapAsync requires a mnemonic — construct the client with `new Client(baseUrl, mnemonic)`.");
        }
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(ffi.CreateSwap(
                sourceChain,
                sourceToken,
                targetChain,
                targetToken,
                amount,
                receiveTo,
                gasless))),
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
        if (!_hasMnemonic)
        {
            throw new InvalidOperationException(
                "FundSwapAsync requires a mnemonic — construct the client with `new Client(baseUrl, mnemonic)`.");
        }
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => FundReceipt.FromFfi(ffi.FundSwapGasless(swapId, opts))),
            cancellationToken);
    }

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
    /// <param name="config">
    /// Arkade-side configuration. The mnemonic here is the user's Arkade
    /// identity (BIP-85 derivation) — distinct from the lendaswap signing
    /// mnemonic the client was constructed with. They MUST match the
    /// mnemonic used to derive the receive address passed to
    /// <see cref="CreateSwapAsync"/>, otherwise the VHTLC's receiver key
    /// won't match the claim signer.
    /// </param>
    public Task<ClaimReceipt> ClaimAsync(
        string swapId,
        string destination,
        ArkadeConfig config,
        CancellationToken cancellationToken = default)
    {
        if (!_hasMnemonic)
        {
            throw new InvalidOperationException(
                "ClaimAsync requires a mnemonic — construct the client with `new Client(baseUrl, mnemonic)`.");
        }
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ClaimReceipt.FromFfi(ffi.Claim(swapId, destination, config))),
            cancellationToken);
    }
}
