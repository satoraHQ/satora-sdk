//! Collaborative exit of the client's own Arkade wallet to an on-chain address.
use super::delegate::unix_now;
use super::execution::decode;
use super::execution::invalid;
use super::*;
use crate::client::WALLET_OFFBOARD_TIMEOUT;
use ark_rs::core::server::FeeInfo;
use ark_rs::core::server::VirtualTxOutPoint;

/// What an offboard pays on chain, and the server's fee for it, which comes out of
/// change on top of the amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffboardQuote {
    pub amount_sats: u64,
    pub fee_sats: u64,
}

/// ark-client 0.11's collaborative_redeem joins a batch up to four times (three
/// retries) with the same inputs.
const BATCH_JOIN_ATTEMPTS: u64 = 4;
/// How long ark-client 0.11 keeps an intent registered (`expire_at` in batch.rs).
const INTENT_VALIDITY_SECS: u64 = 120;
/// backon's default backoff between ark-client's join attempts: 1, 2 and 4 seconds.
const BATCH_JOIN_BACKOFF_SECS: u64 = 1 + 2 + 4;

/// How long before its expiry a VTXO stops being an offboard input, as one that
/// expires while the batch is pending fails it for every participant. Each join
/// attempt can wait out its intent's validity and then a session for the batch to
/// finish, but the call gives up at [`WALLET_OFFBOARD_TIMEOUT`]. On a server whose
/// VTXOs live minutes (regtest: 512 s) that would leave nothing to offboard, so the
/// margin stays within half the VTXO lifetime, and a retried offboard there can still
/// meet an expiring input.
fn expiry_margin(session_duration: u64, vtxo_lifetime: Option<u64>) -> u64 {
    let attempts = INTENT_VALIDITY_SECS
        .saturating_add(session_duration)
        .saturating_mul(BATCH_JOIN_ATTEMPTS)
        .saturating_add(BATCH_JOIN_BACKOFF_SECS);
    let margin = attempts.min(WALLET_OFFBOARD_TIMEOUT.as_secs());
    vtxo_lifetime.map_or(margin, |lifetime| margin.min(lifetime / 2))
}

/// The lifetime arkd gives a batch's VTXOs, which ark-core 0.11's `server::Info` does
/// not carry: arkd dates a confirmed VTXO at its batch and expires it one lifetime
/// later. Pre-confirmed VTXOs inherit an earlier expiry, so they would understate it.
fn vtxo_lifetime<'a>(vtxos: impl Iterator<Item = &'a VirtualTxOutPoint>) -> Option<u64> {
    vtxos
        .filter(|vtxo| !vtxo.is_preconfirmed)
        .filter_map(|vtxo| u64::try_from(vtxo.expires_at.checked_sub(vtxo.created_at)?).ok())
        .max()
}

/// Sats an offboard cannot spend, which a refusal for want of balance names.
#[derive(Debug, Default, Clone, Copy)]
struct LeftOut {
    /// Recoverable sats, which only `arkade_settle` spends.
    recoverable: u64,
    /// Sats in VTXOs that expire within `margin` seconds.
    expiring: u64,
    margin: u64,
}

impl LeftOut {
    fn hint(&self, dust: u64) -> String {
        let mut hint = String::new();
        if self.expiring > 0 {
            hint.push_str(&format!(
                "; {} sats expire within {} s and can be settled with arkade_settle once expired",
                self.expiring, self.margin
            ));
        }
        // ark-client refuses to settle recoverable VTXOs worth less than dust in total.
        if self.recoverable >= dust {
            hint.push_str(&format!(
                "; settle the {} recoverable sats with arkade_settle first",
                self.recoverable
            ));
        } else if self.recoverable > 0 {
            hint.push_str(&format!(
                "; the {} recoverable sats are too small to settle on their own",
                self.recoverable
            ));
        }
        hint
    }
}

/// ark-client 0.11's private `build_fee_estimator`, the estimator collaborative_redeem
/// prices its on-chain output with.
fn fee_estimator(fees: Option<FeeInfo>) -> Result<ark_fees::Estimator> {
    let config = fees
        .map(|fees| ark_fees::Config {
            intent_offchain_input_program: fees.intent_fee.offchain_input.unwrap_or_default(),
            intent_onchain_input_program: fees.intent_fee.onchain_input.unwrap_or_default(),
            intent_offchain_output_program: fees.intent_fee.offchain_output.unwrap_or_default(),
            intent_onchain_output_program: fees.intent_fee.onchain_output.unwrap_or_default(),
        })
        .unwrap_or_default();
    ark_fees::Estimator::new(config).map_err(decode)
}

/// Check an offboard of `amount_sats` from inputs worth `total`, or of everything when
/// `None`, and price it as collaborative_redeem does: the on-chain output's fee comes
/// out of change, so the address receives the amount exactly. The change is never
/// below dust, so sending everything takes the largest amount that leaves at least
/// dust; an amount-dependent fee program can skip the amount that leaves exactly
/// dust, and then less than dust plus one fee step stays behind. A refusal for want
/// of balance names what `left_out` holds.
fn quote_offboard(
    address: &str,
    network: Network,
    estimator: &ark_fees::Estimator,
    dust: u64,
    total: u64,
    left_out: &LeftOut,
    amount_sats: Option<u64>,
) -> Result<(Address, OffboardQuote)> {
    let address = Address::from_str(address)
        .map_err(|e| invalid(format!("invalid Bitcoin address `{address}`: {e}")))?
        .require_network(network)
        .map_err(|e| invalid(format!("offboard address: {e}")))?;
    // The same string collaborative_redeem passes: the script's asm, not its hex.
    let script = address.script_pubkey().to_string();
    let fee = |amount: u64| {
        estimator
            .eval_onchain_output(ark_fees::Output {
                amount,
                script: script.clone(),
            })
            .map(|fee| fee.to_satoshis())
            .map_err(decode)
    };
    let hint = left_out.hint(dust);
    // ark-client 0.11 registers a cosigner key for every intent but signs the VTXO tree
    // only when the intent has an offchain output (prepare_intent and join_next_batch
    // in batch.rs). An exit without change therefore never sends the nonces arkd waits
    // for, and arkd fails any batch it shares with outputs that create VTXOs ("collected
    // 1/2 nonces"). The TypeScript SDK registers a signer only for offchain outputs.
    // ark-client recomputes the change from the requested outpoints it still finds
    // unspent, so this holds only while nothing outside this client spends them.
    let change_floor = dust.max(1);
    let budget = total.saturating_sub(change_floor);
    let (amount, fee) = match amount_sats {
        Some(amount) => (amount, fee(amount)?),
        // The search assumes the fee never falls as the amount grows; otherwise it may
        // settle for a smaller amount, which the checks below still validate.
        None => {
            let fits = |amount: u64| -> Result<bool> {
                Ok(amount
                    .checked_add(fee(amount)?)
                    .is_some_and(|spent| spent <= budget))
            };
            let (mut low, mut high) = (0, budget);
            while low < high {
                let mid = high - (high - low) / 2;
                if fits(mid)? {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            (low, fee(low)?)
        }
    };
    if amount < dust {
        return Err(invalid(match amount_sats {
            Some(_) => format!("offboard amount {amount} is below the {dust} sat dust limit"),
            None => format!(
                "offboard amount {amount} from a spendable balance of {total} sats, less the {dust} sats kept as Arkade change, is below the {dust} sat dust limit{hint}"
            ),
        }));
    }
    let needed = amount
        .checked_add(fee)
        .and_then(|spent| spent.checked_add(change_floor));
    if needed.is_none_or(|needed| needed > total) {
        return Err(invalid(format!(
            "insufficient spendable Arkade balance: {total} < {amount} (send) + {fee} (fee) + {dust} (change) sats, as the wallet must keep at least {dust} sats as Arkade change after an exit{hint}"
        )));
    }
    Ok((
        address,
        OffboardQuote {
            amount_sats: amount,
            fee_sats: fee,
        },
    ))
}

/// The inputs that cover `quote` and at least `dust` of change: the soonest-expiring
/// VTXOs first, then the largest, as the TypeScript SDK's `selectVirtualCoins` orders
/// them, so an amount one VTXO covers exactly brings in another for the change.
fn select_inputs<'a>(
    mut eligible: Vec<&'a VirtualTxOutPoint>,
    quote: &OffboardQuote,
    dust: u64,
) -> Vec<&'a VirtualTxOutPoint> {
    let needed = quote
        .amount_sats
        .saturating_add(quote.fee_sats)
        .saturating_add(dust.max(1));
    eligible.sort_by_key(|vtxo| (vtxo.expires_at, std::cmp::Reverse(vtxo.amount)));
    let mut covered = 0u64;
    eligible
        .into_iter()
        .take_while(|vtxo| {
            let short = covered < needed;
            covered = covered.saturating_add(vtxo.amount.to_sat());
            short
        })
        .collect()
}

/// ark-client 0.11's collaborative_redeem pays only the on-chain output's fee, so arkd
/// would refuse an intent whose inputs or change its fee program also charges. Inputs
/// are priced as the TypeScript SDK's `toOffchainInputFeeParams` does.
fn refuse_unpaid_fees(
    estimator: &ark_fees::Estimator,
    inputs: &[&VirtualTxOutPoint],
    change: u64,
    change_script: &str,
) -> Result<()> {
    let mut input_fee = ark_fees::FeeAmount::default();
    for vtxo in inputs {
        input_fee += estimator
            .eval_offchain_input(ark_fees::OffchainInput {
                amount: vtxo.amount.to_sat(),
                expiry: Some(vtxo.expires_at),
                birth: Some(vtxo.created_at),
                input_type: if vtxo.is_swept {
                    ark_fees::VtxoType::Recoverable
                } else {
                    ark_fees::VtxoType::Vtxo
                },
                weight: 0.0,
            })
            .map_err(decode)?;
    }
    let change_fee = if change > 0 {
        estimator
            .eval_offchain_output(ark_fees::Output {
                amount: change,
                script: change_script.to_owned(),
            })
            .map_err(decode)?
    } else {
        ark_fees::FeeAmount::default()
    };
    for (fee, what) in [(input_fee, "inputs"), (change_fee, "change")] {
        let fee = fee.to_satoshis();
        if fee > 0 {
            return Err(invalid(format!(
                "the server charges {fee} sats for the offboard's {what}, which a collaborative exit does not pay"
            )));
        }
    }
    Ok(())
}

/// The VTXOs and total an offboard can spend, and the sats it leaves out because they
/// expire within `margin` seconds of `now`: confirmed and pre-confirmed ones only.
/// ark-core expects a forfeit for every input at or above dust that arkd has not
/// swept, but arkd builds connectors only for those it considers healthy, so one
/// expired-but-unswept VTXO fails the batch for every participant. Recoverable VTXOs
/// are left for `arkade_settle`, and so are those that expire within the margin,
/// which could expire while the batch is pending (or on an arkd clock running ahead).
/// Asset-carrying ones stay offchain, as in VHTLC spends.
fn offboard_inputs<'a>(
    vtxos: impl Iterator<Item = &'a VirtualTxOutPoint>,
    dust: Amount,
    now: i64,
    margin: u64,
) -> Result<(Vec<&'a VirtualTxOutPoint>, u64, u64)> {
    let expires_after = now.saturating_add(i64::try_from(margin).unwrap_or(i64::MAX));
    let mut inputs = Vec::new();
    let (mut total, mut expiring) = (0u64, 0u64);
    for vtxo in vtxos.filter(|vtxo| vtxo.is_spendable_offchain(dust) && vtxo.assets.is_empty()) {
        if vtxo.expires_at <= expires_after {
            expiring = expiring.saturating_add(vtxo.amount.to_sat());
            continue;
        }
        inputs.push(vtxo);
        total = total
            .checked_add(vtxo.amount.to_sat())
            .ok_or_else(|| invalid("Arkade balance overflow"))?;
    }
    Ok((inputs, total, expiring))
}

impl ArkadeWallet {
    /// Destination, inputs and quote of an offboard. The inputs come from the
    /// wallet's spendable VTXOs without assets that do not expire soon; recoverable
    /// VTXOs and boarding outputs are left out.
    async fn prepare_offboard(
        &self,
        address: &str,
        amount_sats: Option<u64>,
    ) -> Result<(Address, Vec<OutPoint>, OffboardQuote)> {
        // collaborative_redeem pays change to this address.
        let change_address = ArkAddress::decode(&self.offchain_address().await?).map_err(decode)?;
        let info = self
            .client
            .server_info()
            .await
            .map_err(|e| Error::Transport(format!("server_info: {e}")))?;
        let vtxos = self
            .client
            .list_vtxos()
            .await
            .map_err(|e| Error::Transport(format!("list_vtxos: {e}")))?;
        let now = i64::try_from(unix_now()?).map_err(decode)?;
        let margin = expiry_margin(
            info.session_duration,
            vtxo_lifetime(vtxos.all().map(|entry| entry.vtxo())),
        );
        let (inputs, total, expiring) = offboard_inputs(
            vtxos
                .spendable_offchain_at(&info, now)
                .map(|entry| entry.vtxo()),
            info.dust,
            now,
            margin,
        )?;
        let left_out = LeftOut {
            recoverable: vtxos
                .recoverable()
                .map(|entry| entry.vtxo().amount.to_sat())
                .fold(0, u64::saturating_add),
            expiring,
            margin,
        };
        let estimator = fee_estimator(info.fees.clone())?;
        let dust = info.dust.to_sat();
        let (address, quote) = quote_offboard(
            address,
            info.network,
            &estimator,
            dust,
            total,
            &left_out,
            amount_sats,
        )?;
        let inputs = select_inputs(inputs, &quote, dust);
        refuse_unpaid_fees(
            &estimator,
            &inputs,
            inputs
                .iter()
                .map(|vtxo| vtxo.amount.to_sat())
                .fold(0, u64::saturating_add)
                .saturating_sub(quote.amount_sats)
                .saturating_sub(quote.fee_sats),
            // The hex ark-fees documents; only the on-chain output follows
            // collaborative_redeem's asm.
            &change_address.to_p2tr_script_pubkey().to_hex_string(),
        )?;
        Ok((
            address,
            inputs.iter().map(|vtxo| vtxo.outpoint).collect(),
            quote,
        ))
    }

    pub(crate) async fn offboard_quote(
        &self,
        address: &str,
        amount_sats: Option<u64>,
    ) -> Result<OffboardQuote> {
        Ok(self.prepare_offboard(address, amount_sats).await?.2)
    }

    pub(crate) async fn offboard(&self, address: &str, amount_sats: Option<u64>) -> Result<Txid> {
        let (address, inputs, quote) = self.prepare_offboard(address, amount_sats).await?;
        let mut rng = rand::rngs::OsRng;
        self.client
            .collaborative_redeem_vtxo_selection(
                &mut rng,
                inputs.into_iter(),
                address,
                Amount::from_sat(quote.amount_sats),
            )
            .await
            .map_err(|e| Error::Transport(format!("offboard: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_rs::core::server::IntentFeeInfo;

    const REGTEST: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
    const DUST: u64 = 330;

    fn estimator(onchain_output: &str) -> ark_fees::Estimator {
        fee_estimator(Some(FeeInfo {
            intent_fee: IntentFeeInfo {
                // A program that must not price the on-chain output.
                offchain_input: Some("999999.0".into()),
                onchain_output: Some(onchain_output.into()),
                ..Default::default()
            },
            tx_fee_rate: String::new(),
        }))
        .unwrap()
    }

    fn quote_leaving_out(
        program: &str,
        total: u64,
        left_out: LeftOut,
        amount: Option<u64>,
    ) -> Result<OffboardQuote> {
        quote_offboard(
            REGTEST,
            Network::Regtest,
            &estimator(program),
            DUST,
            total,
            &left_out,
            amount,
        )
        .map(|(_, quote)| quote)
    }

    fn quote(program: &str, total: u64, amount: Option<u64>) -> Result<OffboardQuote> {
        quote_leaving_out(program, total, LeftOut::default(), amount)
    }

    fn rejection_leaving_out(
        program: &str,
        total: u64,
        left_out: LeftOut,
        amount: Option<u64>,
    ) -> String {
        match quote_leaving_out(program, total, left_out, amount) {
            Err(Error::InvalidSwap(message)) => message,
            other => panic!("expected a pre-flight rejection, got {other:?}"),
        }
    }

    fn rejection(program: &str, total: u64, amount: Option<u64>) -> String {
        rejection_leaving_out(program, total, LeftOut::default(), amount)
    }

    fn recoverable(sats: u64) -> LeftOut {
        LeftOut {
            recoverable: sats,
            ..Default::default()
        }
    }

    fn vtxo(vout: u32, sats: u64, expires_at: i64) -> VirtualTxOutPoint {
        VirtualTxOutPoint {
            outpoint: OutPoint::new(Txid::from_byte_array([4; 32]), vout),
            created_at: 0,
            expires_at,
            amount: Amount::from_sat(sats),
            script: bitcoin::ScriptBuf::new(),
            is_preconfirmed: false,
            is_swept: false,
            is_unrolled: false,
            is_spent: false,
            spent_by: None,
            commitment_txids: vec![],
            settled_by: None,
            ark_txid: None,
            assets: vec![],
            depth: 0,
        }
    }

    #[test]
    fn a_fixed_amount_arrives_whole_and_the_fee_comes_out_of_change() {
        assert_eq!(
            quote("200.0", 100_000, Some(50_000)).unwrap(),
            OffboardQuote {
                amount_sats: 50_000,
                fee_sats: 200
            }
        );
        assert_eq!(
            quote("amount * 0.01 + 100.0", 100_000, Some(50_000)).unwrap(),
            OffboardQuote {
                amount_sats: 50_000,
                fee_sats: 600
            }
        );
        assert_eq!(
            quote("200.0", 100_000, Some(99_470)).unwrap().amount_sats,
            99_470
        );
    }

    #[test]
    fn sending_everything_keeps_dust_as_arkade_change() {
        assert_eq!(
            quote("200.0", 100_000, None).unwrap(),
            OffboardQuote {
                amount_sats: 99_470,
                fee_sats: 200
            }
        );
        // ceil(0.01 * 98_584 + 100) = 1_086, and 98_584 + 1_086 + 330 = 100_000.
        assert_eq!(
            quote("amount * 0.01 + 100.0", 100_000, None).unwrap(),
            OffboardQuote {
                amount_sats: 98_584,
                fee_sats: 1_086
            }
        );
    }

    #[test]
    fn sending_everything_keeps_more_than_dust_when_the_fee_program_skips_the_amount() {
        // a + ceil(a / 2) skips 670 (446 + 223 = 669, 447 + 224 = 671), so 331 sats
        // stay behind.
        assert_eq!(
            quote("amount * 0.5", 1_000, None).unwrap(),
            OffboardQuote {
                amount_sats: 446,
                fee_sats: 223
            }
        );
        // 329 + 165 + 330 = 824, and nothing at or above dust leaves dust behind.
        assert!(rejection("amount * 0.5", 824, None).contains("below the 330 sat dust"));

        // a + ceil(0.01 * a + 200) skips 101 * k + 201 for every k.
        let program = "amount * 0.01 + 200.0";
        let fees = estimator(program);
        let script = Address::from_str(REGTEST)
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_string();
        let eval = |amount| {
            fees.eval_onchain_output(ark_fees::Output {
                amount,
                script: script.clone(),
            })
            .unwrap()
            .to_satoshis()
        };
        let table: Vec<_> = (0..3_000).map(eval).collect();
        let fee = |amount: u64| {
            usize::try_from(amount)
                .ok()
                .and_then(|index| table.get(index).copied())
                .unwrap_or_else(|| eval(amount))
        };
        let skipped = |k: u64| 101 * k + 201 + DUST;
        let totals = (DUST..3_000).chain([100, 1_000, 10_000].map(skipped));
        let mut kept_more = 0;
        for total in totals {
            let change = |amount: u64| total.checked_sub(amount + fee(amount));
            let valid = |amount| change(amount).is_some_and(|c| c >= DUST);
            let best = (DUST..=total).rev().find(|amount| valid(*amount));
            match (quote(program, total, None), best) {
                (Ok(quote), Some(best)) => {
                    assert_eq!((quote.amount_sats, quote.fee_sats), (best, fee(best)));
                    if change(best) != Some(DUST) {
                        kept_more += 1;
                    }
                }
                (Err(Error::InvalidSwap(_)), None) => {}
                (result, best) => panic!("total {total}: {result:?}, but {best:?} is valid"),
            }
        }
        // k = 4 to 24 below 3_000, as smaller skipped totals have nothing at or above
        // dust to send, and the three large ones.
        assert_eq!(kept_more, 24);
    }

    #[test]
    fn a_fixed_amount_that_leaves_less_than_dust_is_refused() {
        // Exactly the balance, then 100 sats short of keeping dust.
        for amount in [99_800, 99_700] {
            let message = rejection("200.0", 100_000, Some(amount));
            assert!(
                message.contains("must keep at least 330 sats as Arkade change"),
                "{message}"
            );
        }
        assert!(quote("200.0", 100_000, Some(99_470)).is_ok());
    }

    #[test]
    fn an_amount_beyond_the_balance_or_below_dust_is_refused() {
        let message = rejection("200.0", 100_000, Some(99_801));
        assert!(
            message.contains("insufficient spendable Arkade balance"),
            "{message}"
        );
        for total in [0, 100, 859] {
            let message = rejection("200.0", total, None);
            assert!(
                message.contains(&format!("spendable balance of {total} sats"))
                    && message.contains("below the 330 sat dust"),
                "{message}"
            );
            assert!(!message.contains("arkade_settle"), "{message}");
        }
        assert!(quote("200.0", 860, None).is_ok());
        assert!(rejection("0.0", 100_000, Some(DUST - 1)).contains("below the 330 sat dust"));
    }

    #[test]
    fn a_short_spendable_balance_names_what_was_left_out() {
        let settle = "settle the 134955 recoverable sats with arkade_settle first";
        let expiring =
            "60000 sats expire within 256 s and can be settled with arkade_settle once expired";
        let both = LeftOut {
            recoverable: 134_955,
            expiring: 60_000,
            margin: 256,
        };
        for (program, total, amount) in [
            ("200.0", 0, Some(20_000)),
            ("200.0", 0, None),
            ("0.0", 0, None),
            ("0.0", 100, None),
            ("0.0", 20_000, Some(20_000)),
        ] {
            let message = rejection_leaving_out(program, total, both, amount);
            assert!(
                message.contains("spendable")
                    && message.contains(settle)
                    && message.contains(expiring),
                "{program} {total} {amount:?}: {message}"
            );
        }
        // Below dust, ark-client cannot settle the recoverable sats on their own.
        let message = rejection_leaving_out("0.0", 0, recoverable(DUST - 1), None);
        assert!(
            message.contains("the 329 recoverable sats are too small to settle on their own")
                && !message.contains("arkade_settle"),
            "{message}"
        );
        assert!(
            rejection_leaving_out("0.0", 0, recoverable(DUST), None).contains("settle the 330")
        );
        // An amount below dust is the caller's, whatever the balance.
        assert!(
            !rejection_leaving_out("0.0", 100_000, both, Some(DUST - 1)).contains("arkade_settle")
        );
        assert!(quote_leaving_out("200.0", 100_000, both, None).is_ok());
    }

    #[test]
    fn the_expiry_margin_covers_the_join_attempts_within_the_call_timeout() {
        // Four attempts of a 120 s intent plus a session, and 7 s of backoff.
        assert_eq!(expiry_margin(10, None), 527);
        // The call itself gives up after 600 s.
        assert_eq!(expiry_margin(30, None), 600);
        assert_eq!(expiry_margin(60, Some(30 * 24 * 3600)), 600);
        // Regtest VTXOs live 512 s.
        assert_eq!(expiry_margin(30, Some(512)), 256);
        assert_eq!(expiry_margin(10, Some(1_100)), 527);
    }

    #[test]
    fn the_vtxo_lifetime_comes_from_confirmed_vtxos_only() {
        let confirmed = |created_at, expires_at| VirtualTxOutPoint {
            created_at,
            ..vtxo(0, 1_000, expires_at)
        };
        let mut claim = confirmed(1_790_968_720, 1_790_968_802);
        claim.is_preconfirmed = true;
        assert_eq!(vtxo_lifetime([&claim].into_iter()), None);
        let batch = confirmed(1_790_969_044, 1_790_969_556);
        assert_eq!(vtxo_lifetime([&claim, &batch].into_iter()), Some(512));
    }

    #[test]
    fn only_spendable_vtxos_without_assets_outside_the_margin_are_offboard_inputs() {
        let asset = ark_rs::core::server::Asset {
            asset_id: ark_rs::core::asset::AssetId {
                txid: Txid::from_byte_array([8; 32]),
                group_index: 0,
            },
            amount: 1,
        };
        let with_asset = VirtualTxOutPoint {
            assets: vec![asset],
            ..vtxo(1, 1_000, i64::MAX)
        };
        let mut pre_confirmed = vtxo(3, 5_000, i64::MAX);
        pre_confirmed.is_preconfirmed = true;
        // Expired but not yet swept: ark-core would want a forfeit for it that arkd
        // builds no connector for.
        let expired = vtxo(4, 4_955, 1);
        let mut swept = vtxo(5, 4_000, i64::MAX);
        swept.is_swept = true;
        let mut spent = vtxo(7, 2_000, i64::MAX);
        spent.is_spent = true;
        let mut unrolled = vtxo(8, 1_500, i64::MAX);
        unrolled.is_unrolled = true;
        let now = 2_000_000_000;
        let vtxos = [
            vtxo(0, 30_000, i64::MAX),
            with_asset,
            vtxo(2, 20_000, i64::MAX),
            pre_confirmed,
            expired,
            swept,
            vtxo(6, DUST - 1, i64::MAX),
            spent,
            unrolled,
            // Each side of a regtest margin, then of a mainnet one.
            vtxo(9, 3_000, now + 256),
            vtxo(10, 7_000, now + 257),
            vtxo(11, 600, now + 600),
            vtxo(12, 800, now + 601),
        ];
        let regtest = expiry_margin(30, Some(512));
        let (inputs, total, expiring) =
            offboard_inputs(vtxos.iter(), Amount::from_sat(DUST), now, regtest).unwrap();
        assert_eq!(
            inputs,
            [0, 2, 3, 10, 11, 12].map(|index| &vtxos[index]).to_vec()
        );
        assert_eq!((total, expiring), (63_400, 3_000));
        let mainnet = expiry_margin(60, None);
        let (inputs, total, expiring) =
            offboard_inputs(vtxos.iter(), Amount::from_sat(DUST), now, mainnet).unwrap();
        assert_eq!(inputs, [0, 2, 3, 12].map(|index| &vtxos[index]).to_vec());
        assert_eq!((total, expiring), (55_800, 10_600));
    }

    #[test]
    fn an_amount_one_vtxo_covers_exactly_brings_in_another_for_the_change() {
        // The largest VTXO alone would cover the amount and the change, but it expires
        // last.
        let vtxos = [
            vtxo(0, 20_000, 3_000),
            vtxo(1, 50_000, 1_000),
            vtxo(2, 30_000, 2_000),
            vtxo(3, 60_000, 4_000),
        ];
        let exact = quote("200.0", 100_000, Some(49_800)).unwrap();
        assert_eq!(
            select_inputs(vtxos.iter().collect(), &exact, DUST),
            vec![&vtxos[1], &vtxos[2]]
        );
        // With room for the change, the soonest-expiring VTXO is enough.
        let roomy = quote("200.0", 100_000, Some(49_470)).unwrap();
        assert_eq!(
            select_inputs(vtxos.iter().collect(), &roomy, DUST),
            vec![&vtxos[1]]
        );
        // The same VTXO alone cannot keep dust, so that offboard is refused.
        assert!(
            rejection("200.0", 50_000, Some(49_800))
                .contains("must keep at least 330 sats as Arkade change")
        );
    }

    #[test]
    fn sending_everything_spends_every_input() {
        let vtxos = [vtxo(0, 60_000, 1_000), vtxo(1, 40_000, 2_000)];
        let everything = quote("amount * 0.01 + 100.0", 100_000, None).unwrap();
        assert_eq!(
            select_inputs(vtxos.iter().collect(), &everything, DUST),
            vec![&vtxos[0], &vtxos[1]]
        );
    }

    #[test]
    fn fees_on_the_inputs_or_the_change_are_refused() {
        let estimator = |offchain_input: &str, offchain_output: &str| {
            fee_estimator(Some(FeeInfo {
                intent_fee: IntentFeeInfo {
                    offchain_input: Some(offchain_input.into()),
                    offchain_output: Some(offchain_output.into()),
                    onchain_output: Some("200.0".into()),
                    ..Default::default()
                },
                tx_fee_rate: String::new(),
            }))
            .unwrap()
        };
        let healthy = vtxo(0, 50_000, i64::MAX);
        let mut swept = vtxo(0, 50_000, i64::MAX);
        swept.is_swept = true;
        let check = |estimator: &ark_fees::Estimator, input: &VirtualTxOutPoint, change| {
            refuse_unpaid_fees(estimator, &[input], change, "")
        };
        let free = estimator("0.0", "0.0");
        assert!(check(&free, &swept, 1_000).is_ok());
        let rejected = check(&estimator("amount * 0.01", "0.0"), &healthy, 0).unwrap_err();
        assert!(
            rejected
                .to_string()
                .contains("500 sats for the offboard's inputs")
        );
        let recoverable_only = estimator("inputType == 'recoverable' ? 100.0 : 0.0", "0.0");
        assert!(check(&recoverable_only, &healthy, 0).is_ok());
        assert!(matches!(
            check(&recoverable_only, &swept, 0),
            Err(Error::InvalidSwap(_))
        ));
        // The change output is priced only when there is change.
        let per_output = estimator("0.0", "100.0");
        assert!(check(&per_output, &healthy, 0).is_ok());
        let rejected = check(&per_output, &healthy, 1_000).unwrap_err();
        assert!(
            rejected
                .to_string()
                .contains("100 sats for the offboard's change")
        );
    }

    #[test]
    fn an_address_of_another_network_is_refused() {
        let mainnet = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let offboard = |network| {
            quote_offboard(
                mainnet,
                network,
                &estimator("0.0"),
                DUST,
                100_000,
                &LeftOut::default(),
                None,
            )
        };
        assert!(matches!(
            offboard(Network::Regtest),
            Err(Error::InvalidSwap(_))
        ));
        assert!(offboard(Network::Bitcoin).is_ok());
    }
}
