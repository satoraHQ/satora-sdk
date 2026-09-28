//! Recovery templates. Calls below execute only when explicitly invoked by the
//! embedding application; main does not connect or broadcast.
use lendaswap_sdk::Client;
use lendaswap_sdk::Result;
use lendaswap_sdk::arkade::ArkadeBackend;
use lendaswap_sdk::arkade::ArkadeSettlement;
use lendaswap_sdk::arkade::ArkadeSpend;
use lendaswap_sdk::arkade::ArkadeSpendOptions;
use lendaswap_sdk::bitcoin::BitcoinBackend;
use lendaswap_sdk::bitcoin::BitcoinSpendOptions;
use lendaswap_sdk::bitcoin::BitcoinSpendReceipt;

pub async fn recover_records(mnemonic: String, database: &str) -> Result<Client> {
    let client = Client::builder()
        .mnemonic(mnemonic)
        .storage_path(database)
        .build()?;
    // Recover server records and advance the key counter after database loss.
    // Existing local execution metadata is retained when snapshots are refreshed.
    client.recover_all_swaps(None).await?;
    Ok(client)
}
/// Call only after the application approves the refund. Repeating the same
/// operation/destination resumes the stored transaction instead of replacing it.
pub async fn refund_bitcoin(
    client: &Client,
    id: &str,
    chain: &dyn BitcoinBackend,
    options: BitcoinSpendOptions,
) -> Result<BitcoinSpendReceipt> {
    client.refund_bitcoin(id, chain, options).await
}
/// The same entry point handles claims, timeout refunds, cooperative refunds,
/// and restart recovery. A delegated path requires a pinned delegate cosigner.
pub async fn settle_arkade(
    client: &Client,
    id: &str,
    backend: &dyn ArkadeBackend,
    chain: &dyn BitcoinBackend,
    operation: ArkadeSpend,
    delegate: bitcoin::secp256k1::PublicKey,
    options: ArkadeSpendOptions,
) -> Result<ArkadeSettlement> {
    client
        .settle_arkade(id, backend, Some(chain), operation, Some(delegate), options)
        .await
}
#[cfg(feature = "gasless")]
pub mod evm {
    use super::*;
    use lendaswap_sdk::bridge::BridgeApi;
    use lendaswap_sdk::bridge::BridgeSubmission;
    use lendaswap_sdk::bridge::CctpDestination;
    use lendaswap_sdk::bridge::CctpInboundRecovered;
    use lendaswap_sdk::bridge::CctpInboundRecovery;
    use lendaswap_sdk::bridge::CctpMint;
    use lendaswap_sdk::bridge::CctpRecoveryAccount;
    use lendaswap_sdk::bridge::CctpSweep;
    use lendaswap_sdk::bridge::cctp::CctpBurn;
    use lendaswap_sdk::bridge::cctp::CctpSource;
    use lendaswap_sdk::control::PollOptions;
    use lendaswap_sdk::evm::B256;
    use lendaswap_sdk::evm::EvmReceipt;
    use lendaswap_sdk::evm::EvmWallet;
    use lendaswap_sdk::evm::FundingOptions;
    use lendaswap_sdk::evm::RefundOptions;

    /// Refresh the original swap, verify the refund/balance, and reuse a durable
    /// replacement ID if continuation was interrupted. Destination is explicit.
    pub async fn continue_refund(
        client: &Client,
        id: &str,
        wallet: &dyn EvmWallet,
        refund: &RefundOptions,
        destination: &str,
        funding: FundingOptions,
    ) -> Result<(String, EvmReceipt)> {
        client.refresh_swap(id).await?;
        client
            .continue_refunded_evm_swap(id, wallet, refund, destination, funding)
            .await
    }
    /// Recover a failed Circle forward. Reuse the verified burn terms and source
    /// receipt hash; never select an unrelated message from the same transaction.
    /// `None` means the nonce was already consumed, not a newly submitted mint.
    pub async fn recover_cctp(
        client: &Client,
        api: &BridgeApi,
        id: &str,
        wallet: &dyn EvmWallet,
        burn: &CctpBurn,
        source_tx: B256,
        poll: &PollOptions,
    ) -> Result<Option<EvmReceipt>> {
        let attestation = api.wait_cctp_attestation(source_tx, burn, poll).await?;
        let destination = CctpDestination::mainnet(wallet.chain_id())?;
        client
            .mint_cctp(
                id,
                wallet,
                CctpMint {
                    burn,
                    destination: &destination,
                    attestation: &attestation,
                },
                BridgeSubmission::Wallet,
                poll,
            )
            .await
    }
    /// Recover a stalled inbound burn from its source transaction alone, with no
    /// swap record. The seed key that received the mint signs; only this burn's net
    /// mint moves to `recipient`. IRIS can take minutes to attest.
    pub async fn recover_cctp_from_tx(
        client: &Client,
        api: &BridgeApi,
        source_chain_id: u64,
        source_rpc: url::Url,
        source_tx: B256,
        destination_rpc: url::Url,
        recipient: lendaswap_sdk::evm::Address,
    ) -> Result<CctpInboundRecovered> {
        let poll = PollOptions {
            interval: std::time::Duration::from_secs(10),
            timeout: std::time::Duration::from_secs(15 * 60),
            ..Default::default()
        };
        client
            .recover_cctp_inbound(
                api,
                CctpInboundRecovery {
                    source: CctpSource::mainnet(source_chain_id)?,
                    source_rpc,
                    source_txid: source_tx,
                    destination: CctpDestination::mainnet(42161)?,
                    recipient,
                    account: CctpRecoveryAccount::Local {
                        destination_rpc,
                        key_index: None,
                    },
                    submission: BridgeSubmission::Wallet,
                    sweep: CctpSweep::Minted,
                    attestation: None,
                },
                &poll,
            )
            .await
    }
}
fn main() {
    println!(
        "Recovery template: reopen the same seed/database, inspect execution journals, and reconcile existing submissions before retrying."
    );
}
