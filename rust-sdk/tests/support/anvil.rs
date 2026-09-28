//! Isolated real EVM fixture. Only the quote API is stubbed; contracts, signing,
//! transaction execution, receipts, balances and observations use a fresh Anvil.
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use lendaswap_sdk::Client;
use lendaswap_sdk::Signer;
use lendaswap_sdk::SqliteSwapStorage;
use lendaswap_sdk::StoredSwap;
use lendaswap_sdk::SwapMetadata;
use lendaswap_sdk::SwapStorage;
use lendaswap_sdk::evm::Address;
use lendaswap_sdk::evm::B256;
use lendaswap_sdk::evm::EvmClaimOptions;
use lendaswap_sdk::evm::RpcWallet;
use lendaswap_sdk::evm::U256;
use serde_json::Value;
use serde_json::json;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

pub const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
pub const CHAIN: u64 = 137;
pub const AMOUNT: u64 = 100_000;
pub const ID: &str = "00000000-0000-0000-0000-000000000001";
pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
sol! {
    interface Token {
        function approve(address spender, uint256 amount) external returns (bool);
        function transfer(address to, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
    }
    interface Lock {
        function create(bytes32 preimageHash, uint256 amount, address token, address claimAddress, uint256 timelock) external;
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub struct Fixture {
    _process: Process,
    pub directory: tempfile::TempDir,
    pub url: url::Url,
    http: reqwest::Client,
    pub deployer: Address,
    pub owner: Address,
    pub htlc: Address,
    pub coordinator: Address,
    pub token: Address,
    pub hash: B256,
    pub timelock: u64,
    pub wallet: RpcWallet,
    pub client: Client,
    pub store: Arc<SqliteSwapStorage>,
}
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts")
}
fn artifact(relative: &str) -> TestResult<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/e2e-contracts")
        .join(relative);
    let json: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|e| {
        format!(
            "{}: {e}; run scripts/e2e/test-rust-sdk-local.sh",
            path.display()
        )
    })?)?;
    Ok(hex::decode(
        json["bytecode"]["object"]
            .as_str()
            .ok_or("missing contract bytecode")?
            .trim_start_matches("0x"),
    )?)
}
impl Fixture {
    pub async fn new() -> TestResult<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let process = Process(
            Command::new(std::env::var_os("ANVIL_BIN").unwrap_or_else(|| "anvil".into()))
                .args([
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                    "--chain-id",
                    &CHAIN.to_string(),
                    "--silent",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("start Anvil: {e}; install Foundry first"))?,
        );
        let url: url::Url = format!("http://127.0.0.1:{port}").parse()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if rpc(&http, &url, "eth_chainId", json!([])).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await?;
        let accounts = rpc(&http, &url, "eth_accounts", json!([])).await?;
        let deployer: Address = accounts[0]
            .as_str()
            .ok_or("missing Anvil account")?
            .parse()?;
        let signer = Signer::from_mnemonic(MNEMONIC)?;
        let key = signer.derive_evm_key(0)?;
        let owner = key.address.parse()?;
        rpc(
            &http,
            &url,
            "anvil_setBalance",
            json!([owner, "0x56BC75E2D63100000"]),
        )
        .await?;
        let wallet = RpcWallet::connect(&key, CHAIN, url.clone()).await?;
        let deploy = |path: &'static str, arguments: Vec<u8>| {
            deploy_contract(&http, &url, deployer, path, arguments)
        };
        let htlc = deploy("HTLCErc20.sol/HTLCErc20.json", (deployer,).abi_encode()).await?;
        let token = deploy("HTLCCoordinatorSwapAndLock.t.sol/MockWBTC.json", vec![]).await?;
        let permit2: Address = lendaswap_sdk::aa::permit2::PERMIT2_ADDRESS;
        // Use the canonical runtime shipped by upstream's DeployPermit2 helper.
        // Its domain cache is for chain 1; chain 137 recomputes the correct domain.
        let helper =
            std::fs::read_to_string(root().join("lib/permit2/test/utils/DeployPermit2.sol"))?;
        let runtime = helper
            .split("hex\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .ok_or("missing canonical Permit2 bytecode")?;
        rpc(
            &http,
            &url,
            "anvil_setCode",
            json!([permit2, format!("0x{runtime}")]),
        )
        .await?;
        let coordinator = deploy(
            "HTLCCoordinator.sol/HTLCCoordinator.json",
            (htlc, permit2).abi_encode(),
        )
        .await?;
        let block = rpc(
            &http,
            &url,
            "eth_getBlockByNumber",
            json!(["latest", false]),
        )
        .await?;
        let timelock = u64::from_str_radix(
            block["timestamp"]
                .as_str()
                .ok_or("missing timestamp")?
                .trim_start_matches("0x"),
            16,
        )? + 3600;
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SqliteSwapStorage::open(
            directory.path().join("swaps.sqlite"),
            "e2e",
        )?);
        let client = Client::builder()
            .base_url("http://127.0.0.1:1")
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()?;
        Ok(Self {
            _process: process,
            directory,
            url,
            http,
            deployer,
            owner,
            htlc,
            coordinator,
            token,
            hash: signer.derive_swap_params(0)?.hash_lock.into(),
            timelock,
            wallet,
            client,
            store,
        })
    }
    pub fn reopen(&self, base_url: &str) -> TestResult<Client> {
        Ok(Client::builder()
            .base_url(base_url)
            .mnemonic(MNEMONIC)
            .storage(Arc::new(SqliteSwapStorage::open(
                self.directory.path().join("swaps.sqlite"),
                "e2e",
            )?))
            .build()?)
    }
    pub async fn rpc(&self, method: &str, params: Value) -> TestResult<Value> {
        rpc(&self.http, &self.url, method, params).await
    }
    pub async fn deploy(&self, path: &str, arguments: Vec<u8>) -> TestResult<Address> {
        deploy_contract(&self.http, &self.url, self.deployer, path, arguments).await
    }
    pub async fn transact(
        &self,
        from: Address,
        to: Address,
        call: impl SolCall,
    ) -> TestResult<B256> {
        self.transact_value(from, to, call, U256::ZERO).await
    }
    pub async fn transact_value(
        &self,
        from: Address,
        to: Address,
        call: impl SolCall,
        value: U256,
    ) -> TestResult<B256> {
        let receipt = send(
            &self.http,
            &self.url,
            json!({
                "from": from,
                "to": to,
                "value": format!("{value:#x}"),
                "data": format!("0x{}", hex::encode(call.abi_encode())),
                "gas": "0x989680"
            }),
        )
        .await?;
        Ok(receipt["transactionHash"]
            .as_str()
            .ok_or("missing hash")?
            .parse()?)
    }
    pub async fn balance(&self, address: Address) -> TestResult<U256> {
        let data = Token::balanceOfCall { account: address }.abi_encode();
        self.word(self.token, data).await
    }
    pub async fn word(&self, to: Address, data: Vec<u8>) -> TestResult<U256> {
        let value = self
            .rpc(
                "eth_call",
                json!([{"to":to,"data":format!("0x{}",hex::encode(data))},"latest"]),
            )
            .await?;
        Ok(U256::from_be_slice(&hex::decode(
            value
                .as_str()
                .ok_or("missing call result")?
                .trim_start_matches("0x"),
        )?))
    }
    pub fn save_swap(&self, source: bool, fund: Option<B256>) -> TestResult<()> {
        let fixtures: Value =
            serde_json::from_str(include_str!("../fixtures/swap-responses.json"))?;
        let mut value = fixtures[if source {
            "evm_to_arkade"
        } else {
            "arkade_to_evm"
        }]
        .clone();
        for (key, val) in [
            ("id", json!(ID)),
            ("evm_chain_id", json!(CHAIN)),
            ("client_evm_address", json!(self.owner)),
            ("server_evm_address", json!(self.deployer)),
            ("evm_htlc_address", json!(self.htlc)),
            ("evm_coordinator_address", json!(self.coordinator)),
            ("hash_lock", json!(self.hash)),
            ("evm_refund_locktime", json!(self.timelock)),
            ("evm_htlc_version", json!(6)),
            ("wbtc_address", json!(self.token)),
            ("evm_expected_sats", json!(AMOUNT.to_string())),
        ] {
            value[key] = val;
        }
        value["source_token"]["token_id"] = json!(self.token);
        value["target_token"]["token_id"] = json!(self.token);
        self.store.store_swap(StoredSwap::new(
            0,
            serde_json::from_value(value)?,
            SwapMetadata {
                evm_fund_txid: fund.map(|h| h.to_string()),
                ..Default::default()
            },
        )?)?;
        Ok(())
    }
    pub fn claim_options(&self) -> EvmClaimOptions {
        EvmClaimOptions {
            htlc: self.htlc,
            coordinator: self.coordinator,
            destination: self.owner,
            sweep_token: self.token,
            min_amount_out: U256::from(AMOUNT),
            minimum_remaining_seconds: 300,
            timeout: Duration::from_secs(20),
        }
    }
    pub async fn fund_target(&self) -> TestResult<B256> {
        self.transact(
            self.deployer,
            self.token,
            Token::approveCall {
                spender: self.htlc,
                amount: U256::from(AMOUNT),
            },
        )
        .await?;
        self.transact(
            self.deployer,
            self.htlc,
            Lock::createCall {
                preimageHash: self.hash,
                amount: U256::from(AMOUNT),
                token: self.token,
                claimAddress: self.owner,
                timelock: U256::from(self.timelock),
            },
        )
        .await
    }
}
async fn deploy_contract(
    http: &reqwest::Client,
    url: &url::Url,
    deployer: Address,
    path: &str,
    arguments: Vec<u8>,
) -> TestResult<Address> {
    let mut bytes = artifact(path)?;
    bytes.extend(arguments);
    let receipt = send(
        http,
        url,
        json!({
            "from": deployer,
            "data": format!("0x{}", hex::encode(bytes)),
            "gas": "0x989680"
        }),
    )
    .await?;
    Ok(receipt["contractAddress"]
        .as_str()
        .ok_or("missing contract address")?
        .parse()?)
}
async fn rpc(
    http: &reqwest::Client,
    url: &url::Url,
    method: &str,
    params: Value,
) -> TestResult<Value> {
    let value: Value = http
        .post(url.clone())
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(error) = value.get("error") {
        return Err(format!("{method}: {error}").into());
    }
    Ok(value
        .get("result")
        .ok_or("RPC response missing result")?
        .clone())
}
pub async fn send(http: &reqwest::Client, url: &url::Url, transaction: Value) -> TestResult<Value> {
    let hash = rpc(http, url, "eth_sendTransaction", json!([transaction])).await?;
    let receipt = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let receipt = rpc(http, url, "eth_getTransactionReceipt", json!([hash])).await?;
            if !receipt.is_null() {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(receipt);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    if receipt["status"] != "0x1" {
        return Err(format!("EVM transaction failed: {receipt}").into());
    }
    Ok(receipt)
}
