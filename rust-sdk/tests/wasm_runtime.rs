#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::Signer;
use lendaswap_sdk::types::EvmAsset;
use lendaswap_sdk::types::GetSwapResponse;
use lendaswap_sdk::types::LightningDestination;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::SwapCreation;
use lendaswap_sdk::types::SwapOptions;
use lendaswap_sdk::types::SwapStatus;
use lendaswap_sdk::types::TokenId;
use serde_json::Value;
use std::time::Duration;
use wasm_bindgen_test::wasm_bindgen_test;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[wasm_bindgen_test]
async fn lightning_evm_round_trip() {
    let base_url = option_env!("SATORA_WASM_TEST_URL")
        .expect("run python3 tests/run_wasm.py to start the fixture API");
    let client = Client::builder()
        .base_url(base_url)
        .mnemonic(MNEMONIC)
        .build()
        .unwrap();
    assert_ne!(Signer::generate(12).unwrap().mnemonic(), Some(MNEMONIC));

    let asset = EvmAsset {
        chain_id: 42161,
        token: TokenId::Usdt0Arbitrum,
    };
    let recipient = "0x3333333333333333333333333333333333333333";
    let buy = client
        .create_swap_request(
            SwapCreation::LightningToEvm {
                target: asset.clone(),
                amount: QuoteAmount::Source(1_000),
                target_address: recipient.into(),
            },
            SwapOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(buy.status, SwapStatus::Pending);
    assert!(matches!(
        client.refresh_swap(&buy.id).await.unwrap(),
        GetSwapResponse::LightningToEvm(_)
    ));

    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    let payout_invoice = fixtures["evm_to_lightning"]["client_lightning_invoice"]
        .as_str()
        .unwrap();
    let sell = client
        .create_swap_request(
            SwapCreation::EvmToLightning {
                source: asset,
                destination: LightningDestination::Invoice(payout_invoice.into()),
            },
            SwapOptions::default(),
        )
        .await
        .unwrap();
    let stored = client.get_stored_swap(&sell.id).unwrap().unwrap();
    assert_eq!(stored.key_index, 1);
    assert!(matches!(
        stored.response,
        GetSwapResponse::EvmToLightning(_)
    ));

    #[cfg(feature = "gasless")]
    funding::fund(&client, base_url, &sell.id).await;

    assert!(matches!(
        client
            .wait_for_swap_status(
                &sell.id,
                &[SwapStatus::ServerFunded],
                Duration::from_millis(100)
            )
            .await,
        Err(Error::Timeout(_))
    ));
    assert!(matches!(
        client
            .create_swap_request(
                SwapCreation::BitcoinToArkade {
                    sats_receive: 1_000,
                    target_address: String::new()
                },
                SwapOptions::default()
            )
            .await,
        Err(Error::InvalidSwap(_))
    ));
}

#[cfg(feature = "gasless")]
mod funding {
    use super::Client;
    use super::Duration;
    use super::GetSwapResponse;
    use super::MNEMONIC;
    use super::Signer;
    use alloy::sol;
    use alloy::sol_types::SolCall;
    use alloy::sol_types::SolEvent;
    use lendaswap_sdk::evm::Address;
    use lendaswap_sdk::evm::B256;
    use lendaswap_sdk::evm::Bytes;
    use lendaswap_sdk::evm::EvmLog;
    use lendaswap_sdk::evm::EvmReceipt;
    use lendaswap_sdk::evm::EvmTransaction;
    use lendaswap_sdk::evm::EvmTypedData;
    use lendaswap_sdk::evm::EvmWallet;
    use lendaswap_sdk::evm::FundingOptions;
    use lendaswap_sdk::evm::RpcWallet;
    use lendaswap_sdk::evm::Signature;
    use lendaswap_sdk::evm::U256;
    use lendaswap_sdk::evm::WalletFuture;
    use lendaswap_sdk::evm::htlc::EvmHtlc;
    use std::rc::Rc;

    sol! {
        event SwapCreated(bytes32 indexed preimageHash, address indexed refundAddress, address indexed claimAddress, address token, uint256 amount, uint256 timelock, bytes32 key);
    }

    struct TestWallet {
        rpc: Rc<RpcWallet>,
        lock: EvmHtlc,
        coordinator: Address,
    }

    impl EvmWallet for TestWallet {
        fn address(&self) -> Address {
            self.rpc.address()
        }

        fn chain_id(&self) -> u64 {
            self.rpc.chain_id()
        }

        fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
            self.rpc.sign_typed_data(data)
        }

        fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
            self.rpc.call(tx)
        }

        fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
            Box::pin(async move {
                assert_eq!(tx.to, self.coordinator);
                assert_eq!(
                    tx.data[..4],
                    lendaswap_sdk::aa::abi::IHTLCCoordinator::executeAndCreateWithPermit2Call::SELECTOR
                );
                Ok(B256::repeat_byte(0xaa))
            })
        }

        fn wait_for_receipt(&self, hash: B256) -> WalletFuture<'_, EvmReceipt> {
            Box::pin(async move {
                let log = SwapCreated {
                    preimageHash: self.lock.hash_lock,
                    refundAddress: self.lock.sender,
                    claimAddress: self.lock.claimer,
                    token: self.lock.token,
                    amount: self.lock.amount,
                    timelock: U256::from(self.lock.timelock),
                    key: self.lock.key(),
                }
                .encode_log_data();
                Ok(EvmReceipt {
                    transaction_hash: hash,
                    success: true,
                    block_number: 1,
                    logs: vec![EvmLog {
                        address: self.lock.address,
                        topics: log.topics().to_vec(),
                        data: log.data,
                    }],
                })
            })
        }
    }

    pub async fn fund(client: &Client, base_url: &str, id: &str) {
        let record = client.get_stored_swap(id).unwrap().unwrap();
        let GetSwapResponse::EvmToLightning(swap) = record.response else {
            panic!("expected an EVM to Lightning swap");
        };
        let chain_id = u64::try_from(swap.evm_chain_id).unwrap();
        let key = Signer::from_mnemonic(MNEMONIC)
            .unwrap()
            .derive_evm_key(record.key_index)
            .unwrap();
        let rpc = RpcWallet::connect(&key, chain_id, format!("{base_url}/rpc").parse().unwrap())
            .await
            .unwrap();
        let coordinator = Address::repeat_byte(0x44);
        let lock_token = Address::repeat_byte(0x55);
        let wallet = TestWallet {
            rpc: Rc::new(rpc),
            coordinator,
            lock: EvmHtlc {
                chain_id,
                address: swap.evm_htlc_address.parse().unwrap(),
                hash_lock: swap.hash_lock.parse().unwrap(),
                amount: U256::from(1000),
                token: lock_token,
                sender: swap.client_evm_address.parse().unwrap(),
                claimer: swap.server_evm_address.parse().unwrap(),
                timelock: swap.evm_refund_locktime.try_into().unwrap(),
            },
        };
        let receipt = client
            .fund_swap_with_wallet(
                id,
                &wallet,
                FundingOptions {
                    coordinator,
                    htlc: swap.evm_htlc_address.parse().unwrap(),
                    lock_token,
                    max_source_amount: U256::from(100_000),
                    allowed_call_targets: Vec::new(),
                    timeout: Duration::from_secs(10),
                },
            )
            .await
            .unwrap();
        assert_eq!(receipt.transaction_hash, B256::repeat_byte(0xaa));
        assert_eq!(
            client
                .get_stored_swap(id)
                .unwrap()
                .unwrap()
                .metadata
                .evm_fund_txid,
            Some(receipt.transaction_hash.to_string()),
        );
    }
}
