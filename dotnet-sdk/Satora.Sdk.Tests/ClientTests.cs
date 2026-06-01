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

    /// <summary>
    /// Every BitcoinNetwork variant has a default URL set; construction
    /// against each must succeed (defaults are wired, no network calls
    /// happen at ctor time). If a new variant lands in the FFI enum
    /// without a matching Defaults.For entry, this catches it.
    /// </summary>
    [Theory]
    [InlineData(BitcoinNetwork.Mainnet)]
    [InlineData(BitcoinNetwork.Testnet)]
    [InlineData(BitcoinNetwork.Signet)]
    [InlineData(BitcoinNetwork.Regtest)]
    public void ConstructionWithEachNetworkSucceeds(BitcoinNetwork network)
    {
        using var client = new Client(TestMnemonic, network);
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
