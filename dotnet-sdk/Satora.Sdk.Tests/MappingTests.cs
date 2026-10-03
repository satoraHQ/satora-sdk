// Offline tests of the facade records built from the generated bindings'
// types. Every field gets its own value, so a mix-up between two fields
// of the same type fails.

using Satora.Sdk;
using Xunit;
using Ffi = uniffi.satora_sdk_ffi;

namespace Satora.Sdk.Tests;

public class MappingTests
{
    [Fact]
    public void SwapDetailsKeepDirectionCreationTimeAndEachDeadline()
    {
        var swap = SwapDetails.FromFfi(new Ffi.Swap(
            @id: "swap",
            @status: new Ffi.SwapStatus.ServerFunded(),
            @direction: new Ffi.SwapDirection.LightningToArkade(),
            @funding: new Ffi.SwapFunding.Bolt11Invoice("lnbcrt1invoice"),
            @depositAmount: "1000",
            @depositToken: new Ffi.TokenId.Btc(),
            @receiveAddress: "tark1receiver",
            @receiveAmount: "900",
            @receiveToken: new Ffi.TokenId.Btc(),
            @createdAt: "2026-09-01T00:00:07Z",
            @vhtlcRefundLocktime: 1_001,
            @btcRefundLocktime: 1_002,
            @evmRefundLocktime: 1_003,
            @invoiceExpiresAt: 1_004));

        Assert.Equal(
            new SwapDetails(
                "swap",
                new Ffi.SwapStatus.ServerFunded(),
                new Ffi.SwapDirection.LightningToArkade(),
                new Ffi.SwapFunding.Bolt11Invoice("lnbcrt1invoice"),
                "1000",
                new Ffi.TokenId.Btc(),
                "tark1receiver",
                "900",
                new Ffi.TokenId.Btc(),
                "2026-09-01T00:00:07Z",
                1_001,
                1_002,
                1_003,
                1_004),
            swap);
    }
}
