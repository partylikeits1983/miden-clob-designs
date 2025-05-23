use std::time::Instant;

use miden_client::{
    asset::FungibleAsset,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::NoteType,
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use miden_crypto::{hash::rpo::Rpo256 as Hasher, rand::FeltRng};
use miden_objects::crypto::hash::rpo::Rpo256;
use miden_objects::vm::AdviceMap;

use miden_clob_designs::common::{
    compute_partial_swapp, create_p2id_note, create_partial_swap_note,
    create_partial_swap_note_cancellable, create_partial_swap_private_note,
    delete_keystore_and_store, get_p2id_serial_num, get_swapp_note, setup_accounts_and_faucets,
};

#[tokio::test]
async fn swap_note_partial_consume_public_test() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

    // Initialize client
    let _endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let endpoint = Endpoint::testnet();

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

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Create SWAPP note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    // offered asset amount
    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    // requested asset amount
    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    let swapp_note_id = swapp_note.id();

    // Time from after SWAPP creation
    let start_time = Instant::now();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 2: Partial Consume SWAPP note
    // -------------------------------------------------------------------------
    let fill_amount_bob = 25;
    let (_amount_a_1, new_amount_a, new_amount_b) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    // leftover portion of Alice’s original order
    let swapp_note_1 = create_partial_swap_note(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_amount_a)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_amount_b)
            .unwrap()
            .into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // P2ID note for Bob’s partial fill going to Alice
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);

    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    client.sync_state().await?;

    // pass in amount to fill via note args
    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), Some(consume_amount_note_args))])
        .with_expected_output_notes(vec![p2id_note, swapp_note_1])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());

    let _ = client.submit_transaction(tx_result).await;

    // Stop timing
    let duration = start_time.elapsed();
    println!("SWAPP note partially filled");
    println!("Time from SWAPP creation to partial fill: {:?}", duration);

    Ok(())
}

#[tokio::test]
async fn swap_note_partial_consume_private_test() -> Result<(), ClientError> {
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

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Create SWAPP note (unchanged from your original code)
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    // offered asset amount
    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    // requested asset amount
    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_private_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    let swapp_note_id = swapp_note.id();

    // Time from after SWAPP creation
    let start_time = Instant::now();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 2: Partial Consume SWAPP note
    // -------------------------------------------------------------------------
    let fill_amount_bob = 25;
    let (_amount_a_1, new_amount_a, new_amount_b) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    // leftover portion of Alice’s original order
    let swapp_note_1 = create_partial_swap_private_note(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_amount_a)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_amount_b)
            .unwrap()
            .into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // P2ID note for Bob’s partial fill going to Alice
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);

    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Private,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    client.sync_state().await?;

    // pass in amount to fill via note args
    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), Some(consume_amount_note_args))])
        .with_expected_output_notes(vec![p2id_note, swapp_note_1])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());

    let _ = client.submit_transaction(tx_result).await;

    // Stop timing
    let duration = start_time.elapsed();
    println!("SWAPP note partially filled");
    println!("Time from SWAPP creation to partial fill: {:?}", duration);

    Ok(())
}

#[tokio::test]
async fn swap_note_reclaim_public_test() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

    // Initialize client
    let _endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let endpoint = Endpoint::testnet();

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

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Create SWAPP note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    // offered asset amount
    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    // requested asset amount
    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    let swapp_note_id = swapp_note.id();

    // Time from after SWAPP creation
    let start_time = Instant::now();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 2: Reclaim SWAPP note
    // -------------------------------------------------------------------------

    println!(
        "alice account id: {:?} {:?}",
        alice_account.id().prefix(),
        alice_account.id().suffix()
    );

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), None)])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(alice_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());

    let _ = client.submit_transaction(tx_result).await;

    // Stop timing
    let duration = start_time.elapsed();
    println!("SWAPP note partially filled");
    println!("Time from SWAPP creation to partial fill: {:?}", duration);

    Ok(())
}

#[tokio::test]
async fn partial_swap_chain_public_optimistic_benchmark() -> Result<(), ClientError> {
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

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // Alice
        vec![0, 100], // Bob
        vec![0, 100], // Charlie
    ];

    // This returns `(Vec<Account>, Vec<Account>)` => (accounts, faucets)
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let charlie_account = accounts[2].clone();

    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Alice creates a SWAPP note: 50 A → 50 B
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note for Alice: 50 A -> 50 B");

    let amount_a = 50; // offered A
    let amount_b = 50; // requested B

    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(), // “Seller” side
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    client.submit_transaction(tx_result).await?;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Bob partially consumes swapp_note (optimistically)
    // -------------------------------------------------------------------------
    println!("\n[STEP 4] Bob partially fills SWAPP note with 25 B");
    let start_time_bob = Instant::now(); // measure Bob's fill time

    let fill_amount_bob: u64 = 25;

    // Bob has 25 B => fill ratio => 25 B in => 25 A out
    // leftover => 25 A, 25 B
    let (_amount_out_offered, new_offered_asset_amount, new_requested_asset_amount) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    // leftover portion -> swapp_note_1
    let swapp_note_1 = create_partial_swap_note(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_offered_asset_amount)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_requested_asset_amount)
            .unwrap()
            .into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // p2id note for Bob’s partial fill => 25 B in for Alice
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);
    let p2id_note_1 = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    // Pass Bob's partial fill args
    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    // Use optimistic consumption (no proof needed from note side)
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note, Some(consume_amount_note_args))])
        .with_expected_output_notes(vec![p2id_note_1.clone(), swapp_note_1.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Bob consumed swapp_note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    client.submit_transaction(tx_result).await?;

    let duration_bob = start_time_bob.elapsed();
    println!("SWAPP note partially filled by Bob");
    println!("Time for Bob’s partial fill: {duration_bob:?}");

    // -------------------------------------------------------------------------
    // STEP 3: Charlie partially consumes swapp_note_1 (optimistically)
    // -------------------------------------------------------------------------
    println!("\n[STEP 5] Charlie partially fills swapp_note_1 with 10 B");
    let start_time_charlie = Instant::now(); // measure Charlie's fill time

    let fill_amount_charlie = 10;

    // leftover from step 4 => leftover_a_1, leftover_b_1
    // Charlie has 10 B => partial fill => 10 A out, leftover => 15 A, 15 B
    let (_a_out_2, new_offered_asset_amount_1, new_requested_asset_amount_1) =
        compute_partial_swapp(
            new_offered_asset_amount,
            new_requested_asset_amount,
            fill_amount_charlie,
        );

    let swap_serial_num_2 = [
        swap_serial_num_1[0],
        swap_serial_num_1[1],
        swap_serial_num_1[2],
        Felt::new(swap_serial_num_1[3].as_int() + 1),
    ];
    let swap_count_2 = swap_count_1 + 1;

    let swapp_note_2 = create_partial_swap_note(
        alice_account.id(),
        charlie_account.id(),
        FungibleAsset::new(faucet_a.id(), new_offered_asset_amount_1)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_requested_asset_amount_1)
            .unwrap()
            .into(),
        swap_serial_num_2,
        swap_count_2,
    )
    .unwrap();

    let p2id_note_asset_2 = FungibleAsset::new(faucet_b.id(), fill_amount_charlie).unwrap();
    let p2id_serial_num_2 = get_p2id_serial_num(swap_serial_num_1, swap_count_2);
    let p2id_note_2 = create_p2id_note(
        charlie_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_2.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_2,
    )
    .unwrap();

    let consume_amount_note_args_charlie = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_charlie),
    ];
    let consume_custom_req_charlie = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note_1, Some(consume_amount_note_args_charlie))])
        .with_expected_output_notes(vec![p2id_note_2.clone(), swapp_note_2.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(charlie_account.id(), consume_custom_req_charlie)
        .await
        .unwrap();

    println!(
        "Charlie consumed swapp_note_1 Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    client.submit_transaction(tx_result).await?;

    let duration_charlie = start_time_charlie.elapsed();
    println!("SWAPP note partially filled by Charlie");
    println!("Time for Charlie’s partial fill: {duration_charlie:?}");

    println!(
        "SWAPP note leftover after Charlie’s partial fill => A: {}, B: {}",
        new_offered_asset_amount_1, new_requested_asset_amount_1
    );
    println!("Done with partial swap ephemeral chain test.");

    Ok(())
}

#[tokio::test]
async fn partial_swap_chain_private_optimistic_benchmark() -> Result<(), ClientError> {
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

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // Alice
        vec![0, 100], // Bob
        vec![0, 100], // Charlie
    ];

    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let charlie_account = accounts[2].clone();

    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Alice creates a SWAPP note: 50 A → 50 B
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note for Alice: 50 A -> 50 B");

    let amount_a = 50; // offered A
    let amount_b = 50; // requested B

    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_private_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    client.submit_transaction(tx_result).await?;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Bob partially consumes swapp_note (optimistically)
    // -------------------------------------------------------------------------
    println!("\n[STEP 4] Bob partially fills SWAPP note with 25 B");
    let start_time_bob = Instant::now(); // measure Bob's fill time

    let fill_amount_bob: u64 = 25;
    let (_amount_out_offered, new_offered_asset_amount, new_requested_asset_amount) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    let swapp_note_1 = create_partial_swap_private_note(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_offered_asset_amount)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_requested_asset_amount)
            .unwrap()
            .into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);
    let p2id_note_1 = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Private,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note, Some(consume_amount_note_args))])
        .with_expected_output_notes(vec![p2id_note_1.clone(), swapp_note_1.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Bob consumed swapp_note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    client.submit_transaction(tx_result).await?;

    let duration_bob = start_time_bob.elapsed();
    println!("SWAPP note partially filled by Bob");
    println!("Time for Bob’s partial fill: {duration_bob:?}");

    // -------------------------------------------------------------------------
    // STEP 3: Charlie partially consumes swapp_note_1 (optimistically)
    // -------------------------------------------------------------------------
    println!("\n[STEP 5] Charlie partially fills swapp_note_1 with 10 B");
    let start_time_charlie = Instant::now(); // measure Charlie's fill time

    let fill_amount_charlie = 10;

    let (_a_out_2, new_offered_asset_amount_1, new_requested_asset_amount_1) =
        compute_partial_swapp(
            new_offered_asset_amount,
            new_requested_asset_amount,
            fill_amount_charlie,
        );

    let swap_serial_num_2 = [
        swap_serial_num_1[0],
        swap_serial_num_1[1],
        swap_serial_num_1[2],
        Felt::new(swap_serial_num_1[3].as_int() + 1),
    ];
    let swap_count_2 = swap_count_1 + 1;

    let swapp_note_2 = create_partial_swap_private_note(
        alice_account.id(),
        charlie_account.id(),
        FungibleAsset::new(faucet_a.id(), new_offered_asset_amount_1)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_requested_asset_amount_1)
            .unwrap()
            .into(),
        swap_serial_num_2,
        swap_count_2,
    )
    .unwrap();

    let p2id_note_asset_2 = FungibleAsset::new(faucet_b.id(), fill_amount_charlie).unwrap();
    let p2id_serial_num_2 = get_p2id_serial_num(swap_serial_num_1, swap_count_2);
    let p2id_note_2 = create_p2id_note(
        charlie_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_2.into()],
        NoteType::Private,
        Felt::new(0),
        p2id_serial_num_2,
    )
    .unwrap();

    let consume_amount_note_args_charlie = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_charlie),
    ];
    let consume_custom_req_charlie = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note_1, Some(consume_amount_note_args_charlie))])
        .with_expected_output_notes(vec![p2id_note_2.clone(), swapp_note_2.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(charlie_account.id(), consume_custom_req_charlie)
        .await
        .unwrap();

    println!(
        "Charlie consumed swapp_note_1 Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    client.submit_transaction(tx_result).await?;

    let duration_charlie = start_time_charlie.elapsed();
    println!("SWAPP note partially filled by Charlie");
    println!("Time for Charlie’s partial fill: {duration_charlie:?}");

    println!(
        "SWAPP note leftover after Charlie’s partial fill => A: {}, B: {}",
        new_offered_asset_amount_1, new_requested_asset_amount_1
    );
    println!("Done with partial swap ephemeral chain test.");

    Ok(())
}

#[tokio::test]
async fn swap_note_instant_cancel_test() -> Result<(), ClientError> {
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

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Create cancellable SWAPP note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    // offered asset amount
    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    // requested asset amount
    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let mut swap_secret = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    swap_secret.splice(0..0, Word::default().iter().cloned());
    let swap_secret_hash = Hasher::hash_elements(&swap_secret);

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note_cancellable(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_secret_hash.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    let swapp_note_id = swapp_note.id();

    // Time from after SWAPP creation
    let start_time = Instant::now();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 2: Partial Consume SWAPP note
    // -------------------------------------------------------------------------
    let fill_amount_bob = 25;
    let (_amount_a_1, new_amount_a, new_amount_b) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    // leftover portion of Alice’s original order
    let swapp_note_1 = create_partial_swap_note_cancellable(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_amount_a)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_amount_b)
            .unwrap()
            .into(),
        swap_secret_hash.into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // P2ID note for Bob’s partial fill going to Alice
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);

    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    client.sync_state().await?;

    // pass in amount to fill via note args
    // simulate that BOB has to get this from a data base
    // time how long it would take to retrive this secret from a database
    let secret: Word = [Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];

    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    let mut advice_map = AdviceMap::default();

    let note_args_value = vec![
        secret[0],
        secret[1],
        secret[2],
        secret[3],
        consume_amount_note_args[0],
        consume_amount_note_args[1],
        consume_amount_note_args[2],
        consume_amount_note_args[3],
    ];
    println!("note_args: {:?}", note_args_value);

    let note_args_commitment = Rpo256::hash_elements(&note_args_value);

    advice_map.insert(note_args_commitment.into(), note_args_value.to_vec());

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), Some(note_args_commitment.into()))])
        .with_expected_output_notes(vec![p2id_note, swapp_note_1])
        .extend_advice_map(advice_map)
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());

    let _ = client.submit_transaction(tx_result).await;

    // Stop timing
    let duration = start_time.elapsed();
    println!("SWAPP note partially filled");
    println!("Time from SWAPP creation to partial fill: {:?}", duration);

    Ok(())
}

#[tokio::test]
async fn secret_drop_test() {
    // A bit of a redundant test, but it proves a point that cancellation of an order
    // that uses a hash preimage secret, is as fast as dropping a value from a
    // database.
    // Shared in-memory "database" using a HashMap wrapped in a Mutex.
    let secret_db: Arc<Mutex<HashMap<&str, Word>>> = Arc::new(Mutex::new(HashMap::new()));
    let key = "swap_secret";

    // 1. Push the secret into the "database".
    let secret: Word = [Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    {
        let mut db = secret_db.lock().unwrap();
        db.insert(key, secret);
    }
    println!("Secret inserted into the database.");

    // 2. Alice drops (removes) the secret and we time how long it takes.
    let alice_db = secret_db.clone();
    let start_time = Instant::now();
    {
        let mut db = alice_db.lock().unwrap();
        db.remove(key);
    }
    let drop_duration = start_time.elapsed();
    println!("Alice dropped the secret in: {:?}", drop_duration);

    // 3. Bob attempts to retrieve the secret.
    let bob_db = secret_db.clone();
    let retrieved_secret = {
        let db = bob_db.lock().unwrap();
        db.get(key).cloned() // Attempt to get the secret.
    };

    match retrieved_secret {
        Some(secret) => {
            println!("Bob retrieved the secret: {:?}", secret);
        }
        None => {
            println!("Bob could not retrieve the secret; order is cancelled.");
        }
    }
}

#[tokio::test]
async fn test_compute_partial_swapp() -> Result<(), ClientError> {
    let amount_b_in = 25;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        100,         // originally offered A
        50,          // originally requested B
        amount_b_in, // Bob fills 25 B
    );

    println!("==== test_compute_partial_swapp ====");
    println!("amount_a_1 (A out):          {}", amount_a_1);
    println!("amount_b_1 (B in):           {}", amount_b_in);
    println!("new_amount_a (A leftover):   {}", new_amount_a);
    println!("new_amount_b (B leftover):   {}", new_amount_b);

    assert_eq!(amount_a_1, 50);
    assert_eq!(amount_b_in, 25);
    assert_eq!(new_amount_a, 50);
    assert_eq!(new_amount_b, 25);

    let amount_b_in = 2500;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        5000,        // originally offered A
        5000,        // originally requested B
        amount_b_in, // Bob fills 2500 B
    );

    // 1. For a 1:1 ratio: 2500 B in => 2500 A out => leftover 2500 A / 2500 B
    assert_eq!(amount_a_1, 2500);
    assert_eq!(amount_b_in, 2500);
    assert_eq!(new_amount_a, 2500);
    assert_eq!(new_amount_b, 2500);

    Ok(())
}
