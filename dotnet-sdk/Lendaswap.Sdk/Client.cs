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
/// Top-level client. Today only exposes <see cref="GetVersionAsync"/> as
/// a smoke endpoint; full surface lands as the FFI exports grow.
/// </summary>
public sealed class Client
{
    private readonly string _baseUrl;

    public Client(string baseUrl) => _baseUrl = baseUrl;

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
        var baseUrl = _baseUrl;
        return Task.Run(
            () =>
            {
                try
                {
                    return Version.FromFfi(Ffi.LendaswapSdkFfiMethods.FetchVersion(baseUrl));
                }
                catch (Ffi.SdkException.Internal ex)
                {
                    throw new SdkException(ex.Message, ex);
                }
            },
            cancellationToken);
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
        var baseUrl = _baseUrl;
        return Task.Run(
            () =>
            {
                try
                {
                    return Quote.FromFfi(Ffi.LendaswapSdkFfiMethods.FetchQuote(
                        baseUrl,
                        sourceChain,
                        sourceToken,
                        targetChain,
                        targetToken,
                        amount));
                }
                catch (Ffi.SdkException.Internal ex)
                {
                    throw new SdkException(ex.Message, ex);
                }
            },
            cancellationToken);
    }
}
