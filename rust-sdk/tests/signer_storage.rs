use bitcoin::Network;
use bitcoin::bip32::Xpriv;
use lendaswap_sdk::Client;
use lendaswap_sdk::Signer;
use lendaswap_sdk::types::GetSwapResponse;
use serde_json::Value;
use serde_json::json;
use std::str::FromStr;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[tokio::test]
async fn tprv_import_reopens_records_stored_under_mnemonic() {
    let server = MockServer::start().await;
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    let swap: GetSwapResponse = serde_json::from_value(fixtures["evm_to_arkade"].clone()).unwrap();
    let mut response = serde_json::to_value(swap).unwrap();
    response["hash_lock"] = json!(hex::encode(signer.derive_swap_params(3).unwrap().hash_lock));
    response["index"] = json!(3);
    let id = response["id"].as_str().unwrap().to_owned();
    Mock::given(method("POST")).and(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"swaps":[response],"highest_index":3,"next_index":4,"scanned_until":10,"complete":true})))
        .mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swaps.db");
    {
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage_path(&db)
            .build()
            .unwrap();
        client.recover_all_swaps(None).await.unwrap();
    }
    let seed = bip39::Mnemonic::from_str(MNEMONIC).unwrap().to_seed("");
    let tprv = Xpriv::new_master(Network::Testnet, &seed)
        .unwrap()
        .to_string();
    assert!(tprv.starts_with("tprv"));
    let client = Client::builder()
        .base_url(server.uri())
        .xprv(tprv)
        .storage_path(&db)
        .build()
        .unwrap();
    assert_eq!(client.get_stored_swap(&id).unwrap().unwrap().key_index, 3);
}
