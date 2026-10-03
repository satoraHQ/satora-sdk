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
/// Which Rust SDK error an <see cref="SdkException"/> carries. Except for
/// <see cref="UnresolvedFunding"/>, it names the failure, not whether a retry
/// is safe: <see cref="Transport"/> and
/// <see cref="Timeout"/> cover both requests that never reached the server
/// and spends whose outcome is unknown, and <see cref="Storage"/> from
/// <see cref="Client.ClaimAsync"/> once arkd may hold the spend, from
/// <see cref="Client.RefundBitcoinAsync"/> after a fee bump, or from
/// <see cref="Client.FundSwapAsync(string, GaslessOpts, CancellationToken)"/>
/// at any point, leaves the outcome unknown too. Each method's remarks
/// say what to do next.
/// </summary>
public enum SdkErrorKind
{
    Cancelled,
    InvalidBaseUrl,
    Transport,
    Decode,
    /// <summary>The backend answered with an error; <see cref="SdkException.StatusCode"/> holds the HTTP status.</summary>
    Api,
    InvalidSigner,
    /// <summary>Invalid arguments, including ones the bindings could not parse.</summary>
    InvalidSwap,
    UnsupportedQuotePath,
    Timeout,
    Storage,
    /// <summary>
    /// An earlier funding of the swap may have gone out, and the SDK cannot
    /// tell. Never fund the swap by other means and do not treat it as
    /// unfunded: follow its status meanwhile, and give it up only once a
    /// check by hand shows where the funds went.
    /// </summary>
    UnresolvedFunding,
    /// <summary>
    /// A failure in the bindings themselves, a Rust panic included, or a Rust
    /// SDK error this version does not name yet; <see cref="Exception.InnerException"/>
    /// keeps the original.
    /// </summary>
    Internal,
}

/// <summary>
/// Wraps every error surfaced by the Satora SDK across the FFI
/// boundary, so callers catch one type without importing the generated
/// namespace. <see cref="Exception.Message"/> is the Rust error's own
/// text, for <see cref="SdkErrorKind.Api"/> the server's message.
/// </summary>
public sealed class SdkException : Exception
{
    public SdkException(
        SdkErrorKind kind,
        string message,
        int? statusCode = null,
        Exception? innerException = null)
        : base(message, innerException)
    {
        Kind = kind;
        StatusCode = statusCode;
    }

    public SdkErrorKind Kind { get; }

    /// <summary>HTTP status of an <see cref="SdkErrorKind.Api"/> error; null for every other kind.</summary>
    public int? StatusCode { get; }

    internal static SdkException FromFfi(Ffi.UniffiException ex) => ex switch
    {
        Ffi.SdkException.Cancelled e => new(SdkErrorKind.Cancelled, e.@message, innerException: ex),
        Ffi.SdkException.InvalidBaseUrl e => new(SdkErrorKind.InvalidBaseUrl, e.@message, innerException: ex),
        Ffi.SdkException.Transport e => new(SdkErrorKind.Transport, e.@message, innerException: ex),
        Ffi.SdkException.Decode e => new(SdkErrorKind.Decode, e.@message, innerException: ex),
        Ffi.SdkException.Api e => new(SdkErrorKind.Api, e.@message, e.@status, ex),
        Ffi.SdkException.InvalidSigner e => new(SdkErrorKind.InvalidSigner, e.@message, innerException: ex),
        Ffi.SdkException.InvalidSwap e => new(SdkErrorKind.InvalidSwap, e.@message, innerException: ex),
        Ffi.SdkException.UnsupportedQuotePath e => new(SdkErrorKind.UnsupportedQuotePath, e.@message, innerException: ex),
        Ffi.SdkException.Timeout e => new(SdkErrorKind.Timeout, e.@message, innerException: ex),
        Ffi.SdkException.Storage e => new(SdkErrorKind.Storage, e.@message, innerException: ex),
        Ffi.SdkException.UnresolvedFunding e => new(SdkErrorKind.UnresolvedFunding, e.@message, innerException: ex),
        Ffi.SdkException.Internal e => new(SdkErrorKind.Internal, e.@message, innerException: ex),
        _ => new(SdkErrorKind.Internal, ex.Message, innerException: ex),
    };
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
/// <remarks>
/// Locktimes and expiries are unix seconds, null when the direction has
/// no such leg. A locktime on the leg the client funds is when the
/// client's refund unlocks. On the leg the server funds, the server can
/// refund from then, so it is the client's deadline to claim. Bitcoin and
/// Arkade locks mature on median time past, which trails wall clock by
/// about an hour, so a refund at the locktime can still be refused;
/// <see cref="Client.ClaimAsync"/> refuses within 300 seconds of the
/// VHTLC locktime.
/// </remarks>
/// <param name="Id">Swap UUID. Persist this to drive funding / claim.</param>
/// <param name="Status">Current backend state; see <see cref="SwapStatus"/>.</param>
/// <param name="Direction">
/// Route of the swap. <c>SwapDirection.Unknown</c> is a direction of the
/// Rust SDK this version does not name yet.
/// </param>
/// <param name="Funding">
/// Funding instructions, by direction: <c>SwapFunding.Gasless</c> or
/// <c>SwapFunding.UserSubmitted</c> for EVM sources (gasless on or off),
/// <c>SwapFunding.BitcoinAddress</c> for on-chain Bitcoin,
/// <c>SwapFunding.Bolt11Invoice</c> for Lightning and
/// <c>SwapFunding.ArkadeAddress</c> for Arkade. <c>SwapFunding.Unknown</c>
/// is a variant of the Rust SDK this version does not name yet.
/// </param>
/// <param name="DepositAmount">Amount the user must deposit, in the smallest unit of <paramref name="DepositToken"/>.</param>
/// <param name="DepositToken">Source token the user pays in.</param>
/// <param name="ReceiveAddress">Where the user receives the target asset.</param>
/// <param name="ReceiveAmount">Amount the user will receive, in the smallest unit of <paramref name="ReceiveToken"/>.</param>
/// <param name="ReceiveToken">Target token the user receives.</param>
/// <param name="CreatedAt">Server creation time as sent on the wire (RFC 3339).</param>
/// <param name="VhtlcRefundLocktime">
/// Arkade VHTLC refund locktime. Bitcoin, EVM or Lightning to Arkade: the
/// claim deadline. Arkade to EVM or Lightning: when the client's refund
/// unlocks.
/// </param>
/// <param name="BtcRefundLocktime">
/// On-chain Bitcoin HTLC refund locktime. Bitcoin to Arkade or EVM: when
/// the client's refund unlocks. EVM to Bitcoin: the claim deadline.
/// </param>
/// <param name="EvmRefundLocktime">
/// EVM HTLC refund locktime. EVM to Arkade, Bitcoin or Lightning: when the
/// client's refund unlocks. Bitcoin, Arkade or Lightning to EVM: the claim
/// deadline.
/// </param>
/// <param name="InvoiceExpiresAt">
/// Expiry of the invoice the payer pays (Lightning to Arkade or EVM) or
/// the server pays (Arkade or EVM to Lightning). Null also when the server
/// reports no expiry, which it sends as 0. A payer must pay before both
/// this expiry and the claim cutoff on the server-funded leg
/// (<paramref name="VhtlcRefundLocktime"/> for Lightning to Arkade,
/// <paramref name="EvmRefundLocktime"/> for Lightning to EVM); the server
/// does not guarantee which comes first.
/// </param>
public sealed record SwapDetails(
    string Id,
    SwapStatus Status,
    SwapDirection Direction,
    SwapFunding Funding,
    string DepositAmount,
    TokenId DepositToken,
    string ReceiveAddress,
    string ReceiveAmount,
    TokenId ReceiveToken,
    string CreatedAt,
    long? VhtlcRefundLocktime,
    long? BtcRefundLocktime,
    long? EvmRefundLocktime,
    long? InvoiceExpiresAt)
{
    internal static SwapDetails FromFfi(Ffi.Swap s) => new(
        s.@id,
        s.@status,
        s.@direction,
        s.@funding,
        s.@depositAmount,
        s.@depositToken,
        s.@receiveAddress,
        s.@receiveAmount,
        s.@receiveToken,
        s.@createdAt,
        s.@vhtlcRefundLocktime,
        s.@btcRefundLocktime,
        s.@evmRefundLocktime,
        s.@invoiceExpiresAt);
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
/// Single-shot snapshot of the depositor EOA's funding state for a
/// swap. Use this to decide whether
/// <see cref="Client.FundSwapAsync(string, CancellationToken)"/> is
/// ready to call. For the polling/waiting flow, use
/// <see cref="Client.WaitForDepositFundingAsync"/>.
/// </summary>
/// <param name="SourceTokenBalance">Current balance of the source token at the depositor EOA, in smallest unit. Decimal-as-string to fit U256.</param>
/// <param name="SourceTokenRequired">Amount the depositor EOA must hold before the funding userOp can be submitted. Decimal-as-string.</param>
/// <param name="NativeBalanceWei">Native (ETH/MATIC/…) balance at the depositor EOA, in wei. Matters only when the backend has no paymaster.</param>
/// <param name="HasSufficientSourceToken">Convenience: <c>SourceTokenBalance &gt;= SourceTokenRequired</c>.</param>
public sealed record DepositStatus(
    string SourceTokenBalance,
    string SourceTokenRequired,
    ulong NativeBalanceWei,
    bool HasSufficientSourceToken)
{
    internal static DepositStatus FromFfi(Ffi.DepositStatus d) => new(
        d.@sourceTokenBalance,
        d.@sourceTokenRequired,
        d.@nativeBalanceWei,
        d.@hasSufficientSourceToken);
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
/// Result of an on-chain Bitcoin HTLC refund.
/// </summary>
/// <param name="Txid">Refund transaction ID, hex as block explorers show it (no <c>0x</c>).</param>
/// <param name="FeeSats">Fee the refund transaction pays, in satoshis.</param>
public sealed record BitcoinRefundReceipt(string Txid, ulong FeeSats)
{
    internal static BitcoinRefundReceipt FromFfi(Ffi.BitcoinRefundReceipt r) => new(r.@txid, r.@feeSats);
}

/// <summary>
/// Top-level client. Holds an FFI handle whose Rust side owns the
/// per-swap key_index storage that <see cref="CreateSwapAsync"/> writes
/// to and the funding / claim flows read from. Dispose this when done
/// — the underlying Rust resources are released by the handle's
/// <c>IDisposable</c>.
/// </summary>
/// <remarks>
/// Every async method runs its native call on the thread pool. Its
/// <see cref="CancellationToken"/> only stops a call that has not started;
/// a running native call is not interrupted and runs until it completes or
/// hits its own timeout.
/// </remarks>
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
    /// <param name="contractDbPath">
    /// Optional SQLite DB path for ark-rs contract-manager state. If null,
    /// it goes in <paramref name="dataDirectory"/> when that is set, and
    /// otherwise in ark-rs's default <c>contracts.db</c> in the working
    /// directory, shared by every client in the process. A path naming the
    /// <c>swaps.sqlite</c> file of <paramref name="dataDirectory"/> (after
    /// resolving symlinks, <c>.</c> and <c>..</c>, and on macOS and Windows
    /// letter case) throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.Storage"/>.
    /// </param>
    /// <param name="dataDirectory">
    /// Optional directory for durable state, created if missing; use one
    /// per wallet and network, as neither file in it is partitioned by
    /// network. Swaps are kept in <c>swaps.sqlite</c>, so a client built
    /// later over the same directory can still fund and claim them. The
    /// Arkade contract store is <c>contracts-&lt;wallet id&gt;.sqlite</c>,
    /// so a different seed restored over the same directory never shares
    /// it. If null, swaps live only as long as this client. Each kind of
    /// execution through one client, such as wallet spends or swap claims,
    /// runs one at a time, and different kinds can overlap. Separate
    /// instances are not coordinated, even over the same directory: keep one
    /// client per wallet per process, and build a replacement only once the
    /// calls still running on a disposed one have returned or timed out.
    /// </param>
    /// <exception cref="ArgumentException">
    /// <paramref name="network"/> has no default endpoints
    /// (<see cref="BitcoinNetwork.Testnet"/>) and one was not passed.
    /// </exception>
    public Client(
        string mnemonic,
        BitcoinNetwork network = BitcoinNetwork.Mainnet,
        string? baseUrl = null,
        string? arkadeServerUrl = null,
        string? esploraUrl = null,
        string? referralCode = null,
        string? contractDbPath = null,
        string? dataDirectory = null)
    {
        var defaults = Defaults.For(network);
        var resolvedBaseUrl = baseUrl
            ?? defaults?.BaseUrl
            ?? throw NoDefault(network, nameof(baseUrl));
        var resolvedArkadeServerUrl = arkadeServerUrl
            ?? defaults?.ArkadeServerUrl
            ?? throw NoDefault(network, nameof(arkadeServerUrl));
        var resolvedEsploraUrl = esploraUrl
            ?? defaults?.EsploraUrl
            ?? throw NoDefault(network, nameof(esploraUrl));

        string? storagePath = null;
        if (dataDirectory is not null)
        {
            storagePath = Path.Combine(dataDirectory, "swaps.sqlite");
            contractDbPath ??= Path.Combine(
                dataDirectory,
                $"contracts-{TryOrThrow(() => Ffi.SatoraSdkFfiMethods.ContractStoreId(mnemonic))}.sqlite");
            Directory.CreateDirectory(dataDirectory);
        }

        var arkadeConfig = new Ffi.ArkadeConfig(
            resolvedArkadeServerUrl,
            resolvedEsploraUrl,
            mnemonic,
            network,
            contractDbPath);
        _ffi = TryOrThrow(() => Ffi.SatoraClient.NewWithArkade(
            resolvedBaseUrl, mnemonic, arkadeConfig, referralCode, storagePath));
    }

    /// <summary>Releases the FFI handle. Idempotent.</summary>
    /// <remarks>
    /// Calls already running keep going until they return or hit their own
    /// timeout, and only then is the Rust client freed. Build no replacement
    /// client over the same <c>dataDirectory</c> before then.
    /// </remarks>
    public void Dispose() => _ffi.Dispose();

    private static ArgumentException NoDefault(BitcoinNetwork network, string parameter) =>
        new($"{network} has no default endpoints; pass {parameter}.", parameter);

    /// <summary>
    /// Shared helper: invoke an FFI call and rethrow anything the bindings
    /// raise, a Rust panic included, as the public
    /// <see cref="SdkException"/>. Used by both the constructor (which can
    /// fail on invalid base URL) and the method bodies below.
    /// </summary>
    internal static T TryOrThrow<T>(Func<T> call)
    {
        try { return call(); }
        catch (Ffi.UniffiException ex) { throw SdkException.FromFfi(ex); }
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
    /// Create a swap. The direction is resolved from
    /// <paramref name="sourceChain"/>: EVM stablecoin → BTC on Arkade,
    /// Lightning → BTC on Arkade, or on-chain Bitcoin → BTC on Arkade.
    /// For the Lightning and Bitcoin directions prefer the dedicated
    /// <see cref="CreateLightningToArkadeSwapAsync"/> and
    /// <see cref="CreateBitcoinToArkadeSwapAsync"/>: the same routing
    /// with a clearer signature. Other direction combos return
    /// an <see cref="SdkException"/>.
    /// </summary>
    /// <param name="sourceChain">Source chain enum.</param>
    /// <param name="sourceToken">Source token enum.</param>
    /// <param name="targetChain">Target chain enum.</param>
    /// <param name="targetToken">Target token enum.</param>
    /// <param name="amount">
    /// Source / target amount mutex. Bitcoin sources take only
    /// <c>QuoteAmount.Target</c>.
    /// </param>
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
    /// Roll the wallet's recoverable VTXOs (expired, swept or below dust)
    /// and its boarding outputs into the next Arkade batch; healthy VTXOs
    /// are left alone. Returns the hex commitment txid of the batch that
    /// absorbed them, or <c>null</c> if the wallet had nothing to settle.
    /// </summary>
    /// <remarks>
    /// A <see cref="SdkErrorKind.Transport"/> error, or
    /// <see cref="SdkErrorKind.Timeout"/> after 300 seconds, leaves the
    /// outcome unknown; the balance shows whether the batch went through.
    /// The 300 seconds start once any wallet spend in progress on this
    /// client has finished.
    /// </remarks>
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
    /// <remarks>
    /// A <see cref="SdkErrorKind.Transport"/> error, or
    /// <see cref="SdkErrorKind.Timeout"/> after 120 seconds, leaves the
    /// outcome unknown: the payment may have gone out, so check the balance
    /// or the recipient before sending again. The 120 seconds start once any
    /// wallet spend in progress on this client has finished.
    /// </remarks>
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
    /// Create a Lightning → Arkade swap. The dedicated counterpart to
    /// <see cref="CreateArkadeToLightningSwapAsync"/>: the backend
    /// returns a BOLT11 invoice (in <c>swap.Funding</c> as
    /// <c>SwapFunding.Bolt11Invoice</c>) that the user pays from their
    /// own Lightning wallet; the server then funds an Arkade VHTLC the
    /// user sweeps with <see cref="ClaimAsync"/>.
    ///
    /// This routes the same direction as
    /// <see cref="CreateSwapAsync"/> with <c>sourceChain =
    /// ChainId.Lightning</c>, but with a signature that only exposes the
    /// knobs the Lightning rail actually uses.
    /// </summary>
    /// <param name="satsReceive">
    /// Amount the user wants to <b>receive</b> on Arkade, in satoshis.
    /// The server quotes the corresponding BOLT11 amount internally and
    /// embeds it in the returned invoice.
    /// </param>
    /// <param name="receiveTo">
    /// Arkade address (<c>tark1…</c>) that receives the BTC. Pass
    /// <c>null</c> to route to the SDK's own internal Arkade wallet —
    /// requires the client to have been constructed with a mnemonic.
    /// </param>
    public Task<SwapDetails> CreateLightningToArkadeSwapAsync(
        ulong satsReceive,
        Address? receiveTo = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(
                ffi.CreateLightningToArkadeSwap(satsReceive, receiveTo))),
            cancellationToken);
    }

    /// <summary>
    /// Create an on-chain Bitcoin → Arkade swap. The payer sends
    /// <c>swap.DepositAmount</c> sats to the HTLC address in
    /// <c>swap.Funding</c> (<c>SwapFunding.BitcoinAddress</c>); once the
    /// server funds the Arkade VHTLC, the user sweeps it with
    /// <see cref="ClaimAsync"/>. If the swap does not complete,
    /// <see cref="RefundBitcoinAsync"/> returns the deposit once the
    /// HTLC's refund locktime has matured.
    ///
    /// This routes the same direction as <see cref="CreateSwapAsync"/>
    /// with <c>sourceChain = ChainId.Bitcoin</c>. Either way, a server HTLC
    /// address that differs from the one the client's keys rebuild throws
    /// <see cref="SdkException"/> with <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </summary>
    /// <param name="satsReceive">Exact amount to <b>receive</b> on Arkade, in satoshis.</param>
    /// <param name="receiveTo">
    /// Arkade address (<c>tark1…</c>) that receives the BTC. Pass
    /// <c>null</c> to route to the SDK's own internal Arkade wallet.
    /// </param>
    /// <param name="extraFeesBps">
    /// Optional per-swap fee surcharge in basis points, as for
    /// <see cref="CreateSwapAsync"/>.
    /// </param>
    public Task<SwapDetails> CreateBitcoinToArkadeSwapAsync(
        ulong satsReceive,
        Address? receiveTo = null,
        ushort? extraFeesBps = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(
                ffi.CreateBitcoinToArkadeSwap(satsReceive, receiveTo, extraFeesBps))),
            cancellationToken);
    }

    /// <summary>
    /// Refund the on-chain HTLC of a Bitcoin → Arkade swap to
    /// <paramref name="destination"/> once the HTLC's refund locktime has
    /// matured. Chain data comes from the client's esplora endpoint;
    /// unconfirmed HTLC outputs are refunded too, and the call gives up
    /// with <see cref="SdkErrorKind.Timeout"/> after 60 seconds.
    /// </summary>
    /// <param name="swapId">UUID returned by <see cref="CreateBitcoinToArkadeSwapAsync"/>.</param>
    /// <param name="destination">Bitcoin address on the client's network that receives the refund.</param>
    /// <param name="feeRateSatVb">
    /// Fee rate in sat/vB. Pass <c>null</c> to take the esplora estimate
    /// for confirmation within six blocks.
    /// </param>
    /// <param name="maximumFeeSats">
    /// Upper bound on the fee, in satoshis, whatever the rate. HTLC outputs
    /// whose fee would push the refund over it are left for a later call;
    /// if not even one fits, the call throws <see cref="SdkException"/>
    /// with <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </param>
    /// <remarks>
    /// <para>
    /// Once a refund is pending, a call returns it unchanged unless
    /// <paramref name="feeRateSatVb"/> is at least 1 sat/vB above its rate
    /// and the replacement's fee stays within
    /// <paramref name="maximumFeeSats"/>; compare the returned txid or fee
    /// to see whether it was bumped. Once a refund to one address exists, a
    /// different <paramref name="destination"/> throws
    /// <see cref="SdkException"/> with <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </para>
    /// <para>
    /// A <see cref="SdkErrorKind.Transport"/> or
    /// <see cref="SdkErrorKind.Timeout"/> error, or a
    /// <see cref="SdkErrorKind.Storage"/> error after a fee bump, leaves
    /// the outcome unknown: the refund may have been broadcast. Call again
    /// with the same <paramref name="destination"/>, which returns the
    /// pending or confirmed refund instead of building another.
    /// </para>
    /// </remarks>
    public Task<BitcoinRefundReceipt> RefundBitcoinAsync(
        string swapId,
        string destination,
        ulong? feeRateSatVb,
        ulong maximumFeeSats,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => BitcoinRefundReceipt.FromFfi(
                ffi.RefundBitcoin(swapId, destination, feeRateSatVb, maximumFeeSats))),
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
    /// <param name="timeout">Total wait budget. Throws <see cref="SdkException"/> with <see cref="SdkErrorKind.Timeout"/> on timeout.</param>
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
    /// <remarks>
    /// A <see cref="SdkErrorKind.Transport"/> or
    /// <see cref="SdkErrorKind.Timeout"/> error leaves the outcome unknown:
    /// the userOp may have been submitted. So does any
    /// <see cref="SdkErrorKind.Storage"/> error, which may follow the
    /// submission. The operation is stored before submission and a later
    /// call is refused while it is unresolved, so checking the swap status
    /// before acting is safe.
    /// </remarks>
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
    /// Single-shot probe of the depositor EOA for a swap. Returns
    /// immediately with the current balances — no polling, no throw
    /// on "not funded yet". Use this for "is FundSwap ready?"
    /// branching; use
    /// <see cref="WaitForDepositFundingAsync"/> for the polling flow.
    ///
    /// The Rust core picks the node RPC URL from the swap's deposit
    /// chain (same defaults as <see cref="FundSwapAsync(string, CancellationToken)"/>).
    /// Use the
    /// <see cref="CheckDepositAsync(string, string, CancellationToken)"/>
    /// overload if you need a custom RPC.
    /// </summary>
    public Task<DepositStatus> CheckDepositAsync(
        string swapId,
        CancellationToken cancellationToken = default)
        => CheckDepositAsync(swapId, nodeRpcUrl: null, cancellationToken);

    /// <summary>
    /// Single-shot probe of the depositor EOA for a swap, against a
    /// caller-supplied node RPC URL. See
    /// <see cref="CheckDepositAsync(string, CancellationToken)"/> for
    /// the auto-defaulted variant.
    /// </summary>
    public Task<DepositStatus> CheckDepositAsync(
        string swapId,
        string? nodeRpcUrl,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => DepositStatus.FromFfi(
                ffi.CheckDepositFunding(swapId, nodeRpcUrl))),
            cancellationToken);
    }

    /// <summary>
    /// Fetch a swap's current state. Returns the same shape
    /// <see cref="CreateSwapAsync"/> does, so callers can re-read after the
    /// backend transitions states (e.g. ServerFunded).
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
    /// <see cref="SdkErrorKind.Timeout"/>). 3s poll interval is
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
    /// Redeem the Arkade VHTLC for a Bitcoin, EVM or Lightning to Arkade
    /// swap that has reached (or passed) ServerFunded. Sweeps the BTC to
    /// <paramref name="destination"/>.
    /// </summary>
    /// <param name="swapId">UUID of a swap whose backend state is at least <see cref="SwapStatus.ServerFunded"/>.</param>
    /// <param name="destination">Arkade address (`tark1…`) to receive the BTC.</param>
    /// <remarks>
    /// A <see cref="SdkErrorKind.Transport"/> or
    /// <see cref="SdkErrorKind.Timeout"/> error leaves the outcome unknown,
    /// and so does <see cref="SdkErrorKind.Storage"/> once arkd may hold the
    /// claim. The claim's progress is stored before submission, so calling
    /// this again with the same storage resumes it instead of claiming twice.
    /// </remarks>
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

    internal static Endpoints? For(BitcoinNetwork network) => network switch
    {
        BitcoinNetwork.Mainnet => new(
            "https://api.satora.io",
            "https://arkade.computer",
            "https://mempool.space/api"),
        // No Arkade infrastructure runs on testnet. Mutinynet is a signet,
        // so a Testnet client pointed there would fail the swaps' network
        // checks.
        BitcoinNetwork.Testnet => null,
        BitcoinNetwork.Signet => new(
            "https://mutinynetswap.lendasat.com",
            "https://mutinynet.arkade.sh",
            "https://mutinynet.com/api"),
        // The local stack's mempool serves the web app at its root and the
        // esplora API under /api, as on the other networks.
        BitcoinNetwork.Regtest => new(
            "http://localhost:3333",
            "http://localhost:7070",
            "http://localhost:3000/api"),
        _ => throw new ArgumentOutOfRangeException(nameof(network), network, "Unsupported network."),
    };
}
