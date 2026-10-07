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

    [Fact]
    public void LightningSendQuoteKeepsEveryField()
    {
        var quote = LightningSendQuote.FromFfi(new Ffi.LightningSendQuote(
            @exchangeRate: "1",
            @maxAmountSats: 1,
            @minAmountSats: 2,
            @networkFeeSats: 3,
            @protocolFeeRate: 0.004,
            @protocolFeeSats: 5,
            @sourceAmount: "6",
            @sourceAmountSats: 7,
            @targetAmountSats: 8));

        Assert.Equal(new LightningSendQuote("1", 1, 2, 3, 0.004, 5, "6", 7, 8), quote);
    }

    [Fact]
    public void VhtlcAmountsKeepEachBucketAndSpendingTxid()
    {
        var amounts = VhtlcAmounts.FromFfi(new Ffi.VhtlcAmounts(
            @spendableSats: 1,
            @recoverableSats: 2,
            @spentSats: 3,
            @spentBy: new[] { "0xaa", "0xbb" }));

        Assert.Equal((1UL, 2UL, 3UL), (amounts.SpendableSats, amounts.RecoverableSats, amounts.SpentSats));
        Assert.Equal(new[] { "0xaa", "0xbb" }, amounts.SpentBy);
    }

    [Fact]
    public void OffboardQuoteKeepsAmountAndFee()
    {
        var quote = OffboardQuote.FromFfi(new Ffi.OffboardQuote(@amountSats: 1, @feeSats: 2));

        Assert.Equal(new OffboardQuote(AmountSats: 1, FeeSats: 2), quote);
    }

    [Fact]
    public void BulkStatusKeepsEachEntryAndTheUnknownIds()
    {
        var status = BulkStatus.FromFfi(new Ffi.BulkStatus(
            @statuses: new[]
            {
                new Ffi.SwapStatusEntry("swap-a", new Ffi.SwapStatus.Pending()),
                new Ffi.SwapStatusEntry("swap-b", new Ffi.SwapStatus.ServerRedeemed()),
            },
            @notFound: new[] { "swap-c" }));

        Assert.Equal(
            new[]
            {
                new SwapStatusEntry("swap-a", new Ffi.SwapStatus.Pending()),
                new SwapStatusEntry("swap-b", new Ffi.SwapStatus.ServerRedeemed()),
            },
            status.Statuses);
        Assert.Equal(new[] { "swap-c" }, status.NotFound);
    }

    [Fact]
    public void RelayFundingKeepsTheHashAndThePermitFlag()
    {
        var funding = RelayFunding.FromFfi(new Ffi.RelayFundingReceipt(
            @txHash: "0xab",
            @eip2612Permit: true));

        Assert.Equal(new RelayFunding(TxHash: "0xab", Eip2612Permit: true), funding);
    }

    [Fact]
    public void RecoveryResultKeepsTheSwapIdsAndTheCursor()
    {
        var result = RecoveryResult.FromFfi(new Ffi.RecoveryResult(
            @swapIds: new[] { "swap-a", "swap-b" },
            @nextIndex: 6,
            @scannedUntil: 25));

        Assert.Equal(new[] { "swap-a", "swap-b" }, result.SwapIds);
        Assert.Equal((6U, 25U), (result.NextIndex, result.ScannedUntil));
    }
}
