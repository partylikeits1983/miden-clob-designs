use std::{fs, path::Path, sync::Arc};

use rand::{Rng, RngCore};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

use miden_client::{
    account::{AccountStorageMode, AccountType},
    auth::AuthSecretKey,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    rpc::{Endpoint, TonicRpcClient},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_library, create_signature_check_account,
    generate_advice_stack_from_signature, reset_store_sqlite,
};
use miden_crypto::{dsa::rpo_falcon512::Polynomial, hash::rpo::Rpo256 as Hasher, rand::FeltRng};
use miden_objects::{
    account::{AccountBuilder, AccountComponent, StorageMap, StorageSlot},
    assembly::Assembler,
    transaction::TransactionScript,
    vm::AdviceMap,
};

#[tokio::test]
async fn account_signature_check() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    reset_store_sqlite().await;

    // Initialize client
    let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let (alice_account, alice_key) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Alice's account ID: {:?}", alice_account.id().to_hex());
    let (bob_account, bob_key) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Bob's account ID: {:?}", bob_account.id().to_hex());

    println!("\nDeploying a new fungible faucet.");
    let faucet = create_basic_faucet(&mut client, keystore.clone()).await?;
    println!("Faucet account ID: {:?}", faucet.id());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 1: Create signature check smart contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Deploy a smart contract with a mapping");

    let signature_check_contract = create_signature_check_account(&mut client).await.unwrap();

    // -------------------------------------------------------------------------
    // STEP 2: Call the Signature check contract with a Script
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Call Mapping Contract With Script");

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);
    let file_path = Path::new("./masm/accounts/account_signature_check.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let code =
        fs::read_to_string(Path::new("./masm/scripts/account_signature_script.masm")).unwrap();

    let bob_pub_key_word: Word = bob_key.public_key().into();

    let concatenated_pub_key = format!(
        "{}.{}.{}.{}",
        bob_pub_key_word[0], bob_pub_key_word[1], bob_pub_key_word[2], bob_pub_key_word[3]
    );
    let script_code = code.replace("{PUB_KEY}", &concatenated_pub_key);

    // Create the library from the account source code using the helper function.
    let account_component_lib = create_library(
        assembler.clone(),
        "external_contract::signature_check_contract",
        &account_code,
    )
    .unwrap();

    // Compile the transaction script with the library.
    let tx_script = TransactionScript::compile(
        script_code,
        [],
        assembler.with_library(&account_component_lib).unwrap(),
    )
    .unwrap();

    // -------------------------------------------------------------------------
    // STEP 3: hash & sign data
    // -------------------------------------------------------------------------

    let mut data = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    data.splice(0..0, Word::default().iter().cloned());
    let hashed_data = Hasher::hash_elements(&data);
    println!("digest: {:?}", hashed_data);

    // @dev make key pair used Alice's key pair
    let key_pair = bob_key;

    // let h = key_pair.public_key().;
    let h = key_pair.compute_pub_key_poly().coefficients.clone();
    let h_poly = Polynomial::new(h).into();

    let signature = key_pair.sign(hashed_data.into());
    let s2: Polynomial<Felt> = signature.sig_poly().into();

    // generate_data_probabilistic_product_test()
    let advice_value_u64: Vec<u64> = generate_advice_stack_from_signature(h_poly, s2);
    let advice_value_felt: Vec<Felt> = advice_value_u64
        .into_iter()
        .map(|value| Felt::new(value))
        .collect();

    let mut advice_map = AdviceMap::default();

    let advice_key: Word = key_pair.public_key().into();
    advice_map.insert(advice_key.into(), advice_value_felt);

    println!("pub key felts: {:?}", key_pair.public_key());

    // Build a transaction request with the custom script
    let tx_increment_request = TransactionRequestBuilder::new()
        .with_custom_script(tx_script)
        .extend_advice_map(advice_map)
        .build()
        .unwrap();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(signature_check_contract.id(), tx_increment_request)
        .await
        .unwrap();

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;

    client.sync_state().await.unwrap();

    let account = client
        .get_account(signature_check_contract.id())
        .await
        .unwrap();
    let index = 1;
    let key = [Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(0)];
    println!(
        "Mapping state\n Index: {:?}\n Key: {:?}\n Value: {:?}",
        index,
        key,
        account
            .unwrap()
            .account()
            .storage()
            .get_map_item(index, key)
    );

    Ok(())
}
