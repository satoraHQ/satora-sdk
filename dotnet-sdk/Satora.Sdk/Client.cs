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
/// <see cref="UnresolvedFunding"/> and <see cref="InsufficientGas"/>, it names
/// the failure, not whether a retry is safe: <see cref="Transport"/> and
/// <see cref="Timeout"/> cover both requests that never reached the server
/// and spends whose outcome is unknown, and <see cref="Storage"/> from
/// <see cref="Client.ClaimAsync"/> and <see cref="Client.SettleAsync"/> once
/// arkd may hold the spend, from <see cref="Client.RefundBitcoinAsync"/>
/// after a fee bump, or from
/// <see cref="Client.FundSwapAsync(string, GaslessOpts, CancellationToken)"/>
/// or <see cref="Client.FundSwapViaRelayAsync"/> at any point, or from
/// <see cref="Client.SweepEvmDepositAsync"/> and
/// <see cref="Client.RefundEvmHtlcAsync"/> once the transaction is
/// reserved, leaves the outcome unknown too. Each method's remarks say what
/// to do next.
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
    /// check by hand shows where the funds went. After a relay funding,
    /// <see cref="Client.ReconcileRelayFundingAsync"/> resolves it. From
    /// <see cref="Client.SweepEvmDepositAsync"/> it means a relay funding
    /// signature can still pull the deposit.
    /// </summary>
    UnresolvedFunding,
    /// <summary>
    /// The swap's depositor holds less native gas (ETH on Arbitrum, POL on
    /// Polygon) than the transaction's estimated fee. Nothing was reserved or
    /// sent: send the depositor gas and call again.
    /// </summary>
    InsufficientGas,
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
        Ffi.SdkException.InsufficientGas e => new(SdkErrorKind.InsufficientGas, e.@message, innerException: ex),
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
/// On-chain Bitcoin HTLC refund locktime. Bitcoin to Arkade, EVM or
/// Lightning: when the client's refund unlocks. EVM to Bitcoin: the claim
/// deadline.
/// </param>
/// <param name="EvmRefundLocktime">
/// EVM HTLC refund locktime. EVM to Arkade, Bitcoin or Lightning: when the
/// client's refund unlocks. Bitcoin, Arkade or Lightning to EVM: the claim
/// deadline.
/// </param>
/// <param name="InvoiceExpiresAt">
/// Expiry of the invoice the payer pays (Lightning to Arkade or EVM) or
/// the server pays (Arkade, Bitcoin or EVM to Lightning). Null also when the server
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
/// Exact quote for paying a Lightning destination from Arkade, from
/// <see cref="Client.GetLightningSendQuoteAsync"/>.
/// </summary>
/// <param name="ExchangeRate">Source tokens per BTC locked; "1" for Arkade, where the source is already sats.</param>
/// <param name="MaxAmountSats">Maximum payout for this route, in satoshis.</param>
/// <param name="MinAmountSats">Minimum payout for this route, in satoshis.</param>
/// <param name="NetworkFeeSats">The Lightning provider's send fee for this destination, in satoshis.</param>
/// <param name="ProtocolFeeRate">Protocol fee rate (0.003 = 0.30%).</param>
/// <param name="ProtocolFeeSats">Protocol fee in satoshis.</param>
/// <param name="SourceAmount">Total taken from Arkade (payout plus fees), in satoshis, as a decimal string.</param>
/// <param name="SourceAmountSats">Deprecated by the server in favour of <paramref name="SourceAmount"/>.</param>
/// <param name="TargetAmountSats">Paid out on the Lightning invoice, in satoshis.</param>
public sealed record LightningSendQuote(
    string ExchangeRate,
    ulong MaxAmountSats,
    ulong MinAmountSats,
    ulong NetworkFeeSats,
    double ProtocolFeeRate,
    ulong ProtocolFeeSats,
    string SourceAmount,
    ulong SourceAmountSats,
    ulong TargetAmountSats)
{
    internal static LightningSendQuote FromFfi(Ffi.LightningSendQuote q) => new(
        q.@exchangeRate,
        q.@maxAmountSats,
        q.@minAmountSats,
        q.@networkFeeSats,
        q.@protocolFeeRate,
        q.@protocolFeeSats,
        q.@sourceAmount,
        q.@sourceAmountSats,
        q.@targetAmountSats);
}

/// <summary>
/// Sats at a swap's VHTLC by state, from <see cref="Client.AmountsForSwapAsync"/>.
/// </summary>
/// <param name="SpendableSats">Spendable by an offchain transaction.</param>
/// <param name="RecoverableSats">Expired outputs that only <see cref="Client.SettleAsync"/> can spend.</param>
/// <param name="SpentSats">Already spent.</param>
/// <param name="SpentBy">
/// Transactions that spent the VHTLC (hex, <c>0x…</c>); useful when
/// another device completed the swap.
/// </param>
public sealed record VhtlcAmounts(
    ulong SpendableSats,
    ulong RecoverableSats,
    ulong SpentSats,
    IReadOnlyList<string> SpentBy)
{
    internal static VhtlcAmounts FromFfi(Ffi.VhtlcAmounts a) => new(
        a.@spendableSats,
        a.@recoverableSats,
        a.@spentSats,
        a.@spentBy);
}

/// <summary>One swap's status in a <see cref="BulkStatus"/>.</summary>
public sealed record SwapStatusEntry(string Id, SwapStatus Status)
{
    internal static SwapStatusEntry FromFfi(Ffi.SwapStatusEntry e) => new(e.@id, e.@status);
}

/// <summary>
/// Statuses of the swaps <see cref="Client.GetBulkStatusAsync"/> was asked about.
/// </summary>
/// <param name="Statuses">Status of each known swap.</param>
/// <param name="NotFound">Requested IDs the server does not know.</param>
public sealed record BulkStatus(IReadOnlyList<SwapStatusEntry> Statuses, IReadOnlyList<string> NotFound)
{
    internal static BulkStatus FromFfi(Ffi.BulkStatus b) => new(
        b.@statuses.Select(SwapStatusEntry.FromFfi).ToArray(),
        b.@notFound);
}

/// <summary>
/// Swaps <see cref="Client.RecoverAllSwapsAsync"/> found.
/// </summary>
/// <param name="SwapIds">IDs of the recovered swaps, now in the client's storage.</param>
/// <param name="NextIndex">Wallet key index the next new swap uses.</param>
/// <param name="ScannedUntil">
/// Last wallet key index the scan covered. A later scan for newer swaps
/// starts at <paramref name="NextIndex"/> (or the original start index),
/// not after this one.
/// </param>
public sealed record RecoveryResult(IReadOnlyList<string> SwapIds, uint NextIndex, uint ScannedUntil)
{
    internal static RecoveryResult FromFfi(Ffi.RecoveryResult r) => new(r.@swapIds, r.@nextIndex, r.@scannedUntil);
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
/// A relay funding proven on chain, from <see cref="Client.FundSwapViaRelayAsync"/>.
/// </summary>
/// <param name="TxHash">Funding transaction hash, <c>0x…</c>.</param>
/// <param name="Eip2612Permit">Whether the relay was also sent an EIP-2612 approval of Permit2.</param>
public sealed record RelayFunding(string TxHash, bool Eip2612Permit)
{
    internal static RelayFunding FromFfi(Ffi.RelayFundingReceipt r) => new(r.@txHash, r.@eip2612Permit);
}

/// <summary>
/// What a swap's depositor holds, from <see cref="Client.GetDepositorBalanceAsync"/>.
/// Decimal strings; parse with <see cref="System.Numerics.BigInteger"/>.
/// </summary>
/// <param name="TokenUnits">Balance of the token asked about, in its smallest unit.</param>
/// <param name="NativeWei">Native gas balance (ETH on Arbitrum, POL on Polygon), in wei.</param>
public sealed record DepositorBalance(string TokenUnits, string NativeWei)
{
    internal static DepositorBalance FromFfi(Ffi.DepositorBalance b) => new(b.@tokenUnits, b.@nativeWei);
}

/// <summary>
/// A mined EVM transaction the SDK sent and verified, from
/// <see cref="Client.SweepEvmDepositAsync"/>.
/// </summary>
/// <param name="TxHash">Transaction hash, <c>0x…</c>.</param>
public sealed record EvmReceipt(string TxHash)
{
    internal static EvmReceipt FromFfi(Ffi.EvmReceipt r) => new(r.@txHash);
}

/// <summary>
/// A verified HTLC refund, from <see cref="Client.RefundEvmHtlcAsync"/>.
/// </summary>
/// <param name="TxHash">Refund transaction hash, <c>0x…</c>.</param>
/// <param name="LockToken">
/// Contract (<c>0x…</c>) of the token the HTLC locked, which the refund paid
/// to the depositor; pass it to <see cref="Client.SweepEvmDepositAsync"/> as
/// <c>token</c> to send it on.
/// </param>
public sealed record HtlcRefund(string TxHash, string LockToken)
{
    internal static HtlcRefund FromFfi(Ffi.HtlcRefund r) => new(r.@txHash, r.@lockToken);
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
/// What <see cref="Client.OffboardArkadeAsync"/> pays on chain, and the
/// server's fee for it, from <see cref="Client.GetArkadeOffboardQuoteAsync"/>.
/// </summary>
/// <param name="AmountSats">Satoshis the on-chain address receives.</param>
/// <param name="FeeSats">The server's fee in satoshis, taken from change on top of <paramref name="AmountSats"/>.</param>
public sealed record OffboardQuote(ulong AmountSats, ulong FeeSats)
{
    internal static OffboardQuote FromFfi(Ffi.OffboardQuote q) => new(q.@amountSats, q.@feeSats);
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
    /// an <see cref="SdkException"/>. A Lightning swap is checked as
    /// <see cref="CreateLightningToArkadeSwapAsync(ulong, Address?, CancellationToken)"/>
    /// describes before anything is stored.
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
    /// Create a gasless swap from a stablecoin on Polygon or Arbitrum to BTC
    /// on Arkade that pays exactly <paramref name="satsReceive"/> sats.
    /// <see cref="SwapDetails.Funding"/> is <c>SwapFunding.Gasless</c> with the
    /// swap's own deposit address, and <see cref="SwapDetails.DepositAmount"/>
    /// the token amount to send there; fund it with
    /// <see cref="FundSwapViaRelayAsync"/> once it arrives. This is
    /// <see cref="CreateSwapAsync"/> with <c>gasless: true</c> and
    /// <c>QuoteAmount.Target</c>, so it checks no more than that does: a
    /// server answer whose deposit address is not the one this client derived
    /// throws <see cref="SdkException"/> with <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </summary>
    /// <param name="source">A Polygon or Arbitrum token, such as <c>new TokenId.UsdcPolygon()</c>.</param>
    /// <param name="satsReceive">Sats the swap pays on Arkade.</param>
    /// <param name="receiveTo">
    /// Arkade address that receives them; null for the SDK's own Arkade wallet.
    /// </param>
    /// <param name="extraFeesBps">Optional per-swap fee surcharge, as for <see cref="CreateSwapAsync"/>.</param>
    /// <exception cref="ArgumentException">
    /// <paramref name="source"/> is not a token this SDK names on Polygon or Arbitrum.
    /// </exception>
    public Task<SwapDetails> CreateEvmToArkadeSwapAsync(
        TokenId source,
        ulong satsReceive,
        Address? receiveTo = null,
        ushort? extraFeesBps = null,
        CancellationToken cancellationToken = default)
        => CreateSwapAsync(
            EvmChainOf(source),
            source,
            new ChainId.Arkade(),
            new TokenId.Btc(),
            new QuoteAmount.Target(satsReceive),
            receiveTo,
            gasless: true,
            extraFeesBps,
            cancellationToken);

    internal static ChainId EvmChainOf(TokenId token) => token switch
    {
        TokenId.UsdcPolygon or TokenId.UsdtPolygon or TokenId.WbtcPolygon => new ChainId.Polygon(),
        TokenId.UsdcArbitrum or TokenId.Usdt0Arbitrum or TokenId.WbtcArbitrum => new ChainId.Arbitrum(),
        _ => throw new ArgumentException(
            $"{token} is not a Polygon or Arbitrum token; use CreateSwapAsync.", "source"),
    };

    /// <summary>
    /// Offchain VTXO balance of the SDK's internal Arkade wallet,
    /// broken into the three buckets ark-client distinguishes — see
    /// <see cref="ArkadeBalance"/>. Hits the Arkade server's gRPC
    /// indexer.
    /// </summary>
    /// <remarks>
    /// <see cref="SendArkadeAsync"/>, <see cref="FundArkadeSwapAsync"/> and
    /// <see cref="OffboardArkadeAsync"/> spend <c>balance.confirmedSats</c>
    /// plus <c>balance.preConfirmedSats</c>, less VTXOs about to expire; the
    /// offboard also keeps dust as change.
    /// <c>balance.recoverableSats</c> moves only through
    /// <see cref="SettleArkadeAsync"/>.
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
    /// This settles the wallet's own outputs; <see cref="SettleAsync"/>
    /// spends a swap's VHTLC.
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
    /// Roll the wallet's VTXOs that expire within the margin
    /// <see cref="SendArkadeAsync"/>, <see cref="FundArkadeSwapAsync"/> and
    /// <see cref="OffboardArkadeAsync"/> leave out into the next Arkade
    /// batch, as fresh confirmed VTXOs at the same address. Returns the hex
    /// commitment txid of the batch that renewed them, or <c>null</c> if none
    /// was due.
    /// </summary>
    /// <remarks>
    /// <para>
    /// <see cref="SettleArkadeAsync"/> takes VTXOs only once they have
    /// expired, so call this often enough to catch them inside the margin:
    /// on regtest, whose VTXOs live 512 s, within the last 256 s of their
    /// life. Recoverable VTXOs and boarding outputs are left to
    /// <see cref="SettleArkadeAsync"/>.
    /// </para>
    /// <para>
    /// A <see cref="SdkErrorKind.Transport"/> error, or
    /// <see cref="SdkErrorKind.Timeout"/> after 300 seconds, leaves the
    /// outcome unknown; the balance shows whether the batch went through, and
    /// renewing again is safe. The 300 seconds start once any wallet spend in
    /// progress on this client has finished.
    /// </para>
    /// </remarks>
    public Task<string?> RenewArkadeAsync(CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(() => TryOrThrow(() => ffi.ArkadeRenew()), cancellationToken);
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
    /// Arkade wallet to <paramref name="destination"/> (an Arkade address
    /// on the configured server) via an offchain Ark transaction.
    /// Returns the Ark txid as <c>0x…</c> hex. Fund a swap's VHTLC with
    /// <see cref="FundArkadeSwapAsync"/> instead, which journals the
    /// payment and resolves a retry from it.
    /// </summary>
    /// <remarks>
    /// <para>
    /// An address for another network, or whose server key is not the
    /// Arkade server's current signer, is refused with
    /// <see cref="SdkErrorKind.InvalidSwap"/> before anything is sent.
    /// </para>
    /// <para>
    /// It spends confirmed and pre-confirmed VTXOs, including inputs a failed
    /// <see cref="FundArkadeSwapAsync"/> journaled; once arkd names and
    /// returns this spend, which pays nothing to its VHTLC, that call
    /// resolves its journal.
    /// </para>
    /// <para>
    /// VTXOs that expire within the margin <see cref="OffboardArkadeAsync"/>
    /// leaves stay out, since the payment's outputs would expire with them.
    /// When the rest falls short, the send throws
    /// <see cref="SdkErrorKind.InvalidSwap"/> before anything is sent, naming
    /// the sats left out as expiring or recoverable.
    /// </para>
    /// <para>
    /// A <see cref="SdkErrorKind.Transport"/> error, or
    /// <see cref="SdkErrorKind.Timeout"/> after 120 seconds, leaves the
    /// outcome unknown: the payment may have gone out, so check the balance
    /// or the recipient before sending again. The 120 seconds start once any
    /// wallet spend in progress on this client has finished.
    /// </para>
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
    /// Pay the VHTLC of an Arkade → Lightning or Arkade → EVM swap from the
    /// SDK's internal Arkade wallet. Returns the Ark txid as <c>0x…</c> hex.
    /// The wallet inputs are journaled in the client's storage before the
    /// payment and the txid after it. Refuses with
    /// <see cref="SdkErrorKind.InvalidSwap"/> a swap that is not
    /// <see cref="SwapStatus.Pending"/>, whose VHTLC holds outputs this
    /// client did not journal, or whose VHTLC is locked to a server key other
    /// than arkd's current signer (create a new swap then).
    /// </summary>
    /// <param name="swapId">UUID returned by <see cref="CreateArkadeToLightningSwapAsync"/>.</param>
    /// <remarks>
    /// <para>
    /// Before journaling, it also refuses a VHTLC amount below arkd's dust,
    /// and a VHTLC whose hash lock is not the swap's: the hash of this
    /// client's preimage for Arkade → EVM, the payment hash of the stored
    /// invoice for Arkade → Lightning. A new selection draws on confirmed
    /// and pre-confirmed VTXOs, less those that expire within the margin
    /// <see cref="OffboardArkadeAsync"/> leaves, since the VHTLC would expire
    /// with them; a balance short without them throws
    /// <see cref="SdkErrorKind.InvalidSwap"/> naming the sats left out as
    /// expiring or recoverable. A swap this client never created or
    /// recovered throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.Storage"/>.
    /// </para>
    /// <para>
    /// A retry returns a journaled txid. Otherwise an Ark transaction arkd
    /// names as the spender of any journaled input may be the payment, or
    /// that of an earlier attempt still in flight when a retry widened the
    /// selection: the call returns that transaction once its VHTLC output is
    /// visible and throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.UnresolvedFunding"/> until then, unless arkd
    /// returns it and none of its outputs pays the VHTLC. A journaled input
    /// spent without arkd naming a spender, a journaled input arkd does not
    /// know, and a journal for a VHTLC address other than the swap's throw
    /// the same way and are never resolved automatically. While a funding is
    /// unresolved, follow the swap's status, never pay the VHTLC by other
    /// means, and give the swap up only once a check by hand shows where the
    /// funds went.
    /// </para>
    /// <para>
    /// Journaled inputs left unspent, forfeited into a batch, or spent by
    /// transactions that pay nothing to the VHTLC prove the payment never
    /// happened. The swap is then paid again under the checks above from a
    /// selection that includes every journaled input still spendable, so
    /// arkd accepts at most one of the two payments if the first is still in
    /// flight. When no journaled input is spendable any more, that rests on
    /// arkd also refusing the ones this client sees as expired, swept,
    /// unrolled or under a retired signer. A swap that no longer awaits
    /// funding instead has such a journal emptied before the refusal. With
    /// storage that kept this swap's journal, that refusal is the only error
    /// proving the wallet paid nothing; the others, such as a refusal before
    /// the journal is read, leave an earlier payment possible. A swap
    /// recovered on another device, or after a restart without
    /// <c>dataDirectory</c>, has no journal.
    /// </para>
    /// <para>
    /// A <see cref="SdkErrorKind.Transport"/> error, or
    /// <see cref="SdkErrorKind.Timeout"/> after 120 seconds, leaves the
    /// outcome unknown: call this again with the same storage, which
    /// resolves it from the journal, and never pay the VHTLC by other means
    /// such as <see cref="SendArkadeAsync"/>. The 120 seconds start once any
    /// wallet spend in progress on this client has finished. Once arkd
    /// accepted the payment, a failure to journal its txid is logged and the
    /// txid returned; a retry resolves it from the journaled inputs. A
    /// journaled input that the new payment must spend but that expires
    /// within the margin also throws <see cref="SdkErrorKind.Transport"/>,
    /// until it has expired. Disposing the client does not stop a running
    /// call, so build no replacement over the same storage until it has
    /// returned.
    /// </para>
    /// </remarks>
    public Task<string> FundArkadeSwapAsync(
        string swapId,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ffi.FundArkadeSwap(swapId)),
            cancellationToken);
    }

    /// <summary>
    /// Send <paramref name="amountSats"/> from the SDK's internal Arkade
    /// wallet to the on-chain Bitcoin <paramref name="address"/> in a
    /// collaborative exit. Returns the commitment txid as <c>0x…</c> hex.
    /// Every exit keeps at least the server's dust (330 sats today) in the
    /// wallet as Arkade change: ark-client 0.11 registers a signing key for
    /// every exit but signs the batch's VTXO tree only for one that creates
    /// a VTXO, so arkd would fail any batch an exit without change shares
    /// with others. The exit spends confirmed and pre-confirmed VTXOs only,
    /// the soonest-expiring first. Recoverable VTXOs (a batch that mixes
    /// them with healthy ones fails) and boarding outputs need
    /// <see cref="SettleArkadeAsync"/> first; VTXOs carrying an asset stay
    /// offchain. VTXOs that expire before ark-client's four batch join
    /// attempts could end (each can wait out its intent's 120 s validity
    /// plus an arkd session; at most this call's 600 s) stay out too, since
    /// one that expires while the batch is pending fails it; once expired
    /// they are recoverable. On a server whose VTXOs live minutes, such as
    /// regtest's 512 s, that margin is half the VTXO lifetime, so a retried
    /// exit there can still meet an expiring input. The lifetime is read off
    /// the wallet's confirmed VTXOs, so a wallet that has only received
    /// offchain gets the full margin until a settle or boarding gives it one.
    /// </summary>
    /// <param name="address">Bitcoin address on the client's network.</param>
    /// <param name="amountSats">
    /// Satoshis <paramref name="address"/> receives exactly, as the server's
    /// fee comes out of change; an amount that cannot leave dust as change
    /// is refused. Pass <c>null</c> to send everything except the
    /// recoverable balance and the change: the most that leaves at least
    /// dust, or less than dust plus one fee step when the server's fee
    /// program skips the amount that leaves exactly dust.
    /// </param>
    /// <remarks>
    /// Address, dust and balance checks throw <see cref="SdkException"/>
    /// with <see cref="SdkErrorKind.InvalidSwap"/> before anything is sent,
    /// naming the sats left out as expiring or recoverable when the
    /// spendable balance falls short, and so does a server fee program that
    /// charges the inputs or the change, which a collaborative exit does not
    /// pay. A <see cref="SdkErrorKind.Transport"/> error, or
    /// <see cref="SdkErrorKind.Timeout"/> after 600 seconds, leaves the
    /// outcome unknown: the batch may still settle, so check the balance
    /// before offboarding again. The 600 seconds start once any wallet spend
    /// in progress on this client has finished. Disposing the client does
    /// not stop a running call, so build no replacement over the same
    /// storage until it has returned.
    /// </remarks>
    public Task<string> OffboardArkadeAsync(
        string address,
        ulong? amountSats,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ffi.ArkadeOffboard(address, amountSats)),
            cancellationToken);
    }

    /// <summary>
    /// The amount and fee <see cref="OffboardArkadeAsync"/> would use for
    /// the same arguments, without sending. It waits for a wallet spend in
    /// progress on this client, so it prices what is left afterwards.
    /// </summary>
    /// <remarks>
    /// It fails as <see cref="OffboardArkadeAsync"/> does, except that
    /// nothing is ever sent: call it again freely after a
    /// <see cref="SdkErrorKind.Transport"/> error or a
    /// <see cref="SdkErrorKind.Timeout"/> after 300 seconds, which start
    /// once any wallet spend in progress on this client has finished.
    /// </remarks>
    public Task<OffboardQuote> GetArkadeOffboardQuoteAsync(
        string address,
        ulong? amountSats,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => OffboardQuote.FromFfi(ffi.ArkadeOffboardQuote(address, amountSats))),
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
    ///
    /// A server invoice that does not pay the hash of this client's
    /// preimage throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.InvalidSwap"/>, and nothing is stored:
    /// paying it would let the server keep the payment without funding the
    /// VHTLC.
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
    /// Create a Lightning → Arkade swap with an exact source or target
    /// amount, a fee surcharge or an invoice description; otherwise as
    /// <see cref="CreateLightningToArkadeSwapAsync(ulong, Address?, CancellationToken)"/>.
    /// </summary>
    /// <param name="amount">
    /// <c>QuoteAmount.Source</c> fixes the invoice amount,
    /// <c>QuoteAmount.Target</c> what arrives on Arkade, both in satoshis.
    /// </param>
    /// <param name="receiveTo">
    /// Arkade address (<c>tark1…</c>) that receives the BTC. Pass
    /// <c>null</c> to route to the SDK's own internal Arkade wallet.
    /// </param>
    /// <param name="extraFeesBps">
    /// Optional per-swap fee surcharge in basis points, as for
    /// <see cref="CreateSwapAsync"/>.
    /// </param>
    /// <param name="invoiceDescription">
    /// Description in the invoice, at most 639 UTF-8 bytes. Pass
    /// <c>null</c> for the server default.
    /// </param>
    public Task<SwapDetails> CreateLightningToArkadeSwapAsync(
        QuoteAmount amount,
        Address? receiveTo = null,
        ushort? extraFeesBps = null,
        string? invoiceDescription = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(ffi.CreateLightningToArkadeSwapWithOptions(
                amount, receiveTo, extraFeesBps, invoiceDescription))),
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
    /// Fee rate in sat/vB. Pass <c>null</c> to take Mempool's recommended
    /// hour fee from the esplora endpoint, or plain esplora's estimate for
    /// confirmation within six blocks, never below 1 sat/vB.
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
    ///
    /// A supplied invoice that does not parse throws
    /// <see cref="SdkException"/> with <see cref="SdkErrorKind.InvalidSwap"/>
    /// before any request is sent. A hash lock that is not the payment hash
    /// of the returned invoice, or a returned invoice other than the
    /// supplied one, throws the same, and nothing is stored. For a Lightning
    /// address or LNURL the server picks the invoice, so only their
    /// agreement is checked.
    /// </summary>
    /// <param name="destination">
    /// Lightning destination. Use <c>LightningDestination.Invoice</c>
    /// for a BOLT11 (amount embedded), or
    /// <c>LightningDestination.Address</c> /
    /// <c>LightningDestination.Lnurl</c> with an explicit <c>sats</c>
    /// — the server resolves LNURL-pay itself, no client-side LNURL
    /// resolver needed. <c>LightningDestination.AddressSendMax</c> /
    /// <c>LightningDestination.LnurlSendMax</c> take a fixed
    /// <c>amount</c> from Arkade instead and pay out what remains after
    /// fees.
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
    /// Quote paying <paramref name="destination"/> from Arkade BTC with the
    /// Lightning provider's fee for that destination, unlike
    /// <see cref="GetQuoteAsync"/>, whose Lightning fee is an estimate.
    /// Informational: creating the swap quotes the fee again, and it can
    /// move in between.
    /// </summary>
    /// <param name="destination">Destination as for <see cref="CreateArkadeToLightningSwapAsync"/>.</param>
    /// <param name="extraFeesBps">
    /// Optional per-swap fee surcharge in basis points, as for
    /// <see cref="CreateSwapAsync"/>.
    /// <see cref="CreateArkadeToLightningSwapAsync"/> has no surcharge
    /// parameter yet, so pass <c>null</c> to quote like for like.
    /// </param>
    public Task<LightningSendQuote> GetLightningSendQuoteAsync(
        LightningDestination destination,
        ushort? extraFeesBps = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => LightningSendQuote.FromFfi(
                ffi.GetLightningSendQuote(destination, extraFeesBps))),
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
    /// submission. The operation is stored before submission, so a later
    /// call throws <see cref="SdkErrorKind.InvalidSwap"/> when its hash was
    /// recorded and <see cref="SdkErrorKind.UnresolvedFunding"/> when it
    /// was not; checking the swap status before acting is safe. This SDK
    /// can neither record a lost hash nor release the stored operation, so
    /// <see cref="SdkErrorKind.UnresolvedFunding"/> then comes back on every
    /// call: follow the swap status, and never fund the swap by other means.
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
    /// Fund an EVM-sourced swap created with <c>gasless: true</c> through the
    /// Satora server's relay, and return once the funding's receipt proves it.
    /// The swap's depositor key signs a Permit2 transfer, plus an unlimited
    /// EIP-2612 approval of Permit2 when the depositor has no Permit2
    /// allowance, and the server submits the funding and pays its gas. The
    /// depositor must already hold the swap's
    /// <see cref="SwapDetails.DepositAmount"/>; poll
    /// <see cref="CheckDepositAsync(string, string, CancellationToken)"/>
    /// for that. The server's copy of the swap and its funding payload must
    /// match the stored swap before anything is signed.
    /// </summary>
    /// <param name="swapId">UUID returned by the creation call.</param>
    /// <param name="permitDeadline">
    /// How long the signatures stay valid, in whole seconds from signing. The
    /// relay can land the funding until then, so the depositor's tokens stay
    /// committed to it until chain time passes that deadline by 120 seconds,
    /// and <see cref="SweepEvmDepositAsync"/> waits as long. It must end
    /// before the swap's <see cref="SwapDetails.EvmRefundLocktime"/>; a
    /// funding started late should pass a shorter deadline so that a funding
    /// landing at its end still leaves time to claim.
    /// </param>
    /// <param name="timeout">Bounds the whole call, including the relay's wait for its receipt.</param>
    /// <param name="maxRelayFee">
    /// Largest relay fee the server may take from the input, in the source
    /// token's smallest unit as a decimal string. Only Ethereum charges one.
    /// Anything but a decimal integer throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.InvalidSwap"/> before anything is sent.
    /// </param>
    /// <param name="nodeRpcUrl">EVM node for the balance, allowance, permit and receipt reads; null for the chain's public default.</param>
    /// <remarks>
    /// Every attempt signs the swap's one Permit2 nonce, so at most one can
    /// fund it, and calling this again after any error is safe. A refusal
    /// before the request is sent, such as <see cref="SdkErrorKind.Api"/>
    /// when the DEX rate moved or <see cref="SdkErrorKind.InvalidSwap"/> for
    /// an insufficient balance, leaves nothing in flight, and the next call
    /// tries again. Once the request may have left,
    /// <see cref="SdkErrorKind.Transport"/>, <see cref="SdkErrorKind.Timeout"/>,
    /// <see cref="SdkErrorKind.Storage"/>, <see cref="SdkErrorKind.Decode"/>,
    /// <see cref="SdkErrorKind.Api"/> or <see cref="SdkErrorKind.InvalidSwap"/>
    /// can leave the outcome unknown; the next call then throws
    /// <see cref="SdkErrorKind.UnresolvedFunding"/> without sending anything.
    /// Resolve that with <see cref="ReconcileRelayFundingAsync"/>, never by
    /// funding the swap another way. Once the funding's hash is recorded the
    /// call throws <see cref="SdkErrorKind.InvalidSwap"/>, and reconciliation
    /// reports it funded.
    /// </remarks>
    public Task<RelayFunding> FundSwapViaRelayAsync(
        string swapId,
        TimeSpan permitDeadline,
        TimeSpan timeout,
        string maxRelayFee = "0",
        string? nodeRpcUrl = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        var opts = new Ffi.RelayOpts(
            maxRelayFee,
            nodeRpcUrl,
            Seconds(permitDeadline, nameof(permitDeadline), roundUp: false),
            Seconds(timeout, nameof(timeout), roundUp: true));
        return Task.Run(
            () => TryOrThrow(() => RelayFunding.FromFfi(ffi.FundSwapGaslessRelay(swapId, opts))),
            cancellationToken);
    }

    /// <summary>
    /// Resolve a relay funding whose outcome is unknown from the chain and the
    /// server, without sending anything. <c>RelayFundingState.Funded</c>
    /// carries the funding hash. <c>RelayFundingState.Pending</c> is not
    /// proven yet: with <c>nonceConsumed</c> the swap was funded under a
    /// transaction this client does not know and must never be funded again;
    /// without, a signature may still land until chain time passes
    /// <c>deadline</c> (unix seconds) by 120 seconds.
    /// <c>RelayFundingState.Expired</c> means nothing can land any more, and
    /// <see cref="FundSwapViaRelayAsync"/> may run again.
    /// </summary>
    /// <param name="swapId">UUID of a swap funded through <see cref="FundSwapViaRelayAsync"/>.</param>
    /// <param name="timeout">Bounds the whole call.</param>
    /// <param name="nodeRpcUrl">EVM node for the chain reads; null for the chain's public default.</param>
    /// <remarks>
    /// A swap never funded through the relay throws <see cref="SdkException"/>
    /// with <see cref="SdkErrorKind.InvalidSwap"/>. Nothing is sent, so every
    /// error can be retried.
    /// </remarks>
    public Task<RelayFundingState> ReconcileRelayFundingAsync(
        string swapId,
        TimeSpan timeout,
        string? nodeRpcUrl = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        var opts = new Ffi.RelayOpts(
            "0", nodeRpcUrl, 0, Seconds(timeout, nameof(timeout), roundUp: true));
        return Task.Run(
            () => TryOrThrow(() => ffi.ReconcileGaslessRelayFunding(swapId, opts)),
            cancellationToken);
    }

    /// <summary>
    /// The balance of <paramref name="token"/>, the swap's source token when
    /// null, and of native gas at a stored EVM-sourced swap's depositor
    /// address. Reads only, so every error can be retried.
    /// </summary>
    /// <param name="swapId">UUID of an EVM-sourced swap in this client's storage.</param>
    /// <param name="token">ERC-20 contract address (<c>0x…</c>); null for the swap's source token.</param>
    /// <param name="nodeRpcUrl">EVM node; null for the chain's public default.</param>
    /// <remarks>
    /// An argument that does not parse throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.InvalidSwap"/> before anything is read, and so
    /// does a swap this client does not store.
    /// </remarks>
    public Task<DepositorBalance> GetDepositorBalanceAsync(
        string swapId,
        string? token = null,
        string? nodeRpcUrl = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => DepositorBalance.FromFfi(
                ffi.DepositorBalance(swapId, token, nodeRpcUrl))),
            cancellationToken);
    }

    /// <summary>
    /// Send the depositor's whole balance of <paramref name="token"/>, the
    /// swap's source token when null, to <paramref name="destination"/>, and
    /// return once the receipt shows the transfer: a deposit that cannot fund
    /// the swap, the excess over a proven funding, or the lock token
    /// <see cref="RefundEvmHtlcAsync"/> returned.
    /// </summary>
    /// <param name="swapId">UUID of an EVM-sourced swap in this client's storage.</param>
    /// <param name="destination">EVM address (<c>0x…</c>) that receives the tokens; not the depositor itself.</param>
    /// <param name="timeout">Bounds the whole call, including the wait for the receipt.</param>
    /// <param name="token">ERC-20 contract address (<c>0x…</c>); null for the swap's source token.</param>
    /// <param name="nodeRpcUrl">EVM node for the reads and the broadcast; null for the chain's public default.</param>
    /// <remarks>
    /// <para>
    /// The depositor pays the gas from its own native balance, so send it a
    /// little ETH (Arbitrum) or POL (Polygon) first: with less than the
    /// transfer's estimated fee the call throws
    /// <see cref="SdkErrorKind.InsufficientGas"/> before anything is reserved.
    /// While a relay funding signature can still pull the deposit it throws
    /// <see cref="SdkErrorKind.UnresolvedFunding"/> before anything is sent:
    /// reconcile the funding with <see cref="ReconcileRelayFundingAsync"/>,
    /// or wait until chain time passes the signatures' deadline by 120
    /// seconds. An argument that does not parse throws
    /// <see cref="SdkErrorKind.InvalidSwap"/> before anything is read.
    /// </para>
    /// <para>
    /// The transfer is reserved with its amount before it is sent and its
    /// hash recorded after, so calling again with the same token and
    /// destination verifies a recorded transfer instead of sending another,
    /// and returns the settled receipt once the balance it moved is gone. A
    /// transfer that mined and reverted moved nothing: the call throws
    /// <see cref="SdkErrorKind.InvalidSwap"/>, and the next call sends a new
    /// one. <see cref="SdkErrorKind.Transport"/>,
    /// <see cref="SdkErrorKind.Timeout"/> and
    /// <see cref="SdkErrorKind.Storage"/> can leave the outcome unknown; call
    /// again. A transfer reserved but never recorded is sent again by the next
    /// call with the same token and destination, from the balance then held,
    /// since a second whole-balance transfer to the same destination moves
    /// what is there or reverts. Once that balance is gone the call throws
    /// <see cref="SdkErrorKind.InvalidSwap"/> and releases the reservation.
    /// Until then a sweep of another token or to another destination throws
    /// <see cref="SdkErrorKind.InvalidSwap"/> naming the sweep to repeat.
    /// </para>
    /// </remarks>
    public Task<EvmReceipt> SweepEvmDepositAsync(
        string swapId,
        string destination,
        TimeSpan timeout,
        string? token = null,
        string? nodeRpcUrl = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        var seconds = Seconds(timeout, nameof(timeout), roundUp: true);
        return Task.Run(
            () => TryOrThrow(() => EvmReceipt.FromFfi(
                ffi.SweepEvmDeposit(swapId, token, destination, nodeRpcUrl, seconds))),
            cancellationToken);
    }

    /// <summary>
    /// Refund a funded EVM-sourced swap's HTLC once chain time reaches its
    /// <see cref="SwapDetails.EvmRefundLocktime"/>. The depositor calls the
    /// coordinator's <c>refundTo</c>, which pays the token the HTLC locked, a
    /// BTC-pegged token rather than the source token, back to the depositor;
    /// <see cref="SweepEvmDepositAsync"/> with the returned
    /// <see cref="HtlcRefund.LockToken"/> sends it on.
    /// </summary>
    /// <param name="swapId">UUID of an EVM-sourced swap in this client's storage.</param>
    /// <param name="timeout">Bounds the whole call, including the wait for the receipt.</param>
    /// <param name="nodeRpcUrl">EVM node for the reads and the broadcast; null for the chain's public default.</param>
    /// <remarks>
    /// <para>
    /// The depositor pays the gas, so send it a little native gas first.
    /// Before the locktime the call throws
    /// <see cref="SdkErrorKind.InvalidSwap"/>, and with less native gas than
    /// the refund's estimated fee <see cref="SdkErrorKind.InsufficientGas"/>,
    /// both before anything is reserved.
    /// </para>
    /// <para>
    /// The refund is reserved before it is sent and its hash recorded after,
    /// so calling again verifies the recorded refund instead of sending
    /// another. A refund that mined and reverted refunded nothing: the call
    /// throws <see cref="SdkErrorKind.InvalidSwap"/>, and the next call sends
    /// a new one while the HTLC is active. <see cref="SdkErrorKind.Transport"/>,
    /// <see cref="SdkErrorKind.Timeout"/> and <see cref="SdkErrorKind.Storage"/>
    /// can leave the outcome unknown; call again. A refund reserved but never
    /// recorded is sent again while the HTLC is active, since a second
    /// <c>refundTo</c> of the same deposit reverts. Once the HTLC is no longer
    /// active, or the coordinator no longer holds its deposit, the call throws
    /// <see cref="SdkErrorKind.InvalidSwap"/>: that refund or a claim settled
    /// it, so check the depositor's balance of the lock token.
    /// </para>
    /// </remarks>
    public Task<HtlcRefund> RefundEvmHtlcAsync(
        string swapId,
        TimeSpan timeout,
        string? nodeRpcUrl = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        var seconds = Seconds(timeout, nameof(timeout), roundUp: true);
        return Task.Run(
            () => TryOrThrow(() => HtlcRefund.FromFfi(
                ffi.RefundEvmHtlc(swapId, nodeRpcUrl, seconds))),
            cancellationToken);
    }

    private static ulong Seconds(TimeSpan span, string parameter, bool roundUp)
    {
        if (span < TimeSpan.Zero)
        {
            throw new ArgumentOutOfRangeException(parameter, span, "Must not be negative.");
        }
        return (ulong)(roundUp ? Math.Ceiling(span.TotalSeconds) : Math.Floor(span.TotalSeconds));
    }

    /// <summary>
    /// Fetch a swap's current state. Returns the same shape
    /// <see cref="CreateSwapAsync"/> does, so callers can re-read after the
    /// backend transitions states (e.g. ServerFunded).
    /// </summary>
    /// <remarks>
    /// A server copy that names another swap than
    /// <paramref name="swapId"/>, or that changes the direction of a swap
    /// this client stored, throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.InvalidSwap"/>, and so does one whose invoice
    /// no longer pays the hash lock of a Lightning swap this client stored:
    /// never pay or fund the swap then. A swap stored only as its key
    /// index, as in an older database, gets the invoice checks
    /// <see cref="RecoverAllSwapsAsync"/> applies: a Lightning to Arkade
    /// invoice must pay the hash of this client's preimage, and an Arkade to
    /// Lightning hash lock must be its invoice's payment hash.
    /// </remarks>
    public Task<SwapDetails> GetSwapAsync(string swapId, CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => SwapDetails.FromFfi(ffi.GetSwap(swapId))),
            cancellationToken);
    }

    /// <summary>
    /// Current status of each swap in <paramref name="ids"/>, any number
    /// of them, in requests of at most 100 IDs. Read-only, so safe to call
    /// again after any error.
    /// </summary>
    public Task<BulkStatus> GetBulkStatusAsync(
        IEnumerable<string> ids,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        var idArray = ids.ToArray();
        return Task.Run(
            () => TryOrThrow(() => BulkStatus.FromFfi(ffi.GetBulkStatus(idArray))),
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
    /// <remarks>
    /// Each poll is checked as <see cref="GetSwapAsync"/> describes, and a
    /// server copy it refuses ends the wait with <see cref="SdkException"/>
    /// and <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </remarks>
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
    /// Once arkd has refused the stored claim because an input expired, the
    /// call throws <see cref="SdkErrorKind.InvalidSwap"/>, and
    /// <see cref="SettleAsync"/> then claims the VHTLC in a batch while the
    /// claim window is open.
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

    /// <summary>
    /// Claim or refund a swap's Arkade VHTLC to <paramref name="destination"/>.
    /// This spends a swap's VHTLC; <see cref="SettleArkadeAsync"/> recovers
    /// the wallet's own expired, swept or sub-dust VTXOs and boarding
    /// outputs. Unlike <see cref="ClaimAsync"/>, which refuses a VHTLC
    /// holding recoverable outputs, this settles those in a batch with the
    /// cosigner key the server reports. A claim is refused within 300
    /// seconds of the VHTLC refund locktime.
    /// </summary>
    /// <param name="swapId">UUID of a swap with an Arkade VHTLC.</param>
    /// <param name="operation">
    /// <c>ArkadeSpend.Claim</c> for Bitcoin, EVM or Lightning to Arkade;
    /// <c>ArkadeSpend.Refund</c> (after the refund locktime) or
    /// <c>ArkadeSpend.CooperativeRefund</c> (cosigned by the swap server,
    /// before it) for Arkade to EVM or Lightning.
    /// </param>
    /// <param name="destination">Arkade address (<c>tark1…</c>) to receive the BTC.</param>
    /// <remarks>
    /// A <see cref="SdkErrorKind.Transport"/> or
    /// <see cref="SdkErrorKind.Timeout"/> error leaves the outcome unknown,
    /// and so does <see cref="SdkErrorKind.Storage"/> once arkd may hold the
    /// spend. The spend's progress is stored before submission, so calling
    /// this again with the same storage resumes it instead of spending twice;
    /// the swap status and <see cref="AmountsForSwapAsync"/> show where it
    /// stands.
    /// </remarks>
    public Task<ArkadeSettlement> SettleAsync(
        string swapId,
        ArkadeSpend operation,
        string destination,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ffi.Settle(swapId, operation, destination)),
            cancellationToken);
    }

    /// <summary>
    /// Spendable, recoverable and spent sats at a stored swap's VHTLC, read
    /// through the internal Arkade wallet. Reads only the swap record in
    /// the client's storage, which swap creation and
    /// <see cref="RecoverAllSwapsAsync"/> write; a swap missing from it
    /// throws <see cref="SdkException"/> with
    /// <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </summary>
    public Task<VhtlcAmounts> AmountsForSwapAsync(
        string swapId,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => VhtlcAmounts.FromFfi(ffi.AmountsForSwap(swapId))),
            cancellationToken);
    }

    /// <summary>
    /// Whether a claim of a swap's Arkade VHTLC can still complete, read
    /// from the stored claim progress, the Arkade indexer and the claim
    /// window, without sending anything that carries the preimage.
    /// <c>false</c> is final: no later call can complete the claim.
    /// <c>true</c> means <see cref="SettleAsync"/> finishes or resumes the
    /// claim, or already has. Reads only the swap record in the client's
    /// storage; a swap missing from it throws <see cref="SdkException"/>
    /// with <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </summary>
    public Task<bool> IsArkadeClaimPossibleAsync(
        string swapId,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => ffi.ArkadeClaimPossible(swapId)),
            cancellationToken);
    }

    /// <summary>
    /// Read EVM chain <paramref name="chainId"/> through
    /// <paramref name="nodeRpcUrl"/>, replacing any earlier reader for that
    /// chain. When the server funds an EVM to Arkade swap's VHTLC below the
    /// target the swap was created with, because the deposit converted to
    /// less than quoted, <see cref="ClaimAsync"/> and <see cref="SettleAsync"/>
    /// accept it only within <see cref="SetConversionShortfallLimit"/> and
    /// only if the swap's own EVM lock, read through this reader, shows that
    /// shortfall. Without a reader for the swap's chain such a claim is
    /// refused.
    /// </summary>
    /// <param name="chainId">EVM chain id, such as 42161 for Arbitrum.</param>
    /// <param name="nodeRpcUrl">
    /// EVM node of that chain; null for its public default, a mainnet node.
    /// A test fork keeps mainnet's chain id, so pass the fork's own node.
    /// </param>
    /// <remarks>
    /// Connects once. It throws <see cref="SdkException"/> and keeps the
    /// earlier reader for a node that serves another chain
    /// (<see cref="SdkErrorKind.InvalidSwap"/>) or cannot be reached
    /// (<see cref="SdkErrorKind.Transport"/>), and before connecting for a
    /// URL that does not parse or, with <paramref name="nodeRpcUrl"/> null,
    /// a chain without a public default (both
    /// <see cref="SdkErrorKind.InvalidSwap"/>), or for a client built
    /// without a mnemonic (<see cref="SdkErrorKind.InvalidSigner"/>). The
    /// reader only reads.
    /// </remarks>
    public Task SetEvmReaderAsync(
        ulong chainId,
        string? nodeRpcUrl = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() =>
            {
                ffi.SetEvmReader(chainId, nodeRpcUrl);
                return 0; // TryOrThrow wants a T; the void method returns nothing.
            }),
            cancellationToken);
    }

    /// <summary>
    /// Accept an EVM to Arkade VHTLC up to <paramref name="basisPoints"/>
    /// hundredths of a percent below the target its swap was created with,
    /// when the swap's EVM lock shows a conversion shortfall that large (see
    /// <see cref="SetEvmReaderAsync"/>). The default of 0 refuses any VHTLC
    /// below that target. Only the server states the lock it expected, so
    /// this limit is all that bounds how far a dishonest server can lower a
    /// payout; a merchant's payment tolerance is a natural value. Every claim
    /// whose amount is checked after this returns uses the new limit.
    /// </summary>
    /// <param name="basisPoints">At most 10000, a whole target.</param>
    /// <exception cref="SdkException">
    /// <paramref name="basisPoints"/> is over 10000, with
    /// <see cref="SdkErrorKind.InvalidSwap"/>.
    /// </exception>
    public void SetConversionShortfallLimit(uint basisPoints) =>
        TryOrThrow(() =>
        {
            _ffi.SetConversionShortfallLimit(basisPoints);
            return 0;
        });

    /// <summary>
    /// Find this wallet's swaps on the server, from wallet key index
    /// <paramref name="startIndex"/> (null for 0) until the server's gap
    /// limit, and store each with its key index so this client can claim
    /// and refund it. Only the wallet's public recovery key is sent.
    /// </summary>
    /// <remarks>
    /// Pages already processed stay stored if a later one fails, and a
    /// swap stored again keeps its progress, so call again after a
    /// <see cref="SdkErrorKind.Transport"/> or
    /// <see cref="SdkErrorKind.Timeout"/> error.
    /// <see cref="SdkErrorKind.InvalidSwap"/> refuses a page holding a
    /// swap that fails this client's checks, such as a Lightning swap
    /// whose invoice does not pay its hash lock: nothing from that page is
    /// stored, and calling again refuses it again.
    /// </remarks>
    public Task<RecoveryResult> RecoverAllSwapsAsync(
        uint? startIndex = null,
        CancellationToken cancellationToken = default)
    {
        var ffi = _ffi;
        return Task.Run(
            () => TryOrThrow(() => RecoveryResult.FromFfi(ffi.RecoverAllSwaps(startIndex))),
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
