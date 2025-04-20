use std::{fs, path::Path, sync::Arc};

use miden_client::{
    builder::ClientBuilder,
    crypto::SecretKey,
    rpc::{Endpoint, TonicRpcClient},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use miden_clob_designs::common::{
    create_library, create_signature_check_account, delete_keystore_and_store,
    generate_advice_stack_from_signature,
};
use miden_crypto::{dsa::rpo_falcon512::Polynomial, hash::rpo::Rpo256 as Hasher, FieldElement};
use miden_objects::{assembly::Assembler, transaction::TransactionScript, vm::AdviceMap};
use tokio::time::Instant;

#[tokio::test]
async fn multi_signature_verify() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

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

    // -------------------------------------------------------------------------
    // STEP 0: Create signature check smart contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 0] create a smart contract");

    let signature_check_contract = create_signature_check_account(&mut client).await.unwrap();

    // -------------------------------------------------------------------------
    // STEP 1: Prepare the Script
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Prepare Script With Public Keys");

    let number_of_iterations = 10;

    // Read the account signature script template.
    let code =
        fs::read_to_string(Path::new("./masm/scripts/multi_sig_verify_script.masm")).unwrap();

    let script_code = code.replace(
        "{NUMBER_OF_ITERATIONS}",
        &(number_of_iterations - 1).to_string(),
    );

    let mut keys = Vec::new();

    let number_of_keys: usize = number_of_iterations;
    for i in 0..number_of_keys {
        let key = SecretKey::with_rng(client.rng());
        keys.push(key);

        let pub_key_word: Word = keys[i].public_key().into();

        println!("pub key #{:?}: {:?}", i, pub_key_word);
    }

    println!("Final script:\n{}", script_code);

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);
    let file_path = Path::new("./masm/accounts/account_signature_check.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let account_component_lib = create_library(
        assembler.clone(),
        "external_contract::signature_check_contract",
        &account_code,
    )
    .unwrap();

    let tx_script = TransactionScript::compile(
        script_code,
        [],
        assembler.with_library(&account_component_lib).unwrap(),
    )
    .unwrap();

    // -------------------------------------------------------------------------
    // STEP 2: Hash & Sign Data with Each Key and Populate the Advice Map
    // -------------------------------------------------------------------------

    // Prepare some data to hash.
    let mut data = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    data.splice(0..0, Word::default().iter().cloned());
    let hashed_data = Hasher::hash_elements(&data);
    println!("digest: {:?}", hashed_data);

    // Initialize an empty advice map.
    let mut advice_map = AdviceMap::default();

    let mut i = 0;
    for key in keys.iter() {
        let signature = key.sign(hashed_data.into());

        // Convert the signature into its polynomial representation.
        let s2: Polynomial<Felt> = signature.sig_poly().into();

        // Get the public key as a Word (which is 4 field elements).
        let pk: Word = key.public_key().into();

        // If needed by generate_advice_stack_from_signature, convert pk (Word) into a Polynomial.
        let h = key.compute_pub_key_poly().coefficients.clone();
        let h_poly: Polynomial<Felt> = Polynomial::new(h).into();

        // Generate the signature advice values.
        let advice_value_u64: Vec<u64> = generate_advice_stack_from_signature(h_poly, s2);
        let signature_advice: Vec<Felt> = advice_value_u64
            .into_iter()
            .map(|value| Felt::new(value))
            .collect();

        // Build the final advice vector by concatenating:
        // [public key (4 elements), hashed message (4 elements), signature advice values]
        let mut advice_value_felt: Vec<Felt> = Vec::new();
        advice_value_felt.extend(pk);
        advice_value_felt.extend(hashed_data.clone());
        advice_value_felt.extend(signature_advice);

        // Insert the final advice vector into the advice map.
        let advice_key: Word = [Felt::new(i), Felt::ZERO, Felt::ZERO, Felt::ZERO];
        advice_map.insert(advice_key.into(), advice_value_felt);
        i += 1;
    }

    let tx_increment_request = TransactionRequestBuilder::new()
        .with_custom_script(tx_script)
        .extend_advice_map(advice_map)
        .build()
        .unwrap();

    // BEGIN TIMING PROOF GENERATION
    let start = Instant::now();

    let tx_result = client
        .new_transaction(signature_check_contract.id(), tx_increment_request)
        .await
        .unwrap();

    // Calculate the elapsed time for proof generation
    let duration = start.elapsed();
    println!("multisig verify proof generation time: {:?}", duration);
    println!(
        "time per pub key recovery: {:?}",
        duration / number_of_keys.try_into().unwrap()
    );

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    let executed_tx: &miden_client::transaction::ExecutedTransaction =
        tx_result.executed_transaction();
    let total_cycles = executed_tx.measurements().total_cycles();

    println!("total cycles: {:?}", total_cycles);

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;

    // Calculate the time for complete onchain settlement
    let complete_settlement_time = start.elapsed();
    println!(
        "multisig verify tx settled in: {:?}",
        complete_settlement_time
    );
    println!(
        "time per pub key recovery: {:?}",
        complete_settlement_time / number_of_keys.try_into().unwrap()
    );

    client.sync_state().await.unwrap();

    Ok(())
}

#[tokio::test]
async fn multi_signature_benchmark_advice_provider_100() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

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

    // -------------------------------------------------------------------------
    // STEP 0: Create signature check smart contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 0] create a smart contract");

    let signature_check_contract = create_signature_check_account(&mut client).await.unwrap();

    // -------------------------------------------------------------------------
    // STEP 1: Prepare the Script
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Prepare Script With Public Keys");

    let number_of_iterations = 100;

    // Read the account signature script template.
    let code = fs::read_to_string(Path::new(
        "./masm/scripts/multi_sig_advice_provider_script.masm",
    ))
    .unwrap();

    let script_code = code.replace(
        "{NUMBER_OF_ITERATIONS}",
        &(number_of_iterations - 1).to_string(),
    );

    // Generate 10 unique keys.
    let unique_keys = 10;
    let total_keys = number_of_iterations;
    let mut unique_key_vec = Vec::with_capacity(unique_keys);
    for i in 0..unique_keys {
        let key = SecretKey::with_rng(client.rng());
        unique_key_vec.push(key);
        let pub_key_word: Word = unique_key_vec[i].public_key().into();
        println!("Unique pub key #{:?}: {:?}", i, pub_key_word);
    }

    // Reuse the 10 unique keys to populate the keys vector with a total of 100 keys.
    let mut keys = Vec::with_capacity(total_keys);
    for i in 0..total_keys {
        // Reuse by cycling through the unique keys.
        let key = unique_key_vec[i % unique_keys].clone();
        keys.push(key);
    }

    println!("Final script:\n{}", script_code);

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);
    let file_path = Path::new("./masm/accounts/account_signature_check.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let account_component_lib = create_library(
        assembler.clone(),
        "external_contract::signature_check_contract",
        &account_code,
    )
    .unwrap();

    let tx_script = TransactionScript::compile(
        script_code,
        [],
        assembler.with_library(&account_component_lib).unwrap(),
    )
    .unwrap();

    // -------------------------------------------------------------------------
    // STEP 2: Hash & Sign Data with Each Key and Populate the Advice Map
    // -------------------------------------------------------------------------

    // Prepare some data to hash
    let mut data = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    data.splice(0..0, Word::default().iter().cloned());
    let hashed_data = Hasher::hash_elements(&data);
    println!("digest: {:?}", hashed_data);

    // Initialize an empty advice map.
    let mut advice_map = AdviceMap::default();

    for (i, key) in keys.iter().enumerate() {
        let signature = key.sign(hashed_data.into());

        // Convert the signature into its polynomial representation.
        let s2: Polynomial<Felt> = signature.sig_poly().into();

        // Compute the public key polynomial and convert to a word.
        let h = key.compute_pub_key_poly().coefficients.clone();
        let h_poly = Polynomial::new(h).into();

        // Generate advice values from the signature.
        let advice_value_u64: Vec<u64> = generate_advice_stack_from_signature(h_poly, s2);
        let advice_value_felt: Vec<Felt> = advice_value_u64
            .into_iter()
            .map(|value| Felt::new(value))
            .collect();

        let advice_key: Word = [Felt::new(i as u64), Felt::ZERO, Felt::ZERO, Felt::ZERO];

        advice_map.insert(advice_key.into(), advice_value_felt);
    }

    let tx_increment_request = TransactionRequestBuilder::new()
        .with_custom_script(tx_script)
        .extend_advice_map(advice_map)
        .build()
        .unwrap();

    // BEGIN TIMING PROOF GENERATION
    let start = Instant::now();

    let tx_result = client
        .new_transaction(signature_check_contract.id(), tx_increment_request)
        .await
        .unwrap();

    // Calculate the elapsed time for proof generation
    let duration = start.elapsed();
    println!("multisig verify proof generation time: {:?}", duration);
    println!(
        "time per pub key recovery: {:?}",
        duration / total_keys.try_into().unwrap()
    );

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    let executed_tx: &miden_client::transaction::ExecutedTransaction =
        tx_result.executed_transaction();
    let total_cycles = executed_tx.measurements().total_cycles();

    println!("total cycles: {:?}", total_cycles);

    // println!("total cycles: {:?}", tx_result.executed_transaction().);

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;

    // Calculate the time for complete onchain settlement
    let complete_settlement_time = start.elapsed();
    println!(
        "multisig verify tx settled in: {:?}",
        complete_settlement_time
    );
    println!(
        "time per pub key recovery: {:?}",
        complete_settlement_time / total_keys.try_into().unwrap()
    );

    client.sync_state().await.unwrap();

    Ok(())
}
