// Smoke tests for the C# facade. The `live_` tests require a running
// Lendaswap server (default http://localhost:3333) and are marked with
// Trait("Category", "Live") so they're easy to filter out of normal
// `dotnet test` runs.

using Lendaswap.Sdk;
using Xunit;

namespace Lendaswap.Sdk.Tests;

public class ClientTests
{
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

        var client = new Client(baseUrl);
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
        var client = new Client("https://example.invalid");
        Assert.NotNull(client);
    }

    [Fact]
    public async Task InvalidBaseUrlThrowsSdkException()
    {
        var client = new Client("not a url");
        // The Rust side returns Error::InvalidBaseUrl, which the FFI
        // maps to SdkException.Internal. We just check that something
        // throws — the message format is implementation detail.
        await Assert.ThrowsAnyAsync<Exception>(() => client.GetVersionAsync());
    }
}
