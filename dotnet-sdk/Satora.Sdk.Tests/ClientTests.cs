// Smoke tests for the C# facade. The `live_` tests require a running
// Satora server (default http://localhost:3333) and are marked with
// Trait("Category", "Live") so they're easy to filter out of normal
// `dotnet test` runs.

using Satora.Sdk;
using Xunit;
using BitcoinNetwork = uniffi.satora_sdk_ffi.BitcoinNetwork;

namespace Satora.Sdk.Tests;

public class ClientTests
{
    // BIP-39 standard test vector. Real key, but never sent on the wire
    // for the construction-only tests below.
    private const string TestMnemonic =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

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
        // here as Error::InvalidBaseUrl, which the FFI maps to
        // SdkException.Internal and the facade re-wraps as SdkException.
        // The message format is an implementation detail.
        Assert.Throws<SdkException>(() => new Client(TestMnemonic, baseUrl: "not a url"));
    }
}
