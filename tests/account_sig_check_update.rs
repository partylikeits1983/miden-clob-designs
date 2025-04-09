use std::{fs, path::Path, sync::Arc};

use miden_client::{
    builder::ClientBuilder,
    crypto::SecretKey,
    keystore::FilesystemKeyStore,
    rpc::{Endpoint, TonicRpcClient},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_library, create_multisig_poc,
    create_signature_check_account, delete_keystore_and_store,
    generate_advice_stack_from_signature,
};
use miden_crypto::{dsa::rpo_falcon512::Polynomial, hash::rpo::Rpo256 as Hasher, FieldElement};
use miden_objects::transaction::TransactionMeasurements;
use miden_objects::{assembly::Assembler, transaction::TransactionScript, vm::AdviceMap};
use tokio::time::Instant;

#[tokio::test]
async fn updated_signature_check_test() -> Result<(), ClientError> {
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
    // STEP 1: Prepare the Script
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Prepare Script With Public Keys");

    let number_of_iterations = 5;

    // Read the account signature script template.
    let code =
        fs::read_to_string(Path::new("./masm/scripts/updated_multisig_script.masm")).unwrap();

    let script_code = code.replace(
        "{NUMBER_OF_ITERATIONS}",
        &(number_of_iterations - 1).to_string(),
    );

    let mut keys = Vec::new();
    let mut pub_keys: Vec<Word> = Vec::new();

    let number_of_keys: usize = number_of_iterations;
    for i in 0..number_of_keys {
        let key = SecretKey::with_rng(client.rng());
        keys.push(key.clone());

        pub_keys.push(key.public_key().into());

        let pub_key_word: Word = keys[i].public_key().into();

        println!("pub key #{:?}: {:?}", i, pub_key_word);
    }

    println!("Final script:\n{}", script_code);

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);
    let file_path = Path::new("./masm/accounts/sig_check_update.masm");
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
    // STEP 0: Create signature check smart contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 0] create a smart contract");

    let signature_check_contract = create_multisig_poc(&mut client, pub_keys).await.unwrap();

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

        let nonce = signature.nonce().to_elements();
        let s2 = signature.sig_poly();
        let h = key.compute_pub_key_poly().0;
        let pi = Polynomial::mul_modulo_p(&h, s2);

        let mut polynomials: Vec<Felt> = h
            .coefficients
            .iter()
            .map(|a| Felt::from(a.value() as u32))
            .collect();
        polynomials.extend(s2.coefficients.iter().map(|a| Felt::from(a.value() as u32)));
        polynomials.extend(pi.iter().map(|a| Felt::new(*a)));

        let digest_polynomials = Hasher::hash_elements(&polynomials);
        let challenge = (digest_polynomials[0], digest_polynomials[1]);

        let pub_key_felts: Word = key.public_key().into();
        let msg_felts: Word = hashed_data.into();

        let mut result: Vec<Felt> = vec![
            pub_key_felts[0],
            pub_key_felts[1],
            pub_key_felts[2],
            pub_key_felts[3],
            msg_felts[0],
            msg_felts[1],
            msg_felts[2],
            msg_felts[3],
            challenge.0,
            challenge.1,
        ];

        result.extend_from_slice(&polynomials);
        result.extend_from_slice(&nonce);

        // Insert the final advice vector into the advice map.
        let advice_key: Word = [Felt::new(i), Felt::ZERO, Felt::ZERO, Felt::ZERO];
        advice_map.insert(advice_key.into(), result.clone());

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
