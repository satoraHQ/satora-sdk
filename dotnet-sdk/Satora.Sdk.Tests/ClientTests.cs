// Smoke tests for the C# facade. The `live_` tests require a running
// Satora server (default http://localhost:3333) and are marked with
// Trait("Category", "Live") so they're easy to filter out of normal
// `dotnet test` runs.

using Satora.Sdk;
using Xunit;
using BitcoinNetwork = uniffi.satora_sdk_ffi.BitcoinNetwork;
using Ffi = uniffi.satora_sdk_ffi;

namespace Satora.Sdk.Tests;

public class ClientTests
{
    // BIP-39 standard test vector. Real key, but never sent on the wire
    // for the construction-only tests below.
    private const string TestMnemonic =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    private const string OtherMnemonic = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

    // Nothing listens on port 1, so calls through it fail fast without
    // leaving the machine.
    private const string Unreachable = "http://127.0.0.1:1";

    /// <summary>
    /// Smoke test that exercises the entire FFI call chain (Rust →
    /// cdylib → uniffi-generated C# → facade) against a real server.
    /// Skipped unless LENDASWAP_API_URL is set so CI doesn't hit prod.
    /// </summary>
    [Fact]
    [Trait("Category", "Live")]
    public async Task LiveVersionReturnsNonEmptyFields()
    {
        var baseUrl = Environment.GetEnvironmentVariable("LENDASWAP_API_URL");
        if (string.IsNullOrEmpty(baseUrl))
        {
            // Skip without server config — the test harness will report
            // it as passed-with-no-asserts, which is what we want for CI.
            return;
        }

        using var client = new Client(TestMnemonic, baseUrl: baseUrl);
        var version = await client.GetVersionAsync();
        Assert.False(string.IsNullOrEmpty(version.Tag));
        Assert.False(string.IsNullOrEmpty(version.CommitHash));
    }

    [Fact]
    public void ClientConstructionRequiresNoServer()
    {
        // Pure construction must not touch the network or block —
        // protects against regressions where someone accidentally
        // pushes the FFI call into the constructor.
        using var client = new Client(TestMnemonic);
        Assert.NotNull(client);
    }

    public static TheoryData<BitcoinNetwork> Networks => new(Enum.GetValues<BitcoinNetwork>());

    /// <summary>
    /// Every BitcoinNetwork variant but Testnet has default URLs, so
    /// construction against each must succeed (no network calls happen
    /// at ctor time); Testnet must refuse to guess. Driven from the enum,
    /// so a variant added to the FFI without a Defaults.For entry fails
    /// here.
    /// </summary>
    [Theory]
    [MemberData(nameof(Networks))]
    public void ConstructionWithEachNetworkUsesItsDefaults(BitcoinNetwork network)
    {
        if (network == BitcoinNetwork.Testnet)
        {
            Assert.Throws<ArgumentException>(() => new Client(TestMnemonic, network));
            return;
        }
        using var client = new Client(TestMnemonic, network);
        Assert.NotNull(client);
    }

    /// <summary>
    /// Mempool serves the esplora API under /api and its web app at the
    /// root, so a default without the suffix fails on the first chain read.
    /// </summary>
    [Theory]
    [MemberData(nameof(Networks))]
    public void DefaultEsploraUrlsPointAtTheApiRoot(BitcoinNetwork network)
    {
        if (Defaults.For(network) is { } endpoints)
        {
            Assert.EndsWith("/api", endpoints.EsploraUrl);
        }
    }

    [Fact]
    public void TestnetNeedsExplicitEndpoints()
    {
        var ex = Assert.Throws<ArgumentException>(() => new Client(TestMnemonic, BitcoinNetwork.Testnet));
        Assert.Equal("baseUrl", ex.ParamName);
        using var client = new Client(TestMnemonic, BitcoinNetwork.Testnet, Unreachable, Unreachable, Unreachable);
        Assert.NotNull(client);
    }

    [Fact]
    public void InvalidBaseUrlThrowsSdkException()
    {
        // The base URL is parsed eagerly when the client is constructed
        // (Rust `Client::new` → `Url::parse`), so an invalid URL surfaces
        // here as Error::InvalidBaseUrl.
        var ex = Assert.Throws<SdkException>(() => new Client(TestMnemonic, baseUrl: "not a url"));
        Assert.Equal(SdkErrorKind.InvalidBaseUrl, ex.Kind);
        Assert.Null(ex.StatusCode);
        // The url crate's parse error: the payload, without the Rust SDK's
        // "invalid base URL" display prefix.
        Assert.Equal("relative URL without a base", ex.Message);
    }

    public static TheoryData<Exception, SdkErrorKind> BindingErrors => new()
    {
        { new Ffi.SdkException.Cancelled("detail"), SdkErrorKind.Cancelled },
        { new Ffi.SdkException.InvalidBaseUrl("detail"), SdkErrorKind.InvalidBaseUrl },
        { new Ffi.SdkException.Transport("detail"), SdkErrorKind.Transport },
        { new Ffi.SdkException.Decode("detail"), SdkErrorKind.Decode },
        { new Ffi.SdkException.Api(503, "detail"), SdkErrorKind.Api },
        { new Ffi.SdkException.InvalidSigner("detail"), SdkErrorKind.InvalidSigner },
        { new Ffi.SdkException.InvalidSwap("detail"), SdkErrorKind.InvalidSwap },
        { new Ffi.SdkException.UnsupportedQuotePath("detail"), SdkErrorKind.UnsupportedQuotePath },
        { new Ffi.SdkException.Timeout("detail"), SdkErrorKind.Timeout },
        { new Ffi.SdkException.Storage("detail"), SdkErrorKind.Storage },
        { new Ffi.SdkException.UnresolvedFunding("detail"), SdkErrorKind.UnresolvedFunding },
        { new Ffi.SdkException.Internal("detail"), SdkErrorKind.Internal },
        { new Ffi.PanicException("detail"), SdkErrorKind.Internal },
    };

    [Theory]
    [MemberData(nameof(BindingErrors))]
    public void BindingErrorsBecomeSdkExceptionsWithACleanMessage(Exception error, SdkErrorKind kind)
    {
        var ex = Assert.Throws<SdkException>(() => Client.TryOrThrow<int>(() => throw error));
        Assert.Equal(kind, ex.Kind);
        Assert.Equal("detail", ex.Message);
        Assert.Same(error, ex.InnerException);
        Assert.Equal(kind == SdkErrorKind.Api ? 503 : null, ex.StatusCode);
    }

    [Fact]
    public void DataDirectoryHoldsTheSwapStoreFromConstruction()
    {
        using var temp = new TempDirectory();
        var dataDirectory = Path.Combine(temp.Path, "store");
        using (new Client(TestMnemonic, BitcoinNetwork.Regtest, dataDirectory: dataDirectory))
        {
            Assert.True(File.Exists(Path.Combine(dataDirectory, "swaps.sqlite")));
        }
    }

    [Fact]
    public async Task ClientsOverDifferentSeedsGetSeparateContractStores()
    {
        using var temp = new TempDirectory();
        foreach (var mnemonic in new[] { TestMnemonic, OtherMnemonic })
        {
            using var client = new Client(
                mnemonic, BitcoinNetwork.Regtest, Unreachable, Unreachable, Unreachable, dataDirectory: temp.Path);
            // The contract store opens before arkd is dialled, so the
            // failed call still creates it.
            var ex = await Assert.ThrowsAsync<SdkException>(() => client.GetArkadeAddressAsync());
            Assert.Equal(SdkErrorKind.Transport, ex.Kind);
        }
        Assert.Equal(2, Directory.GetFiles(temp.Path, "contracts-*.sqlite").Length);
    }

    [Fact]
    public async Task RenewArkadeGoesThroughTheArkadeWallet()
    {
        using var temp = new TempDirectory();
        using var client = new Client(
            TestMnemonic, BitcoinNetwork.Regtest, Unreachable, Unreachable, Unreachable, dataDirectory: temp.Path);
        // The wallet connects to arkd before it reads any VTXO.
        var ex = await Assert.ThrowsAsync<SdkException>(() => client.RenewArkadeAsync());
        Assert.Equal(SdkErrorKind.Transport, ex.Kind);
    }

    [Fact]
    public async Task ArkadeClaimPossibleReadsTheStoredSwapBeforeArkd()
    {
        using var client = new Client(
            TestMnemonic, BitcoinNetwork.Regtest, Unreachable, Unreachable, Unreachable);
        var ex = await Assert.ThrowsAsync<SdkException>(
            () => client.IsArkadeClaimPossibleAsync("missing"));
        Assert.Equal(SdkErrorKind.InvalidSwap, ex.Kind);
        Assert.Equal("swap record missing", ex.Message);
    }

    [Theory]
    [InlineData("")]
    [InlineData("1.5")]
    [InlineData("-1")]
    [InlineData("0x10")]
    public async Task RelayFundingRefusesARelayFeeThatIsNotADecimalInteger(string maxRelayFee)
    {
        using var client = new Client(
            TestMnemonic, BitcoinNetwork.Regtest, Unreachable, Unreachable, Unreachable);
        var ex = await Assert.ThrowsAsync<SdkException>(() => client.FundSwapViaRelayAsync(
            "missing", TimeSpan.FromMinutes(10), TimeSpan.FromSeconds(5), maxRelayFee));
        Assert.Equal(SdkErrorKind.InvalidSwap, ex.Kind);
        Assert.Contains("max_relay_fee", ex.Message);
    }

    [Fact]
    public async Task RelayFundingAndReconciliationReadTheStoredSwapFirst()
    {
        using var client = new Client(
            TestMnemonic, BitcoinNetwork.Regtest, Unreachable, Unreachable, Unreachable);
        var funded = await Assert.ThrowsAsync<SdkException>(() => client.FundSwapViaRelayAsync(
            "missing", TimeSpan.FromMinutes(10), TimeSpan.FromSeconds(5)));
        Assert.Equal(SdkErrorKind.InvalidSwap, funded.Kind);
        Assert.Contains("recover or create", funded.Message);
        var reconciled = await Assert.ThrowsAsync<SdkException>(
            () => client.ReconcileRelayFundingAsync("missing", TimeSpan.FromSeconds(5)));
        Assert.Equal(SdkErrorKind.InvalidSwap, reconciled.Kind);
        Assert.Equal("swap record missing", reconciled.Message);
        // Refused before the native call is scheduled.
        Assert.Throws<ArgumentOutOfRangeException>(() =>
        {
            _ = client.FundSwapViaRelayAsync(
                "missing", TimeSpan.FromSeconds(-1), TimeSpan.FromSeconds(5));
        });
    }

    [Fact]
    public void ContractDbPathMustNotBeTheSwapStore()
    {
        using var temp = new TempDirectory();
        var ex = Assert.Throws<SdkException>(() => new Client(
            TestMnemonic,
            BitcoinNetwork.Regtest,
            contractDbPath: Path.Combine(temp.Path, ".", "swaps.sqlite"),
            dataDirectory: temp.Path));
        Assert.Equal(SdkErrorKind.Storage, ex.Kind);
    }

    private sealed class TempDirectory : IDisposable
    {
        public string Path { get; } =
            System.IO.Path.Combine(System.IO.Path.GetTempPath(), $"satora-sdk-tests-{Guid.NewGuid():N}");

        public void Dispose()
        {
            if (Directory.Exists(Path))
            {
                Directory.Delete(Path, recursive: true);
            }
        }
    }
}
