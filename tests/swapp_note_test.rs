use std::time::Instant;

use miden_client::{
    asset::FungibleAsset,
    note::NoteType,
    transaction::{OutputNote, TransactionRequestBuilder},
    ClientError, Felt,
};

use miden_crypto::rand::FeltRng;

use miden_clob_designs::common::{
    compute_partial_swapp, create_p2id_note, create_partial_swap_note, get_p2id_serial_num,
    get_swapp_note, initialize_client, reset_store_sqlite, setup_accounts_and_faucets,
};

#[tokio::test]
async fn swap_note_partial_consume_public_test() -> Result<(), ClientError> {
    // reset store.sqlite file
    reset_store_sqlite().await;

    // create & sync the client
    let mut client = initialize_client().await?;
    println!(
        "Client initialized successfully. Latest block: {}",
        client.sync_state().await.unwrap().block_num
    );

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) = setup_accounts_and_faucets(&mut client, 2, 2, balances).await?;

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
        .unwrap()
        .build();
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
    // STEP 2: Partial Consume SWAPP note (unchanged)
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

    println!("CONSUMING NOTE");
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
        .build();

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
async fn partial_swap_ephemeral_chain_benchmark() -> Result<(), ClientError> {
    // Reset store
    reset_store_sqlite().await;

    // Initialize & sync client
    let mut client = initialize_client().await?;
    println!(
        "Client initialized successfully. Latest block: {}",
        client.sync_state().await?.block_num
    );

    // -------------------------------------------------------------------------
    // NEW: Use setup_accounts_and_faucets for Steps 1 & 2
    // -------------------------------------------------------------------------
    // We'll have 3 accounts: Alice, Bob, Charlie
    // We'll have 2 faucets: faucet A, faucet B
    //
    // Each row in `balances` corresponds to an account;
    // each column corresponds to a faucet.
    // So:
    //   - Alice gets 100 tokens from faucet A,  0 from faucet B
    //   - Bob   gets   0 tokens from faucet A, 25 from faucet B
    //   - Charlie gets 0 tokens from faucet A, 10 from faucet B
    let balances = vec![
        vec![100, 0], // Alice
        vec![0, 100], // Bob
        vec![0, 100], // Charlie
    ];

    // This returns `(Vec<Account>, Vec<Account>)` => (accounts, faucets)
    let (accounts, faucets) = setup_accounts_and_faucets(&mut client, 3, 2, balances).await?;

    // Rename them for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let charlie_account = accounts[2].clone();

    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 3: Alice creates a SWAPP note: 50 A → 50 B
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
        .unwrap()
        .build();
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
    // STEP 4: Bob partially consumes swapp_note (optimistically)
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
        .build();

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
    // STEP 5: Charlie partially consumes swapp_note_1 (optimistically)
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

    // leftover portion -> swapp_note_2
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

    // p2id note for Charlie’s partial fill => 10 B in for Alice
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

    // Use optimistic consumption again
    let consume_amount_note_args_charlie = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_charlie),
    ];
    let consume_custom_req_charlie = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note_1, Some(consume_amount_note_args_charlie))])
        .with_expected_output_notes(vec![p2id_note_2.clone(), swapp_note_2.clone()])
        .build();

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
