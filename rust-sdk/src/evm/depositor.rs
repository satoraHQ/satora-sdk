//! The per-swap depositor of an EVM-sourced swap: its balances, a sweep of what it
//! holds, and the HTLC refund it submits, each with the key this client derives.
use super::recovery::balance;
use super::refund::RefundOptions;
use super::relay::RelaySwap;
use super::*;
use crate::Client;
use crate::StoredSwap;
use crate::signer::EvmKey;
use crate::types::EvmHtlcKind;
use std::time::Duration;
use url::Url;

/// What a swap's depositor holds, from [`Client::depositor_balance`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositorBalance {
    /// In the token's smallest unit.
    pub token_units: U256,
    pub native_wei: U256,
}

/// A verified HTLC refund, from [`Client::refund_evm_htlc`].
#[derive(Clone, Debug)]
pub struct DepositorRefund {
    pub receipt: EvmReceipt,
    /// The token the HTLC locked, which the refund paid to the depositor; sweep it
    /// on with [`Client::sweep_evm_deposit`].
    pub lock_token: Address,
}

/// Refuses before anything is reserved when `wallet` cannot pay for `transaction`:
/// a broadcast the node turns down for want of gas would leave the reservation
/// unresolved.
pub(super) async fn ensure_gas(wallet: &dyn EvmWallet, transaction: &EvmTransaction) -> Result<()> {
    let held = wallet.native_balance().await?;
    // Checked before the estimate, which some nodes refuse for an account without gas.
    if held.is_zero() {
        return Err(Error::InsufficientGas(
            "the depositor holds none; send it gas first".into(),
        ));
    }
    let fee = wallet.transaction_fee(transaction).await?;
    if held < fee {
        return Err(Error::InsufficientGas(format!(
            "the depositor holds {held} wei and the transaction needs up to {fee} wei; send it gas first"
        )));
    }
    Ok(())
}

/// Connects `key` to `node_rpc_url`, or with `None` to the swap chain's default node.
pub(super) async fn connect(
    key: &EvmKey,
    context: &EvmContext,
    node_rpc_url: Option<&Url>,
) -> Result<RpcWallet> {
    let url = match node_rpc_url {
        Some(url) => url.clone(),
        None => {
            let default = crate::types::KnownChain::from_wire_str(&context.chain_id.to_string())
                .and_then(|chain| chain.default_node_rpc_url())
                .ok_or_else(|| invalid("swap chain has no default node RPC; set node_rpc_url"))?;
            Url::parse(default).map_err(|e| invalid(e.to_string()))?
        }
    };
    RpcWallet::connect(key, context.chain_id, url).await
}

/// The coordinator and lock token the stored swap was funded with. EVM to Arkade
/// responses name no lock token, so it comes from the relay funding journal.
fn stored_refund_options(stored: &StoredSwap, timeout: Duration) -> Result<RefundOptions> {
    let swap = RelaySwap::from_response(&stored.response)?;
    if stored
        .metadata
        .evm_coordinator_address
        .as_ref()
        .is_some_and(|pinned| !pinned.eq_ignore_ascii_case(&swap.coordinator.to_string()))
    {
        return Err(invalid(
            "coordinator differs from the one persisted for this swap",
        ));
    }
    let lock_token = match (
        swap.context.kind,
        swap.lock_token,
        &stored.metadata.evm_fund_relay,
    ) {
        (EvmHtlcKind::Native, _, _) => Address::ZERO,
        (_, Some(token), _) => token,
        (_, None, Some(journal)) => parse(&journal.lock_token)?,
        (_, None, None) => {
            return Err(invalid(
                "the swap's lock token is unknown; refund it with refund_evm_with_wallet",
            ));
        }
    };
    Ok(RefundOptions {
        htlc: swap.context.htlc,
        lock_token,
        coordinator: Some(swap.coordinator),
        timeout,
    })
}

impl Client {
    pub(super) fn depositor_key(
        &self,
        stored: &StoredSwap,
        context: &EvmContext,
    ) -> Result<EvmKey> {
        self.signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("the depositor key requires the swap seed".into()))?
            .evm_key_for_address(stored.key_index, &context.owner.to_string())
    }

    fn stored_source(&self, id: &str) -> Result<(StoredSwap, EvmContext)> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let context = EvmContext::from_response(&stored.response)?;
        if !context.is_source {
            return Err(invalid("only an EVM-sourced swap has a depositor"));
        }
        Ok((stored, context))
    }

    /// The depositor of a stored EVM-sourced swap as a local wallet: the key this
    /// client derives for its `client_evm_address`, connected to `node_rpc_url`,
    /// or with `None` to the chain's default node. A depositor this client holds
    /// no key for fails with [`Error::InvalidSigner`].
    pub async fn depositor_wallet(
        &self,
        id: &str,
        node_rpc_url: Option<&Url>,
    ) -> Result<RpcWallet> {
        let (stored, context) = self.stored_source(id)?;
        connect(
            &self.depositor_key(&stored, &context)?,
            &context,
            node_rpc_url,
        )
        .await
    }

    /// The depositor's balance of `token`, the swap's source token with `None`,
    /// and of native gas. Reads only, so it is safe to repeat after any error.
    pub async fn depositor_balance(
        &self,
        id: &str,
        token: Option<Address>,
        node_rpc_url: Option<&Url>,
    ) -> Result<DepositorBalance> {
        let token = match token {
            Some(token) => token,
            None => self.stored_source(id)?.1.source_token,
        };
        let wallet = self.depositor_wallet(id, node_rpc_url).await?;
        Ok(DepositorBalance {
            token_units: balance(&wallet, token).await?,
            native_wei: wallet.native_balance().await?,
        })
    }

    /// Send the depositor's whole balance of `token`, the swap's source token
    /// with `None`, to `destination`. The depositor pays the gas, so it must hold
    /// native gas first: with less than the transfer's estimated fee the call
    /// fails with [`Error::InsufficientGas`] before anything is reserved.
    ///
    /// It refuses with [`Error::UnresolvedFunding`] while a relay funding
    /// signature for the swap can still pull the deposit, as
    /// [`Self::recover_evm_deposit_with_wallet`] describes. The transfer is
    /// reserved with its amount before it is sent and its hash recorded after,
    /// so a repeated call verifies a recorded transfer instead of sending another,
    /// and returns the settled receipt once the balance it moved is gone. A
    /// transfer that mined and reverted moved nothing and is released with
    /// [`Error::InvalidSwap`]; the next call sends a new one from the balance
    /// then held. [`Error::Transport`], [`Error::Timeout`] and [`Error::Storage`]
    /// can leave the outcome unknown; call again. A transfer reserved without a
    /// recorded hash is sent again by the next call with the same token and
    /// destination, from the balance then held, since a second whole-balance
    /// transfer to the same destination moves what is there or reverts. Once that
    /// balance is gone the reservation is released with [`Error::InvalidSwap`].
    /// Until then a sweep of another token or to another destination fails with
    /// [`Error::InvalidSwap`] naming the sweep to repeat.
    pub async fn sweep_evm_deposit(
        &self,
        id: &str,
        token: Option<Address>,
        destination: Address,
        node_rpc_url: Option<&Url>,
        timeout: Duration,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(timeout, async {
            let wallet = self.depositor_wallet(id, node_rpc_url).await?;
            self.sweep_with_wallet(id, &wallet, token, destination)
                .await
        })
        .await
        .map_err(|_| {
            Error::Timeout("deposit sweep timed out; call it again to verify or resume it".into())
        })?
    }

    pub(super) async fn sweep_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        token: Option<Address>,
        destination: Address,
    ) -> Result<EvmReceipt> {
        let token = match token {
            Some(token) => token,
            None => self.stored_source(id)?.1.source_token,
        };
        self.recover_deposit(id, wallet, token, destination, None)
            .await
    }

    /// Refund the swap's funded EVM HTLC through its coordinator's `refundTo`,
    /// which pays the lock token back to the depositor, from the depositor
    /// itself, and return the receipt with the lock token. The coordinator comes
    /// from the stored swap and the lock token from it or, for EVM to Arkade,
    /// from the relay funding journal. The funding is verified from its receipt
    /// first. Before chain time reaches the refund locktime the call fails with
    /// [`Error::InvalidSwap`], and when the depositor holds less native gas than
    /// the refund's estimated fee with [`Error::InsufficientGas`], both before
    /// anything is reserved.
    ///
    /// The refund is reserved before it is sent and its hash recorded after, so a
    /// repeated call verifies the recorded refund instead of sending another. A
    /// refund that mined and reverted refunded nothing: it is released with
    /// [`Error::InvalidSwap`], and the next call sends a new one while the HTLC
    /// is active. [`Error::Transport`], [`Error::Timeout`] and [`Error::Storage`]
    /// can leave the outcome unknown; call again. A refund reserved without a
    /// recorded hash is sent again while the HTLC is active, since a second
    /// `refundTo` of the same deposit reverts. Once the HTLC is no longer active,
    /// or the coordinator no longer holds its deposit, the call fails with
    /// [`Error::InvalidSwap`]: that refund or a claim settled it, so check the
    /// depositor's balance of the lock token.
    pub async fn refund_evm_htlc(
        &self,
        id: &str,
        node_rpc_url: Option<&Url>,
        timeout: Duration,
    ) -> Result<DepositorRefund> {
        tokio::time::timeout(timeout, async {
            let wallet = self.depositor_wallet(id, node_rpc_url).await?;
            self.refund_htlc_with_wallet(id, &wallet, timeout).await
        })
        .await
        .map_err(|_| {
            Error::Timeout("HTLC refund timed out; call it again to verify or resume it".into())
        })?
    }

    pub(super) async fn refund_htlc_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        timeout: Duration,
    ) -> Result<DepositorRefund> {
        let (stored, _) = self.stored_source(id)?;
        let options = stored_refund_options(&stored, timeout)?;
        let lock_token = options.lock_token;
        Ok(DepositorRefund {
            receipt: self.refund_evm(id, wallet, options, true).await?,
            lock_token,
        })
    }
}
