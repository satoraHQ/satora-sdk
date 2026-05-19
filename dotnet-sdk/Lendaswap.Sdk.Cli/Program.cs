// Tiny sample CLI demonstrating the Lendaswap.Sdk surface.
//
// Today only one subcommand exists: `quote`. It demonstrates the
// friendly-name → typed-enum translation (`Arb:USDT` → ChainId.Arbitrum
// + TokenId.Usdt0Arbitrum) plus human-unit → smallest-unit scaling
// (`10 USD` → 10_000_000 because USDT has 6 decimals).
//
//   lendaswap quote --source Arb:USDT --target Arkade:BTC --source-amount "10 USD"
//   lendaswap quote --source Arb:USDT --target Arkade:BTC --target-amount "1000 sats"
//
// The CLI keeps the friendly-name lookup table here, NOT in the SDK
// itself, because they're presentation concerns — different
// applications will want different aliases (a wallet UI calling tokens
// by symbol, an admin tool calling them by contract address, etc.).

using Lendaswap.Sdk;

// The SDK's public surface re-exposes these uniffi-generated tagged
// enums. We re-alias them here so the CLI can write `ChainId.Arbitrum`
// without depending on the generated-namespace path.
using ChainId = uniffi.lendaswap_sdk_ffi.ChainId;
using TokenId = uniffi.lendaswap_sdk_ffi.TokenId;
using QuoteAmount = uniffi.lendaswap_sdk_ffi.QuoteAmount;
using Address = uniffi.lendaswap_sdk_ffi.Address;

return await Cli.RunAsync(args).ConfigureAwait(false);

internal static class Cli
{
    private const string DefaultBaseUrl = "https://api.satora.io";

    internal static async Task<int> RunAsync(string[] args)
    {
        if (args.Length == 0 || args[0] is "-h" or "--help")
        {
            PrintUsage();
            return 0;
        }

        try
        {
            return args[0] switch
            {
                "quote" => await QuoteCommand.RunAsync(args[1..]).ConfigureAwait(false),
                "create-swap" => await CreateSwapCommand.RunAsync(args[1..]).ConfigureAwait(false),
                _ => Fail($"unknown subcommand: {args[0]}"),
            };
        }
        catch (SdkException ex)
        {
            // The Rust side already serialises a human-readable message
            // (incl. wrapped backend HTTP errors). Don't dump the
            // stack trace on expected outcomes like 400s — just show
            // the message.
            Console.Error.WriteLine($"error: {ex.Message}");
            return 1;
        }
    }

    private static int Fail(string message)
    {
        Console.Error.WriteLine($"error: {message}");
        PrintUsage();
        return 2;
    }

    private static void PrintUsage()
    {
        Console.Error.WriteLine("""
            lendaswap — Lendaswap SDK sample CLI

            USAGE:
                lendaswap quote        --source <chain:token> --target <chain:token> --source-amount "<value> <unit>"
                lendaswap quote        --source <chain:token> --target <chain:token> --target-amount "<value> <unit>"
                lendaswap create-swap  --source <chain:token> --target <chain:token> --target-amount "<value> <unit>" --receive-to "<address>" [--gasless]

            EXAMPLES:
                lendaswap quote        --source Arb:USDT --target Arkade:BTC --source-amount "10 USD"
                lendaswap quote        --source Arb:USDT --target Arkade:BTC --target-amount "1000 sats"
                lendaswap create-swap  --source Arb:USDC --target Arkade:BTC --target-amount "10000 sats" --receive-to "tark1q..." --gasless

            CHAIN ALIASES: Arb, Eth, Pol, Arkade, Lightning, Bitcoin
            TOKEN ALIASES: USDC, USDT, USDT0, WBTC, BTC
            UNITS:         USD (×10^6), sats (×1), raw (×1)

            ENV:
                LENDASWAP_API_URL   override the default backend
                                    (default: https://api.satora.io)
                MNEMONIC            BIP-39 mnemonic — required for `create-swap`.
                                    Stays in this process's memory only; never
                                    persisted, never echoed.
            """);
    }
}

/// <summary>
/// Implements the <c>quote</c> subcommand. Parsing is hand-rolled
/// (System.CommandLine would be cleaner but adds a dep for what is
/// effectively five flags).
/// </summary>
internal static class QuoteCommand
{
    internal static async Task<int> RunAsync(string[] args)
    {
        string? source = null;
        string? target = null;
        string? sourceAmount = null;
        string? targetAmount = null;

        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--source": source = TakeValue(args, ref i, "--source"); break;
                case "--target": target = TakeValue(args, ref i, "--target"); break;
                case "--source-amount": sourceAmount = TakeValue(args, ref i, "--source-amount"); break;
                case "--target-amount": targetAmount = TakeValue(args, ref i, "--target-amount"); break;
                default:
                    Console.Error.WriteLine($"error: unexpected arg `{args[i]}`");
                    return 2;
            }
        }

        if (source is null || target is null)
        {
            Console.Error.WriteLine("error: --source and --target are required");
            return 2;
        }

        if ((sourceAmount is null) == (targetAmount is null))
        {
            Console.Error.WriteLine("error: exactly one of --source-amount / --target-amount must be set");
            return 2;
        }

        var (sourceChain, sourceToken) = ResolvePair(source);
        var (targetChain, targetToken) = ResolvePair(target);

        QuoteAmount amount = sourceAmount is not null
            ? new QuoteAmount.Source(ParseAmount(sourceAmount, sourceToken))
            : new QuoteAmount.Target(ParseAmount(targetAmount!, targetToken));

        var baseUrl = Environment.GetEnvironmentVariable("LENDASWAP_API_URL") ?? "https://api.satora.io";
        var client = new Client(baseUrl);
        var quote = await client.GetQuoteAsync(sourceChain, sourceToken, targetChain, targetToken, amount).ConfigureAwait(false);

        Console.WriteLine($"  rate           : {quote.ExchangeRate}");
        Console.WriteLine($"  source_amount  : {quote.SourceAmount}");
        Console.WriteLine($"  target_amount  : {quote.TargetAmount}");
        Console.WriteLine($"  net_source     : {quote.NetSourceAmount}");
        Console.WriteLine($"  net_target     : {quote.NetTargetAmount}");
        Console.WriteLine($"  protocol_fee   : {quote.ProtocolFee} (rate {quote.ProtocolFeeRate})");
        Console.WriteLine($"  network_fee    : {quote.NetworkFee}");
        Console.WriteLine($"  gasless_fee    : {quote.GaslessNetworkFee}");
        Console.WriteLine($"  min_amount     : {quote.MinAmount} sats");
        Console.WriteLine($"  max_amount     : {quote.MaxAmount} sats");
        if (quote.BridgeFee.HasValue)
        {
            Console.WriteLine($"  bridge_fee     : {quote.BridgeFee.Value}");
        }
        return 0;
    }

    private static string TakeValue(string[] args, ref int i, string flag)
    {
        if (i + 1 >= args.Length)
        {
            throw new ArgumentException($"{flag} requires a value");
        }
        return args[++i];
    }

    /// <summary>
    /// Parse a `chain:token` shorthand into typed SDK enums. Aliases are
    /// case-insensitive. The combination disambiguates token variants
    /// that exist on multiple chains (USDC on Arbitrum vs. Ethereum etc).
    /// </summary>
    private static (ChainId Chain, TokenId Token) ResolvePair(string pair)
    {
        var parts = pair.Split(':', 2);
        if (parts.Length != 2)
        {
            throw new ArgumentException($"expected `chain:token`, got `{pair}`");
        }
        var chainAlias = parts[0].Trim().ToLowerInvariant();
        var tokenAlias = parts[1].Trim().ToUpperInvariant();

        ChainId chain = chainAlias switch
        {
            "arb" or "arbitrum" => new ChainId.Arbitrum(),
            "eth" or "ethereum" => new ChainId.Ethereum(),
            "pol" or "polygon" => new ChainId.Polygon(),
            "arkade" => new ChainId.Arkade(),
            "lightning" or "ln" => new ChainId.Lightning(),
            "bitcoin" or "btc" => new ChainId.Bitcoin(),
            _ => throw new ArgumentException($"unknown chain alias `{chainAlias}`"),
        };

        // Token variants in the SDK already encode the chain
        // (`UsdcArbitrum`, `UsdtPolygon`, …), so the pair (chain alias,
        // token alias) maps directly to a single TokenId variant. On
        // Arbitrum, `USDT` and `USDT0` both resolve to `Usdt0Arbitrum`
        // since the bridged-USDT legacy contract isn't supported.
        TokenId token = (chainAlias, tokenAlias) switch
        {
            (_, "BTC") => new TokenId.Btc(),
            ("pol" or "polygon", "USDC") => new TokenId.UsdcPolygon(),
            ("arb" or "arbitrum", "USDC") => new TokenId.UsdcArbitrum(),
            ("eth" or "ethereum", "USDC") => new TokenId.UsdcEthereum(),
            ("pol" or "polygon", "USDT") => new TokenId.UsdtPolygon(),
            ("eth" or "ethereum", "USDT") => new TokenId.UsdtEthereum(),
            ("arb" or "arbitrum", "USDT" or "USDT0") => new TokenId.Usdt0Arbitrum(),
            ("pol" or "polygon", "WBTC") => new TokenId.WbtcPolygon(),
            ("arb" or "arbitrum", "WBTC") => new TokenId.WbtcArbitrum(),
            ("eth" or "ethereum", "WBTC") => new TokenId.WbtcEthereum(),
            _ => throw new ArgumentException($"unknown token `{tokenAlias}` on chain `{chainAlias}`"),
        };

        return (chain, token);
    }

    /// <summary>
    /// Parse a "value unit" string into the smallest unit of the
    /// associated token. Examples:
    ///   "10 USD"   → 10 × 10^6 = 10_000_000 (any 6-decimal stablecoin)
    ///   "1000 sats"→ 1_000 (BTC is already in sats)
    ///   "100000"   → 100_000 (raw; no scaling)
    /// </summary>
    private static ulong ParseAmount(string input, TokenId token)
    {
        var trimmed = input.Trim();
        var parts = trimmed.Split(' ', 2, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
        var rawValue = parts[0];
        var unit = parts.Length > 1 ? parts[1].ToLowerInvariant() : "raw";
        _ = token; // accepted for API symmetry; decimals are inferred from the unit, not the token

        return unit switch
        {
            "usd" => CheckedScale(decimal.Parse(rawValue, System.Globalization.CultureInfo.InvariantCulture), 6),
            "sats" or "sat" => ulong.Parse(rawValue, System.Globalization.CultureInfo.InvariantCulture),
            "raw" or "" => ulong.Parse(rawValue, System.Globalization.CultureInfo.InvariantCulture),
            _ => throw new ArgumentException($"unknown unit `{unit}` (expected USD, sats, or raw)"),
        };
    }

    /// <summary>
    /// Multiply a decimal by 10^<paramref name="decimals"/> and round to a ulong,
    /// erroring on overflow.
    /// </summary>
    internal static ulong CheckedScale(decimal value, int decimals)
    {
        var scale = (decimal)Math.Pow(10, decimals);
        var scaled = value * scale;
        if (scaled < 0 || scaled > ulong.MaxValue)
        {
            throw new OverflowException($"amount {value} × 10^{decimals} does not fit in ulong");
        }
        return (ulong)scaled;
    }

    /// <summary>
    /// Shared chain-token alias resolver — exposed internal so
    /// <see cref="CreateSwapCommand"/> can reuse the same alias table
    /// the <c>quote</c> subcommand uses (`Arb:USDC` etc.) without
    /// duplicating the lookup.
    /// </summary>
    internal static (ChainId Chain, TokenId Token) ResolvePairInternal(string pair) => ResolvePair(pair);

    /// <summary>
    /// Likewise exposed for <see cref="CreateSwapCommand"/>'s amount
    /// flags — same `"10 USD"` / `"1000 sats"` / `"100000"` parsing.
    /// </summary>
    internal static ulong ParseAmountInternal(string input, TokenId token) => ParseAmount(input, token);
}

/// <summary>
/// Implements the <c>create-swap</c> subcommand. Drives
/// <see cref="Client.CreateSwapAsync"/> against the configured backend
/// using a mnemonic from the <c>MNEMONIC</c> env var.
/// </summary>
internal static class CreateSwapCommand
{
    internal static async Task<int> RunAsync(string[] args)
    {
        string? source = null;
        string? target = null;
        string? sourceAmount = null;
        string? targetAmount = null;
        string? receiveTo = null;
        var gasless = false;

        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--source": source = TakeValue(args, ref i, "--source"); break;
                case "--target": target = TakeValue(args, ref i, "--target"); break;
                case "--source-amount": sourceAmount = TakeValue(args, ref i, "--source-amount"); break;
                case "--target-amount": targetAmount = TakeValue(args, ref i, "--target-amount"); break;
                case "--receive-to": receiveTo = TakeValue(args, ref i, "--receive-to"); break;
                case "--gasless": gasless = true; break;
                default:
                    Console.Error.WriteLine($"error: unexpected arg `{args[i]}`");
                    return 2;
            }
        }

        if (source is null || target is null || receiveTo is null)
        {
            Console.Error.WriteLine("error: --source, --target and --receive-to are required");
            return 2;
        }

        if ((sourceAmount is null) == (targetAmount is null))
        {
            Console.Error.WriteLine("error: exactly one of --source-amount / --target-amount must be set");
            return 2;
        }

        var mnemonic = Environment.GetEnvironmentVariable("MNEMONIC");
        if (string.IsNullOrWhiteSpace(mnemonic))
        {
            Console.Error.WriteLine("error: MNEMONIC env var must be set for create-swap (BIP-39 phrase).");
            return 2;
        }

        var (sourceChain, sourceToken) = QuoteCommand.ResolvePairInternal(source);
        var (targetChain, targetToken) = QuoteCommand.ResolvePairInternal(target);

        QuoteAmount amount = sourceAmount is not null
            ? new QuoteAmount.Source(QuoteCommand.ParseAmountInternal(sourceAmount, sourceToken))
            : new QuoteAmount.Target(QuoteCommand.ParseAmountInternal(targetAmount!, targetToken));

        // Tag the receive address with the target chain's rail so the
        // SDK's direction validator accepts it. The chain → Address
        // variant mapping mirrors how the Rust e2e wraps `receive_to`
        // before handing it to `create_evm_to_arkade_swap`.
        // The FFI `Address` enum doesn't carry a chain-id wire string —
        // it just tags the rail. Map ChainId → Address rail; an unknown
        // chain (`Other`) is rejected because we can't infer the rail.
        Address receiveAddress = targetChain switch
        {
            ChainId.Arkade => new Address.Arkade(receiveTo),
            ChainId.Bitcoin => new Address.Bitcoin(receiveTo),
            ChainId.Lightning => new Address.Lightning(receiveTo),
            ChainId.Arbitrum or ChainId.Ethereum or ChainId.Polygon
                => new Address.Evm(receiveTo),
            _ => throw new ArgumentException(
                $"cannot infer Address rail for target chain {targetChain} — only Arkade/Bitcoin/Lightning/EVM are supported."),
        };

        var baseUrl = Environment.GetEnvironmentVariable("LENDASWAP_API_URL") ?? "https://api.satora.io";
        var client = new Client(baseUrl, mnemonic);
        var swap = await client.CreateSwapAsync(
            sourceChain,
            sourceToken,
            targetChain,
            targetToken,
            amount,
            receiveAddress,
            gasless).ConfigureAwait(false);

        Console.WriteLine($"  swap_id        : {swap.Id}");
        Console.WriteLine($"  status         : {swap.Status}");
        Console.WriteLine($"  deposit_amount : {swap.DepositAmount}");
        Console.WriteLine($"  deposit_token  : {swap.DepositToken}");
        Console.WriteLine($"  receive_amount : {swap.ReceiveAmount}");
        Console.WriteLine($"  receive_token  : {swap.ReceiveToken}");
        Console.WriteLine($"  receive_to     : {swap.ReceiveAddress}");
        Console.WriteLine($"  funding        : {swap.Funding}");
        return 0;
    }

    private static string TakeValue(string[] args, ref int i, string flag)
    {
        if (i + 1 >= args.Length)
        {
            throw new ArgumentException($"{flag} requires a value");
        }
        return args[++i];
    }
}
