use std::{fs, path::Path};

use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

use miden_client::{
    account::{AccountStorageMode, AccountType},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt,
};

use miden_objects::{
    account::{AccountBuilder, AccountComponent, StorageMap, StorageSlot},
    assembly::Assembler,
    transaction::TransactionScript,
};

use miden_clob_designs::common::{create_library, get_new_pk_and_authenticator, initialize_client};

#[tokio::test]
async fn mapping_example_test() -> Result<(), ClientError> {
    // -------------------------------------------------------------------------
    // Initialize the Miden client
    // -------------------------------------------------------------------------
    let mut client = initialize_client().await?;
    println!("Client initialized successfully.");

    // Fetch and display the latest synchronized block number from the node.
    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    // -------------------------------------------------------------------------
    // STEP 1: Deploy a smart contract with a mapping
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Deploy a smart contract with a mapping");

    // Load the MASM file for the counter contract
    let file_path = Path::new("./masm/accounts/mapping_example_contract.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    // Prepare assembler (debug mode = true)
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    // Using an empty storage value in slot 0 since this is usually resurved
    // for the account pub_key and metadata
    let empty_storage_slot = StorageSlot::empty_value();

    // initialize storage map
    let storage_map = StorageMap::new();
    let storage_slot_map = StorageSlot::Map(storage_map.clone());

    // Compile the account code into `AccountComponent` with one storage slot
    let mapping_contract_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![empty_storage_slot, storage_slot_map],
    )
    .unwrap()
    .with_supports_all_types();

    // Init seed for the counter contract
    let init_seed = ChaCha20Rng::from_entropy().gen();

    // Anchor block of the account
    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    // Build the new `Account` with the component
    let (mapping_example_contract, _seed) = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(mapping_contract_component.clone())
        .build()
        .unwrap();

    let (_, auth_secret_key) = get_new_pk_and_authenticator();

    client
        .add_account(
            &mapping_example_contract.clone(),
            Some(_seed),
            &auth_secret_key,
            false,
        )
        .await
        .unwrap();

    // -------------------------------------------------------------------------
    // STEP 2: Call the Mapping Contract with a Script
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Call Mapping Contract With Script");

    let script_code =
        fs::read_to_string(Path::new("./masm/scripts/mapping_example_script.masm")).unwrap();

    // Create the library from the account source code using the helper function.
    let account_component_lib = create_library(
        assembler.clone(),
        "miden_by_example::mapping_example_contract",
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

    // Build a transaction request with the custom script
    let tx_increment_request = TransactionRequestBuilder::new()
        .with_custom_script(tx_script)
        .unwrap()
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(mapping_example_contract.id(), tx_increment_request)
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
        .get_account(mapping_example_contract.id())
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
